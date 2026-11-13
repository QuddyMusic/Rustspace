//! Догонялка: события, пропущенные за время простоя, забираются из журнала
//! мутаций пира (`utils/federationSync.ts`).
//!
//! Для каждого пира и каждого вида событий (`dm`, `friend`, `profile`)
//! хранится курсор. Проход запрашивает страницы `/api/federation/sync` и
//! применяет события так же, как присланные; событие, которое пока нельзя
//! применить, откладывается в `federation_sync_retry` и повторяется позже,
//! а следующие события того же субъекта ждут за ним.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::fetch::{self, FetchError, Trust};
use super::inbound::{self, Delivery};
use super::outbox::{self, Verdict};
use super::peers::{self, Peer};
use super::{our_origin, relay};
use crate::db::{DbResult, Tx};
use crate::federation_auth::{self, normalize_origin};
use crate::params;
use crate::state::{AppState, now_ms};

/// Виды событий.
pub const ALL_CONTEXTS: [&str; 3] = ["dm", "friend", "profile"];
const PAGE_LIMIT: i64 = 100;
const FIRST_PAGE_OVERLAP_MS: i64 = 120_000;
const RESYNC_INTERVAL: Duration = Duration::from_mins(15);
const RESYNC_FIRST_DELAY: Duration = Duration::from_mins(1);
const RESYNC_CONTINUE_DELAY: Duration = Duration::from_mins(1);
const RETRY_INTERVAL: Duration = Duration::from_mins(5);
const RETRY_BACKOFF_MS: [i64; 6] = [
    60_000, 300_000, 1_800_000, 7_200_000, 21_600_000, 86_400_000,
];
const RETRY_MAX_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const MUTATION_LOG_RETENTION_MS: i64 = 90 * 24 * 60 * 60 * 1000;
const PEER_CONCURRENCY: usize = 4;

/// Предел одного прохода (`SyncPassBudget`).
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// Страниц.
    pub pages: u32,
    /// Времени.
    pub time: Duration,
}

/// Обычный предел (`SYNC_PASS_BUDGET`).
pub const PASS_BUDGET: Budget = Budget {
    pages: 20,
    time: Duration::from_secs(45),
};

/// Итог прохода по пиру (`SyncPeerResult`).
#[derive(Debug, Clone, Default)]
pub struct Stats {
    /// Итог по видам: `ok`, `partial`, `http_error`, `error`.
    pub contexts: Vec<(String, &'static str)>,
    /// Применено.
    pub applied: u64,
    /// Уже было.
    pub duplicates: u64,
    /// Отложено.
    pub deferred: u64,
    /// Отброшено.
    pub dropped: u64,
}

impl Stats {
    /// В виде ответа JSON.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let contexts: serde_json::Map<String, Value> = self
            .contexts
            .iter()
            .map(|(context, outcome)| (context.clone(), json!(outcome)))
            .collect();
        json!({
            "contexts": contexts,
            "applied": self.applied,
            "duplicates": self.duplicates,
            "deferred": self.deferred,
            "dropped": self.dropped,
        })
    }
}

/// Очереди проходов по пирам (`runForPeer`): проходы одного пира идут по одному.
#[derive(Debug, Default)]
pub struct Locks {
    peers: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Locks {
    fn of(&self, peer_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(peer_id.to_owned())
            .or_default()
            .clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Cursor {
    ts: i64,
    id: Option<String>,
}

fn is_after(a: &Cursor, b: &Cursor) -> bool {
    if a.ts != b.ts {
        return a.ts > b.ts;
    }
    match (&a.id, &b.id) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(a), Some(b)) => {
            if a.len() == b.len() {
                a > b
            } else {
                a.len() > b.len()
            }
        }
    }
}

fn friend_history_floor(tx: &mut dyn Tx, now: i64) -> DbResult<Option<i64>> {
    let started = tx
        .query_one(
            "SELECT ledger_started_at FROM instance_settings WHERE id = 1",
            params![],
        )?
        .map(|row| row.opt_int("ledger_started_at"))
        .transpose()?
        .flatten();
    Ok(started.filter(|started| now - started <= MUTATION_LOG_RETENTION_MS))
}

fn first_since(tx: &mut dyn Tx, context: &str, cursor: &Cursor, now: i64) -> DbResult<i64> {
    let since = (cursor.ts - FIRST_PAGE_OVERLAP_MS).max(0);
    if context != "friend" {
        return Ok(since);
    }
    Ok(match friend_history_floor(tx, now)? {
        Some(floor) => since.max(floor),
        None => since,
    })
}

/// Новое воплощение пира начинает курсоры заново (`reconcileCursorEpoch`).
fn reconcile_epoch(tx: &mut dyn Tx, peer: &Peer) -> DbResult<()> {
    let Some(epoch) = &peer.epoch else {
        return Ok(());
    };
    let mut stale = false;
    for row in tx.query(
        "SELECT peer_epoch FROM federation_sync_cursors WHERE peer_id = ?",
        params![&peer.id],
    )? {
        if row.opt_text("peer_epoch")?.is_some_and(|e| &e != epoch) {
            stale = true;
        }
    }
    if stale {
        tracing::warn!(
            peer = peer.origin,
            "новое воплощение пира: курсоры догонялки сначала"
        );
        tx.execute(
            "UPDATE federation_sync_cursors SET cursor_ts = 0, cursor_id = NULL WHERE peer_id = ?",
            params![&peer.id],
        )?;
        tx.execute(
            "DELETE FROM federation_sync_retry WHERE peer_id = ?",
            params![&peer.id],
        )?;
    }
    tx.execute(
        "UPDATE federation_sync_cursors SET peer_epoch = ? WHERE peer_id = ?",
        params![epoch, &peer.id],
    )?;
    Ok(())
}

fn load_cursor(tx: &mut dyn Tx, peer: &Peer, context: &str) -> DbResult<Cursor> {
    if let Some(row) = tx.query_one(
        "SELECT cursor_ts, cursor_id FROM federation_sync_cursors WHERE peer_id = ? AND context_type = ?",
        params![&peer.id, context],
    )? {
        return Ok(Cursor {
            ts: row.int("cursor_ts")?,
            id: row.opt_text("cursor_id")?,
        });
    }
    tx.execute(
        "INSERT INTO federation_sync_cursors (peer_id, context_type, cursor_ts, cursor_id, peer_epoch) \
         VALUES (?, ?, 0, NULL, ?) ON CONFLICT DO NOTHING",
        params![&peer.id, context, &peer.epoch],
    )?;
    Ok(Cursor { ts: 0, id: None })
}

fn identity_key(identity: Option<&Value>) -> [Value; 2] {
    let user = identity
        .and_then(|i| i.get("homeUserId"))
        .and_then(Value::as_str)
        .map_or(Value::Null, |id| json!(id));
    let home = identity
        .and_then(|i| i.get("homeInstance"))
        .and_then(Value::as_str)
        .map_or(Value::Null, |home| {
            json!(normalize_origin(Some(home)).unwrap_or_else(|| home.to_owned()))
        });
    [user, home]
}

fn js(items: &[Value]) -> String {
    serde_json::to_string(items).unwrap_or_default()
}

/// Субъект события: за отложенным событием ждут следующие того же субъекта
/// (`syncSubjectKey`).
#[must_use]
pub fn subject_key(config: &crate::config::Config, event: &Value) -> String {
    let kind = event.get("eventType").cloned().unwrap_or(Value::Null);
    let id = event.get("messageId").cloned().unwrap_or(Value::Null);
    let own = || js(&[json!("event"), kind.clone(), id.clone()]);
    let scope = || {
        event
            .get("federatedId")
            .filter(|v| !v.is_null())
            .or_else(|| event.get("dmChannelId").filter(|v| !v.is_null()))
            .cloned()
            .unwrap_or(Value::Null)
    };
    match kind.as_str().unwrap_or_default() {
        "create" | "update" | "delete" | "reaction_add" | "reaction_remove" => {
            js(&[json!("message"), id.clone()])
        }
        "file_rejected" => js(&[json!("message_here"), id.clone()]),
        "member_add" | "member_remove" => relay::member_subject(
            config,
            event.get("federatedId").and_then(Value::as_str),
            event.pointer("/membership/user").unwrap_or(&Value::Null),
        )
        .unwrap_or_else(own),
        "friend_request_create"
        | "friend_request_update"
        | "friend_request_cancel"
        | "friend_add"
        | "friend_remove" => relay::friend_subject(
            config,
            event.pointer("/friendship/from").unwrap_or(&Value::Null),
            event.pointer("/friendship/to").unwrap_or(&Value::Null),
        )
        .unwrap_or_else(own),
        "ownership_transfer" | "group_metadata_update" => js(&[json!("group"), scope()]),
        "dm_close" | "dm_reopen" => {
            let [user, home] = identity_key(event.get("dmCloseReopen"));
            js(&[json!("closed"), scope(), user, home])
        }
        "read_state_update" => {
            let [user, home] = identity_key(event.pointer("/readState/user"));
            js(&[json!("read"), scope(), user, home])
        }
        "profile_update" => {
            let [user, home] = identity_key(event.get("profileUpdate"));
            js(&[json!("profile"), user, home])
        }
        _ => own(),
    }
}

fn named_message(event: &Value, peer_origin: &str) -> Option<(String, String)> {
    match event.get("eventType").and_then(Value::as_str) {
        Some("update") => match event.get("target") {
            None => Some((
                event.get("messageId")?.as_str()?.to_owned(),
                peer_origin.to_owned(),
            )),
            Some(target) => Some((
                target.pointer("/message/messageId")?.as_str()?.to_owned(),
                target
                    .pointer("/message/messageHomeInstance")?
                    .as_str()?
                    .to_owned(),
            )),
        },
        Some("reaction_add") => {
            let reaction = event.get("reaction").filter(|r| inbound::truthy(r))?;
            let id = reaction
                .get("messageId")
                .filter(|v| !v.is_null())
                .or_else(|| event.get("messageId"))?
                .as_str()?
                .to_owned();
            let home = reaction
                .get("messageHomeInstance")
                .and_then(Value::as_str)
                .filter(|h| !h.is_empty())
                .unwrap_or(peer_origin)
                .to_owned();
            Some((id, home))
        }
        _ => None,
    }
}

fn cannot_arrive(tx: &mut dyn Tx, app: &AppState, peer: &Peer, event: &Value) -> DbResult<bool> {
    let Some((id, home)) = named_message(event, &peer.origin) else {
        return Ok(true);
    };
    let Some(normalized) = normalize_origin(Some(&home)) else {
        return Ok(true);
    };
    if Some(&normalized) == normalize_origin(Some(&our_origin(&app.config))).as_ref()
        || Some(&normalized) == normalize_origin(Some(&peer.origin)).as_ref()
    {
        return Ok(true);
    }
    inbound::has_applied(tx, &home, &format!("dm_delete:{id}"))
}

enum Pulled {
    Applied,
    Taken,
    Refused(String, String),
    Retry(String),
}

async fn apply_pulled(app: &AppState, peer: &Peer, event: &Value) -> Pulled {
    let outcome = inbound::process_events(
        app,
        std::slice::from_ref(event),
        &peer.origin,
        &peer.origin,
        Delivery::CatchUp,
    )
    .await;
    let id = event
        .get("messageId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let rejection = outcome
        .rejected
        .iter()
        .find(|(rejected, _)| rejected == id)
        .or_else(|| outcome.rejected.first());
    let Some((_, reason)) = rejection else {
        return Pulled::Applied;
    };
    let kind = event.get("eventType").and_then(Value::as_str);
    let verdict = outbox::classify_listed(kind, reason).unwrap_or_else(|| {
        tracing::warn!(
            reason,
            ?kind,
            "неизвестная причина отказа: событие догонялки отброшено"
        );
        Verdict::Refused
    });
    match verdict {
        Verdict::Taken => Pulled::Taken,
        Verdict::Refused => Pulled::Refused(reason.clone(), "refused".to_owned()),
        Verdict::Retry => {
            if reason == "unknown_message" {
                let (app_tx, peer_tx, event_tx) = (app.clone(), peer.clone(), event.clone());
                let lost = app
                    .db
                    .transaction(move |tx: &mut dyn Tx| {
                        cannot_arrive(tx, &app_tx, &peer_tx, &event_tx)
                    })
                    .await
                    .unwrap_or(false);
                if lost {
                    return Pulled::Refused(
                        reason.clone(),
                        "its message was refused, deleted or never served".to_owned(),
                    );
                }
            }
            Pulled::Retry(reason.clone())
        }
    }
}

/// Запись JSON с ключами по порядку (`canonicalJson`).
fn canonical(value: &Value) -> String {
    match value {
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        Value::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|key| format!(
                        "{}:{}",
                        crate::jsjson::stringify(&json!(key)),
                        canonical(&fields[*key])
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        other => crate::jsjson::stringify(other),
    }
}

/// Отпечаток события (`syncEventHash`).
#[must_use]
pub fn event_hash(event: &Value) -> String {
    hex::encode(Sha256::digest(canonical(event).as_bytes()))
}

#[allow(clippy::too_many_arguments, reason = "поля отложенного события")]
fn keep_event(
    tx: &mut dyn Tx,
    ids: &crate::snowflake::Snowflake,
    peer_id: &str,
    context: &str,
    subject: &str,
    event: &Value,
    reason: &str,
    next_retry_at: i64,
    now: i64,
) -> DbResult<()> {
    tx.execute(
        "INSERT INTO federation_sync_retry (id, peer_id, context_type, subject_key, event_type, message_id, \
         event_ts, event_hash, event_json, last_reason, attempts, first_failed_at, next_retry_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?) ON CONFLICT DO NOTHING",
        params![
            ids.next(),
            peer_id,
            context,
            subject,
            event.get("eventType").and_then(Value::as_str).unwrap_or_default(),
            event.get("messageId").and_then(Value::as_str).unwrap_or_default(),
            inbound::millis(event.get("timestamp")),
            event_hash(event),
            crate::jsjson::stringify(event),
            reason,
            now,
            next_retry_at
        ],
    )?;
    Ok(())
}

fn log_dropped(peer: &Peer, event: &Value, reason: &str, why: &str) {
    tracing::warn!(
        kind = ?event.get("eventType"),
        event = ?event.get("messageId"),
        peer = peer.origin,
        reason,
        why,
        "событие догонялки отброшено"
    );
}

async fn handle_pulled(
    app: &AppState,
    peer: &Peer,
    context: &str,
    event: &Value,
    stats: &mut Stats,
) {
    let now = now_ms();
    let subject = subject_key(&app.config, event);
    let (peer_id, subject_tx, context_tx, event_tx, app_tx) = (
        peer.id.clone(),
        subject.clone(),
        context.to_owned(),
        event.clone(),
        app.clone(),
    );
    let held = app
        .db
        .transaction(move |tx: &mut dyn Tx| {
            let held = tx
                .query_one(
                    "SELECT id FROM federation_sync_retry WHERE peer_id = ? AND subject_key = ? LIMIT 1",
                    params![&peer_id, &subject_tx],
                )?
                .is_some();
            if held {
                keep_event(
                    tx,
                    &app_tx.ids,
                    &peer_id,
                    &context_tx,
                    &subject_tx,
                    &event_tx,
                    "held_behind_earlier_event",
                    now,
                    now,
                )?;
            }
            Ok::<_, crate::db::DbError>(held)
        })
        .await
        .unwrap_or(false);
    if held {
        stats.deferred += 1;
        return;
    }
    match apply_pulled(app, peer, event).await {
        Pulled::Applied => stats.applied += 1,
        Pulled::Taken => stats.duplicates += 1,
        Pulled::Refused(reason, why) => {
            log_dropped(peer, event, &reason, &why);
            stats.dropped += 1;
        }
        Pulled::Retry(reason) => {
            let (peer_id, context, event, app_tx) = (
                peer.id.clone(),
                context.to_owned(),
                event.clone(),
                app.clone(),
            );
            let kept = app
                .db
                .transaction(move |tx: &mut dyn Tx| {
                    keep_event(
                        tx,
                        &app_tx.ids,
                        &peer_id,
                        &context,
                        &subject,
                        &event,
                        &reason,
                        now + RETRY_BACKOFF_MS[0],
                        now,
                    )
                })
                .await;
            if let Err(error) = kept {
                tracing::error!(%error, "событие догонялки не отложено");
            }
            stats.deferred += 1;
        }
    }
}

fn sync_response(value: &Value) -> Option<(&Vec<Value>, bool, i64, Option<String>)> {
    let events = value.get("events")?.as_array()?;
    let more = value.get("hasMore")?.as_bool()?;
    let checkpoint = value.get("checkpoint")?.as_f64()?;
    let id = match value.get("checkpointId") {
        None => None,
        Some(Value::String(id)) => Some(id.clone()),
        Some(_) => return None,
    };
    #[allow(clippy::cast_possible_truncation, reason = "время в миллисекундах")]
    Some((events, more, checkpoint as i64, id))
}

async fn pull_context(
    app: &AppState,
    peer: &Peer,
    context: &str,
    secret: &str,
    stats: &mut Stats,
    budget: Budget,
) -> &'static str {
    let deadline = Instant::now() + budget.time;
    let (peer_tx, context_tx) = (peer.clone(), context.to_owned());
    let start = app
        .db
        .transaction(move |tx: &mut dyn Tx| {
            let cursor = load_cursor(tx, &peer_tx, &context_tx)?;
            let since = first_since(tx, &context_tx, &cursor, now_ms())?;
            Ok::<_, crate::db::DbError>((cursor, since))
        })
        .await;
    let Ok((mut position, mut since)) = start else {
        return "error";
    };
    let mut after_id: Option<String> = None;
    let mut previous: Option<String> = None;
    let ours = our_origin(&app.config);
    let mut pages = 0;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if pages >= budget.pages || remaining.is_zero() {
            tracing::info!(
                context,
                peer = peer.origin,
                pages,
                "догонялка остановилась на пределе прохода"
            );
            return "partial";
        }
        pages += 1;
        let mut request =
            json!({"sinceTimestamp": since, "limit": PAGE_LIMIT, "contextType": context});
        if let Some(id) = &after_id {
            request["afterId"] = json!(id);
        }
        let key = format!("{since}|{}", after_id.as_deref().unwrap_or_default());
        if previous.as_ref() == Some(&key) {
            tracing::warn!(
                context,
                peer = peer.origin,
                since,
                "догонялка не продвинулась; проход остановлен"
            );
            break;
        }
        previous = Some(key);
        let body = crate::jsjson::stringify(&request);
        let headers = federation_auth::headers(&body, secret, &ours, now_ms());
        let limit = remaining.min(FETCH_TIMEOUT);
        let answer = fetch::request(
            &app.config,
            &peer.origin,
            "/api/federation/sync",
            Some(body),
            &headers,
            Trust::Approved,
            limit,
        )
        .await;
        let answer = match answer {
            Ok(answer) => answer,
            Err(FetchError::Timeout) if remaining <= FETCH_TIMEOUT => {
                tracing::info!(
                    context,
                    peer = peer.origin,
                    "догонялка остановилась на пределе времени"
                );
                return "partial";
            }
            Err(error) => {
                tracing::error!(context, peer = peer.origin, ?error, "догонялка не удалась");
                return "error";
            }
        };
        if !answer.ok() {
            tracing::warn!(
                context,
                peer = peer.origin,
                status = answer.status,
                "догонялка: пир ответил ошибкой"
            );
            return "http_error";
        }
        let Ok(data) = serde_json::from_slice::<Value>(&answer.body) else {
            tracing::error!(context, peer = peer.origin, "догонялка: ответ не JSON");
            return "error";
        };
        let Some((events, more, checkpoint, checkpoint_id)) = sync_response(&data) else {
            tracing::warn!(context, peer = peer.origin, "догонялка: ответ непонятен");
            return "http_error";
        };
        for event in events {
            handle_pulled(app, peer, context, event, stats).await;
        }
        let page = Cursor {
            ts: checkpoint,
            id: checkpoint_id.clone(),
        };
        if is_after(&page, &position) {
            position = page;
            let (peer_id, context_tx, saved) =
                (peer.id.clone(), context.to_owned(), position.clone());
            let _ = app
                .db
                .transaction(move |tx: &mut dyn Tx| {
                    tx.execute(
                        "UPDATE federation_sync_cursors SET cursor_ts = ?, cursor_id = ? \
                         WHERE peer_id = ? AND context_type = ?",
                        params![saved.ts, &saved.id, &peer_id, &context_tx],
                    )
                })
                .await;
        }
        if !more {
            break;
        }
        match checkpoint_id {
            Some(id) => {
                since = checkpoint;
                after_id = Some(id);
            }
            None => {
                let mut next = checkpoint - 1;
                if next <= since {
                    next = checkpoint;
                    tracing::warn!(
                        context,
                        peer = peer.origin,
                        checkpoint,
                        "полная страница с одним временем"
                    );
                }
                since = next;
                after_id = None;
            }
        }
    }
    let (peer_id, context_tx) = (peer.id.clone(), context.to_owned());
    let _ = app
        .db
        .transaction(move |tx: &mut dyn Tx| {
            tx.execute(
                "UPDATE federation_sync_cursors SET last_pulled_at = ? WHERE peer_id = ? AND context_type = ?",
                params![now_ms(), &peer_id, &context_tx],
            )
        })
        .await;
    "ok"
}

async fn relay_enabled(app: &AppState) -> bool {
    app.db
        .transaction(|tx: &mut dyn Tx| super::relay_enabled(tx))
        .await
        .unwrap_or(false)
}

/// Проход догонялки по пиру (`syncPeerMutationLog`); `None` — пира нет,
/// он не действует или пересылка выключена.
pub async fn sync_peer(
    app: &AppState,
    peer_id: &str,
    reason: &str,
    contexts: &[&str],
    budget: Budget,
) -> Option<Stats> {
    let lock = app.federation.sync_locks.of(peer_id);
    let _guard = lock.lock().await;
    app.federation.pulls.hold();
    let stats = pull_peer(app, peer_id, reason, contexts, budget).await;
    app.federation.pulls.release_hold();
    super::lookup::start_home_pulls(app);
    stats
}

async fn pull_peer(
    app: &AppState,
    peer_id: &str,
    reason: &str,
    contexts: &[&str],
    budget: Budget,
) -> Option<Stats> {
    if !relay_enabled(app).await {
        return None;
    }
    let id = peer_id.to_owned();
    let peer = app
        .db
        .transaction(move |tx: &mut dyn Tx| {
            let peer = peers::by_id(tx, &id)?;
            if let Some(peer) = peer.as_ref().filter(|peer| peer.status == "active") {
                reconcile_epoch(tx, peer)?;
            }
            Ok::<_, crate::db::DbError>(peer)
        })
        .await
        .ok()
        .flatten()
        .filter(|peer| peer.status == "active")?;
    let secret = match (&peer.pending_secret, peer.rotation_at) {
        (Some(pending), Some(_)) => pending.clone(),
        _ => peer.secret.clone(),
    };
    let mut stats = Stats::default();
    for context in contexts {
        let outcome = pull_context(app, &peer, context, &secret, &mut stats, budget).await;
        stats.contexts.push(((*context).to_owned(), outcome));
    }
    if stats.contexts.iter().all(|(_, outcome)| *outcome == "ok") {
        let id = peer.id.clone();
        let _ = app
            .db
            .transaction(move |tx: &mut dyn Tx| {
                tx.execute(
                    "UPDATE federation_peers SET last_synced_at = ? WHERE id = ?",
                    params![now_ms(), &id],
                )
            })
            .await;
    }
    if stats.applied + stats.deferred + stats.dropped > 0 {
        tracing::info!(
            peer = peer.origin,
            reason,
            applied = stats.applied,
            duplicates = stats.duplicates,
            deferred = stats.deferred,
            dropped = stats.dropped,
            "догонялка забрала события пира"
        );
    }
    Some(stats)
}

fn retry_backoff(attempts: i64) -> i64 {
    let index = usize::try_from(attempts.max(1))
        .unwrap_or(1)
        .min(RETRY_BACKOFF_MS.len())
        - 1;
    RETRY_BACKOFF_MS[index]
}

async fn retry_peer(app: &AppState, peer: &Peer, now: i64) -> u64 {
    let id = peer.id.clone();
    let rows = app
        .db
        .transaction(move |tx: &mut dyn Tx| {
            tx.query(
                "SELECT * FROM federation_sync_retry WHERE peer_id = ? ORDER BY event_ts ASC, id ASC",
                params![&id],
            )
        })
        .await
        .unwrap_or_default();
    let mut resolved = 0;
    let mut seen = HashSet::new();
    let mut blocked = HashSet::new();
    for row in rows {
        let (Ok(id), Ok(subject)) = (row.text("id"), row.text("subject_key")) else {
            continue;
        };
        if blocked.contains(&subject) {
            continue;
        }
        let head = seen.insert(subject.clone());
        if head && row.int("next_retry_at").unwrap_or(0) > now {
            blocked.insert(subject);
            continue;
        }
        let delete = |app: &AppState, id: String| {
            let app = app.clone();
            async move {
                let _ = app
                    .db
                    .transaction(move |tx: &mut dyn Tx| {
                        tx.execute(
                            "DELETE FROM federation_sync_retry WHERE id = ?",
                            params![&id],
                        )
                    })
                    .await;
            }
        };
        let event: Option<Value> = row
            .text("event_json")
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
        let Some(event) = event else {
            delete(app, id.clone()).await;
            tracing::warn!(
                entry = id,
                peer = peer.origin,
                "нечитаемое отложенное событие отброшено"
            );
            resolved += 1;
            continue;
        };
        let reason = row.text("last_reason").unwrap_or_default();
        if now - row.int("first_failed_at").unwrap_or(now) > RETRY_MAX_AGE_MS {
            delete(app, id).await;
            log_dropped(peer, &event, &reason, "still refused after 7 days");
            resolved += 1;
            continue;
        }
        match apply_pulled(app, peer, &event).await {
            Pulled::Retry(reason) => {
                let attempts = row.int("attempts").unwrap_or(1) + 1;
                let _ = app
                    .db
                    .transaction(move |tx: &mut dyn Tx| {
                        tx.execute(
                            "UPDATE federation_sync_retry SET attempts = ?, last_reason = ?, next_retry_at = ? WHERE id = ?",
                            params![attempts, &reason, now + retry_backoff(attempts), &id],
                        )
                    })
                    .await;
                blocked.insert(subject);
            }
            outcome => {
                delete(app, id).await;
                if let Pulled::Refused(reason, why) = outcome {
                    log_dropped(peer, &event, &reason, &format!("{why} on retry"));
                }
                resolved += 1;
            }
        }
    }
    resolved
}

/// Повтор отложенных событий (`processSyncRetryTick`).
pub async fn retry_tick(app: &AppState, now: i64) -> u64 {
    if !relay_enabled(app).await {
        return 0;
    }
    let peer_ids: Vec<String> = app
        .db
        .transaction(|tx: &mut dyn Tx| {
            tx.query(
                "SELECT DISTINCT peer_id FROM federation_sync_retry",
                params![],
            )
        })
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|row| row.text("peer_id").ok())
        .collect();
    let mut total = 0;
    for peer_id in peer_ids {
        let lock = app.federation.sync_locks.of(&peer_id);
        let _guard = lock.lock().await;
        let id = peer_id.clone();
        let peer = app
            .db
            .transaction(move |tx: &mut dyn Tx| {
                let peer = peers::by_id(tx, &id)?;
                if let Some(peer) = peer.as_ref().filter(|peer| peer.status == "active") {
                    reconcile_epoch(tx, peer)?;
                }
                Ok::<_, crate::db::DbError>(peer)
            })
            .await
            .ok()
            .flatten()
            .filter(|peer| peer.status == "active");
        if let Some(peer) = peer {
            total += retry_peer(app, &peer, now).await;
        }
    }
    total
}

/// Периодическая догонялка по всем действующим пирам (`processResyncTick`);
/// `true` — какой-то проход остановился на пределе.
pub async fn resync_tick(app: &AppState, budget: Budget) -> bool {
    if !relay_enabled(app).await {
        return false;
    }
    let peer_ids: Vec<String> = app
        .db
        .transaction(|tx: &mut dyn Tx| {
            tx.query(
                "SELECT id FROM federation_peers WHERE status = 'active'",
                params![],
            )
        })
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|row| row.text("id").ok())
        .collect();
    let mut partial = false;
    for chunk in peer_ids.chunks(PEER_CONCURRENCY) {
        let mut runs = tokio::task::JoinSet::new();
        for id in chunk {
            let (app, id) = (app.clone(), id.clone());
            runs.spawn(
                async move { sync_peer(&app, &id, "periodic", &ALL_CONTEXTS, budget).await },
            );
        }
        while let Some(result) = runs.join_next().await {
            if let Ok(Some(result)) = result
                && result
                    .contexts
                    .iter()
                    .any(|(_, outcome)| *outcome == "partial")
            {
                partial = true;
            }
        }
    }
    retry_tick(app, now_ms()).await;
    partial
}

/// Фоновые проходы догонялки и повторов (`startPeerSyncWorkers`).
pub fn spawn(app: &AppState) {
    let resync = app.clone();
    tokio::spawn(async move {
        let mut delay = RESYNC_FIRST_DELAY;
        loop {
            tokio::time::sleep(delay).await;
            let partial = resync_tick(&resync, PASS_BUDGET).await;
            delay = if partial {
                RESYNC_CONTINUE_DELAY
            } else {
                RESYNC_INTERVAL
            };
        }
    });
    let retry = app.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(RETRY_INTERVAL).await;
            retry_tick(&retry, now_ms()).await;
        }
    });
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    #[test]
    fn cursor_order() {
        let at = |ts, id: Option<&str>| Cursor {
            ts,
            id: id.map(str::to_owned),
        };
        assert!(is_after(&at(2, None), &at(1, Some("9"))));
        assert!(is_after(&at(1, Some("10")), &at(1, Some("9"))));
        assert!(!is_after(&at(1, None), &at(1, Some("1"))));
        assert!(is_after(&at(1, Some("1")), &at(1, None)));
    }

    #[test]
    fn canonical_json_sorts_keys() {
        assert_eq!(
            canonical(&json!({"b": 1, "a": [true, null]})),
            r#"{"a":[true,null],"b":1}"#
        );
        assert_eq!(event_hash(&json!({})).len(), 64);
        // Порядок ключей не влияет на отпечаток.
        assert_eq!(
            event_hash(&json!({"a": 1, "b": {"d": 2, "c": 3}})),
            event_hash(&json!({"b": {"c": 3, "d": 2}, "a": 1}))
        );
        assert_ne!(event_hash(&json!({"a": 1})), event_hash(&json!({"a": 2})));
    }

    #[test]
    fn sync_page_shape() {
        let page = json!({"events": [], "hasMore": true, "checkpoint": 5.0, "checkpointId": "7"});
        let (events, more, at, id) = sync_response(&page).expect("страница");
        assert!(events.is_empty() && more && at == 5 && id.as_deref() == Some("7"));
        let without_id = json!({"events": [], "hasMore": false, "checkpoint": 1});
        assert!(sync_response(&without_id).expect("страница").3.is_none());
        // Чужие формы страницы не принимаются.
        assert!(sync_response(&json!({"events": {}, "hasMore": false, "checkpoint": 1})).is_none());
        assert!(sync_response(&json!({"events": [], "hasMore": 1, "checkpoint": 1})).is_none());
        assert!(
            sync_response(
                &json!({"events": [], "hasMore": false, "checkpoint": 1, "checkpointId": 7})
            )
            .is_none()
        );
    }

    #[test]
    fn retry_backoff_steps() {
        assert_eq!(retry_backoff(0), 60_000);
        assert_eq!(retry_backoff(1), 60_000);
        assert_eq!(retry_backoff(2), 300_000);
        assert_eq!(retry_backoff(6), 86_400_000);
        assert_eq!(retry_backoff(100), 86_400_000);
    }

    #[test]
    fn subjects() {
        let vars = [
            ("JWT_SECRET", "s".repeat(40)),
            ("PUBLIC_ORIGIN", "https://x.test".to_owned()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        let config = crate::config::Config::from_map(&vars).expect("конфигурация");
        let key = |event: Value| subject_key(&config, &event);
        // Все правки сообщения — один субъект.
        assert_eq!(
            key(json!({"eventType": "create", "messageId": "m1"})),
            key(json!({"eventType": "reaction_remove", "messageId": "m1"}))
        );
        assert_ne!(
            key(json!({"eventType": "create", "messageId": "m1"})),
            key(json!({"eventType": "create", "messageId": "m2"}))
        );
        // Профиль — по паре (пользователь, дом); дом сравнивается нормализованным.
        assert_eq!(
            key(
                json!({"eventType": "profile_update", "profileUpdate": {"homeUserId": "u", "homeInstance": "https://Y.test/"}})
            ),
            key(
                json!({"eventType": "profile_update", "profileUpdate": {"homeUserId": "u", "homeInstance": "https://y.test"}})
            )
        );
        assert_eq!(
            key(json!({"eventType": "group_metadata_update", "federatedId": "g"})),
            key(json!({"eventType": "ownership_transfer", "federatedId": "g", "dmChannelId": "c"}))
        );
        // Неизвестный тип — сам по себе.
        assert_eq!(
            key(json!({"eventType": "x", "messageId": "m"})),
            r#"["event","x","m"]"#
        );
    }
}
