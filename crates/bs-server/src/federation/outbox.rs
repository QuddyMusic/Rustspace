//! Очередь исходящих событий и их доставка (`utils/federationOutbox.ts`,
//! `utils/federationOutboxQueue.ts`, доставка из `utils/federationWorker.ts`).
//!
//! Событие для другого инстанса записывается в `federation_outbox` — по
//! строке на пира — в той же транзакции, что и само изменение. Строки одной
//! сущности образуют очередь (`queue_key`): уходят по одной и по порядку, а
//! новые события сворачивают ещё не отправленные по правилам ниже. Рабочий
//! раз в секунду забирает головы очередей активных пиров и отправляет их
//! пачкой на `/api/federation/relay`; ответ решает судьбу каждой строки.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};

use super::fetch::{self, FetchError, Trust};
use super::handshake::{self, Ensured, Intent};
use super::peers::{self, NewPeer, Transition};
use crate::db::{DbResult, Row, Tx, Value as Sql};
use crate::params;
use crate::snowflake::Snowflake;
use crate::state::{AppState, now_ms};

/// Как часто рабочий смотрит в очередь.
const TICK: Duration = Duration::from_secs(1);
/// Сколько строк уходит за один проход.
const BATCH_LIMIT: i64 = 50;
/// Сколько ждать ответа на пачку.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);
/// Паузы между попытками доставить строку.
const BACKOFF_MS: [i64; 7] = [
    30_000, 60_000, 300_000, 900_000, 3_600_000, 21_600_000, 86_400_000,
];
/// После стольких сетевых неудач подряд пир считается недоступным.
const UNREACHABLE_THRESHOLD: i64 = 10;
/// После стольких отказов в подписи подряд пир требует внимания.
const AUTH_FAILURE_THRESHOLD: i64 = 5;
/// Что умеет наш приём (`RELAY_CAPABILITIES`).
const CAPABILITIES: [&str; 1] = ["attribution_unproven"];

/// Создан пир-заготовка: администраторам уйдёт `federation_peers_changed`.
static PEERS_CHANGED: AtomicBool = AtomicBool::new(false);

/// Семейство события: как новые события сворачивают очередь.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// Состояние: новое заменяет всё, что ещё в очереди.
    State,
    /// Сообщение: создание, правка, удаление.
    Message,
    /// Событие: всё уходит по порядку.
    Event,
}

fn family(event_type: &str) -> Option<Family> {
    Some(match event_type {
        "create" | "update" | "delete" => Family::Message,
        "reaction_add" | "reaction_remove" | "presence_update" | "profile_update"
        | "read_state_update" | "dm_close" | "dm_reopen" => Family::State,
        "file_rejected"
        | "friend_request_create"
        | "friend_request_update"
        | "friend_request_cancel"
        | "friend_add"
        | "friend_remove"
        | "member_add"
        | "member_remove"
        | "ownership_transfer"
        | "group_metadata_update" => Family::Event,
        _ => return None,
    })
}

/// Отказ получателя закрывает всю очередь сущности (`refusalEndsOutboxQueue`):
/// не принятое создание делает бессмысленными правки и удаление.
#[must_use]
pub fn refusal_ends_queue(event_type: &str) -> bool {
    event_type == "create"
}

/// Непустая строка по пути в значении (`payloadString`).
fn text_at<'a>(payload: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut node = payload;
    for key in path {
        node = node.get(key)?;
    }
    node.as_str().filter(|text| !text.is_empty())
}

/// Ключ очереди события (`outboxQueueKey`).
#[must_use]
pub fn queue_key(event_type: &str, entity_id: &str, context_id: &str, payload: &Value) -> String {
    match event_type {
        "create" | "update" | "delete" => format!("message:{entity_id}"),
        "reaction_add" | "reaction_remove" => {
            let at = |key| text_at(payload, &["reaction", key]);
            match (
                at("messageId"),
                at("messageHomeInstance"),
                at("userId"),
                at("emoji"),
            ) {
                (Some(message), Some(home), Some(user), Some(emoji)) => {
                    format!("reaction:{home}:{message}:{user}:{emoji}")
                }
                _ => format!("reaction:{entity_id}"),
            }
        }
        "presence_update" => format!("presence:{entity_id}"),
        "profile_update" => format!("profile:{entity_id}"),
        "read_state_update" => match (
            text_at(payload, &["federatedId"]),
            text_at(payload, &["readState", "user", "homeUserId"]),
            text_at(payload, &["readState", "user", "homeInstance"]),
        ) {
            (Some(federated), Some(user), Some(home)) => {
                format!("read_state:{federated}:{home}:{user}")
            }
            _ => format!("read_state:{entity_id}"),
        },
        "dm_close" | "dm_reopen" => match (
            text_at(payload, &["federatedId"]),
            text_at(payload, &["dmCloseReopen", "homeUserId"]),
            text_at(payload, &["dmCloseReopen", "homeInstance"]),
        ) {
            (Some(federated), Some(user), Some(home)) => {
                format!("dm_open:{federated}:{home}:{user}")
            }
            _ => format!("dm_open:{entity_id}"),
        },
        "file_rejected" => format!(
            "file_rejected:{entity_id}:{}",
            text_at(payload, &["attachmentId"]).unwrap_or_default()
        ),
        "member_add" | "member_remove" | "ownership_transfer" | "group_metadata_update" => {
            format!(
                "group:{}",
                text_at(payload, &["federatedId"]).unwrap_or(context_id)
            )
        }
        _ => format!("friendship:{context_id}"),
    }
}

/// Строка очереди для решения о записи.
struct Queued {
    id: String,
    event_type: String,
    offered: bool,
}

/// Что сделать с очередью при записи нового события (`QueuePlan`).
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// Добавить, убрав перечисленные строки.
    Append(Vec<String>),
    /// Заменить содержимое неотправленного создания.
    Absorb(String),
    /// Убрать перечисленные строки и ничего не добавлять.
    Cancel(Vec<String>),
}

fn plan(event_type: &str, queued: &[Queued]) -> Plan {
    let all = || queued.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
    match family(event_type) {
        Some(Family::State) => Plan::Append(all()),
        Some(Family::Message) => {
            if event_type == "delete" {
                let unoffered_create = queued
                    .iter()
                    .any(|row| row.event_type == "create" && !row.offered);
                return if unoffered_create {
                    Plan::Cancel(all())
                } else {
                    Plan::Append(all())
                };
            }
            if event_type == "update"
                && let Some(tail) = queued.last()
            {
                if tail.event_type == "create" && !tail.offered {
                    return Plan::Absorb(tail.id.clone());
                }
                if tail.event_type == "update" {
                    return Plan::Append(vec![tail.id.clone()]);
                }
            }
            Plan::Append(Vec::new())
        }
        _ => Plan::Append(Vec::new()),
    }
}

/// Событие для одного пира.
struct Entry<'a> {
    peer_id: &'a str,
    context_id: &'a str,
    context_type: &'a str,
    entity_id: &'a str,
    event_type: &'a str,
    payload: &'a Value,
    expires_at: i64,
}

/// Записывает событие в очередь пира (`writeOutboxEvent`).
fn write(tx: &mut dyn Tx, ids: &Snowflake, entry: &Entry<'_>, now: i64) -> DbResult<()> {
    let key = queue_key(
        entry.event_type,
        entry.entity_id,
        entry.context_id,
        entry.payload,
    );
    let rows = tx.query(
        "SELECT id, event_type, offered_at, created_at FROM federation_outbox \
         WHERE peer_id = ? AND queue_key = ? ORDER BY created_at ASC, id ASC",
        params![entry.peer_id, &key],
    )?;
    let mut queued = Vec::new();
    let mut latest = 0;
    for row in &rows {
        latest = latest.max(row.int("created_at")?);
        queued.push(Queued {
            id: row.text("id")?,
            event_type: row.text("event_type")?,
            offered: row.opt_int("offered_at")?.is_some(),
        });
    }
    let text = crate::jsjson::stringify(entry.payload);
    let remove = match plan(entry.event_type, &queued) {
        Plan::Absorb(id) => {
            tx.execute(
                "UPDATE federation_outbox SET payload = ? WHERE id = ?",
                params![text, id],
            )?;
            return Ok(());
        }
        Plan::Cancel(remove) => {
            for id in remove {
                tx.execute("DELETE FROM federation_outbox WHERE id = ?", params![id])?;
            }
            return Ok(());
        }
        Plan::Append(remove) => remove,
    };
    for id in remove {
        tx.execute("DELETE FROM federation_outbox WHERE id = ?", params![id])?;
    }
    tx.execute(
        "INSERT INTO federation_outbox (id, peer_id, context_id, entity_id, queue_key, context_type, \
         event_type, payload, encryption_version, attempts, next_retry_at, expires_at, created_at, \
         offered_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?, ?, ?, NULL)",
        params![
            ids.next(),
            entry.peer_id,
            entry.context_id,
            entry.entity_id,
            key,
            entry.context_type,
            entry.event_type,
            text,
            now,
            entry.expires_at,
            now.max(latest + 1)
        ],
    )?;
    Ok(())
}

/// Срок жизни события в очереди, дни (`getRelayTtlDays`).
fn ttl_days(tx: &mut dyn Tx) -> DbResult<i64> {
    Ok(tx
        .query_one(
            "SELECT federation_relay_ttl_days FROM instance_settings WHERE id = 1",
            params![],
        )?
        .map(|row| row.opt_int("federation_relay_ttl_days"))
        .transpose()?
        .flatten()
        .unwrap_or(30))
}

/// Пир-заготовка для инстанса, о котором мы ещё не знаем
/// (`createAutoPlaceholderPeer`): рукопожатие с ним начнёт рабочий.
fn placeholder(tx: &mut dyn Tx, ids: &Snowflake, origin: &str) -> DbResult<Option<String>> {
    let inbound = tx.query_one(
        "SELECT id FROM peer_approval_requests WHERE origin = ? AND direction = 'inbound'",
        params![origin],
    )?;
    if inbound.is_some() {
        tracing::warn!(
            origin,
            "заготовка пира не создана: ждёт решения входящий запрос на пиринг"
        );
        return Ok(None);
    }
    if !handshake::settings(tx)?.1 {
        tracing::warn!(
            origin,
            "заготовка пира не создана: исходящий пиринг требует одобрения администратора"
        );
        return Ok(None);
    }
    let new = NewPeer {
        origin: origin.to_owned(),
        secret: crate::federation_auth::generate_secret(),
        initiated_by: "auto",
        status: "pending",
        ..NewPeer::default()
    };
    if let Some((peer, _)) = peers::insert(tx, ids, &new)? {
        PEERS_CHANGED.store(true, Ordering::Relaxed);
        return Ok(Some(peer.id));
    }
    Ok(peers::by_origin(tx, origin)?.map(|peer| peer.id))
}

/// Событие для других инстансов (`queueOutboxEvent`).
#[derive(Debug, Clone, Copy)]
pub struct Event<'a> {
    /// Под каким идентификатором событие знает получатель.
    pub entity_id: &'a str,
    /// Беседа, к которой оно относится.
    pub context_id: &'a str,
    /// Вид события.
    pub event_type: &'a str,
    /// Содержимое.
    pub payload: &'a Value,
    /// Кому: адреса инстансов; `None` — всем действующим пирам.
    pub targets: Option<&'a [String]>,
    /// Вид беседы: `dm`, `friend`, `profile`.
    pub context_type: &'a str,
}

/// Ставит событие в очереди пиров (`queueOutboxEvent`).
///
/// # Errors
/// Ошибка базы.
pub fn queue(tx: &mut dyn Tx, ids: &Snowflake, event: &Event<'_>) -> DbResult<()> {
    if !super::relay_enabled(tx)? {
        return Ok(());
    }
    if event.context_type == "dm" && event.targets.is_none() {
        tracing::error!(
            event = event.event_type,
            entity = event.entity_id,
            "событие переписки без адресатов не рассылается всем пирам"
        );
        return Ok(());
    }
    let statuses: &[&str] = if event.targets.is_some() {
        &["active", "pending", "unreachable"]
    } else {
        &["active", "unreachable"]
    };
    let mut matched: Vec<String> = Vec::new();
    for status in statuses {
        for peer in peers::with_status(tx, status)? {
            if event
                .targets
                .is_none_or(|targets| targets.contains(&peer.origin))
            {
                matched.push(peer.id);
            }
        }
    }
    if let Some(targets) = event.targets {
        for origin in targets {
            let known = peers::by_origin(tx, origin)?;
            match known {
                None => {
                    if let Some(id) = placeholder(tx, ids, origin)? {
                        tracing::info!(origin, "создана заготовка пира для события");
                        matched.push(id);
                    }
                }
                Some(peer) => {
                    if !matched.contains(&peer.id) {
                        tracing::debug!(
                            origin,
                            status = peer.status,
                            "событие пиру не ставится: журнал мутаций догонит его после активации"
                        );
                    }
                }
            }
        }
    }
    if matched.is_empty() {
        return Ok(());
    }
    let now = now_ms();
    let expires_at = now + ttl_days(tx)? * 86_400_000;
    // Порядок пиров как в оригинале — по порядку строк выборки.
    let mut seen = HashSet::new();
    for peer_id in matched {
        if !seen.insert(peer_id.clone()) {
            continue;
        }
        write(
            tx,
            ids,
            &Entry {
                peer_id: &peer_id,
                context_id: event.context_id,
                context_type: event.context_type,
                entity_id: event.entity_id,
                event_type: event.event_type,
                payload: event.payload,
                expires_at,
            },
            now,
        )?;
    }
    Ok(())
}

/// Удаляет из очередей события с истёкшим сроком; у сообщений — вместе со
/// всем, что стоит за ними в той же очереди (`expireOutboxQueues`).
///
/// # Errors
/// Ошибка базы.
pub fn expire(tx: &mut dyn Tx, now: i64) -> DbResult<u64> {
    tx.execute(
        "DELETE FROM federation_outbox WHERE id IN (\
           SELECT o.id FROM federation_outbox o WHERE EXISTS (\
             SELECT 1 FROM federation_outbox e WHERE e.expires_at < ? AND e.peer_id = o.peer_id \
             AND (e.id = o.id OR (e.queue_key = o.queue_key \
               AND o.event_type IN ('create', 'update', 'delete') \
               AND (e.created_at < o.created_at OR (e.created_at = o.created_at AND e.id < o.id))))))",
        params![now],
    )
}

/// Отмечает события, которые пир мог получить (`markOutboxOfferedForPeer`).
///
/// # Errors
/// Ошибка базы.
pub fn mark_offered_for_peer(
    tx: &mut dyn Tx,
    peer_id: &str,
    up_to: i64,
    context_type: &str,
) -> DbResult<()> {
    tx.execute(
        "UPDATE federation_outbox SET offered_at = coalesce(offered_at, ?) \
         WHERE peer_id = ? AND context_type = ? AND created_at <= ?",
        params![up_to, peer_id, context_type, up_to],
    )?;
    Ok(())
}

// ─── Доставка ───────────────────────────────────────────────────────────────

/// Чем кончился разбор отказа получателя (`classifyRejection`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Получатель событие уже имеет.
    Taken,
    /// Не примет никогда.
    Refused,
    /// Можно повторить позже.
    Retry,
}

const REFUSED: [&str; 29] = [
    "recipient_not_found",
    "attribution_mismatch",
    "unknown_event_type",
    "self_target_invalid",
    "not_message_author",
    "system_message_immutable",
    "invalid_system_message",
    "invalid_target",
    "source_peer_mismatch",
    "invalid_payload",
    "invalid_status",
    "missing_participants",
    "missing_federated_id",
    "missing_message_payload",
    "missing_reaction_payload",
    "missing_membership_payload",
    "missing_ownership_payload",
    "missing_metadata_payload",
    "missing_friendship_payload",
    "missing_file_rejected_payload",
    "missing_profile_update_payload",
    "missing_presence_update_payload",
    "missing_read_state_payload",
    "missing_dm_close_payload",
    "missing_dm_reopen_payload",
    "missing_call_payload",
    "missing_typing_payload",
    "message_not_found",
    "attachment_not_found",
];

const RETRY: [&str; 11] = [
    "channel_not_found",
    "participant_not_found",
    "author_not_found",
    "user_not_found",
    "sender_not_found",
    "actor_not_found",
    "attribution_unproven",
    "unknown_message",
    "unauthorized_source",
    "max_members_exceeded",
    "processing_error",
];

/// Разбор причины, если она известна (`classifyListed`).
#[must_use]
pub fn classify_listed(event_type: Option<&str>, reason: &str) -> Option<Verdict> {
    if reason == "duplicate" {
        return Some(Verdict::Taken);
    }
    if reason == "unknown_message" && matches!(event_type, Some("delete" | "reaction_remove")) {
        return Some(Verdict::Taken);
    }
    if REFUSED.contains(&reason) {
        return Some(Verdict::Refused);
    }
    if RETRY.contains(&reason) {
        return Some(Verdict::Retry);
    }
    None
}

/// Разбор отказа в ответе на доставку: неизвестная причина — повтор.
#[must_use]
pub fn classify(event_type: Option<&str>, reason: &str) -> Verdict {
    classify_listed(event_type, reason).unwrap_or(Verdict::Retry)
}

/// Пауза перед следующей попыткой (`getBackoffMs`).
fn backoff(config: &crate::config::Config, attempt: i64) -> i64 {
    let index = usize::try_from((attempt - 1).max(0))
        .unwrap_or(0)
        .min(BACKOFF_MS.len() - 1);
    let divisor = i64::from(config.federation_backoff_divisor.max(1));
    (BACKOFF_MS[index] + divisor - 1) / divisor
}

/// Строка очереди в пачке.
#[derive(Debug, Clone)]
struct Item {
    id: String,
    peer_id: String,
    context_id: String,
    entity_id: String,
    queue_key: Option<String>,
    context_type: String,
    event_type: String,
    payload: String,
    encryption_version: i64,
    attempts: i64,
    created_at: i64,
    origin: String,
    secret: String,
}

const ITEM_COLUMNS: &str = "o.id, o.peer_id, o.context_id, o.entity_id, o.queue_key, \
    o.context_type, o.event_type, o.payload, o.encryption_version, o.attempts, o.created_at, \
    p.origin, p.hmac_secret, p.pending_hmac_secret, p.secret_rotation_at";

impl Item {
    fn from_row(row: &Row) -> DbResult<Self> {
        let pending = row.opt_text("pending_hmac_secret")?;
        let rotating = row.opt_int("secret_rotation_at")?.is_some();
        Ok(Self {
            id: row.text("id")?,
            peer_id: row.text("peer_id")?,
            context_id: row.text("context_id")?,
            entity_id: row.text("entity_id")?,
            queue_key: row.opt_text("queue_key")?,
            context_type: row
                .opt_text("context_type")?
                .unwrap_or_else(|| "dm".to_owned()),
            event_type: row.text("event_type")?,
            payload: row.text("payload")?,
            encryption_version: row.opt_int("encryption_version")?.unwrap_or(0),
            attempts: row.opt_int("attempts")?.unwrap_or(0),
            created_at: row.int("created_at")?,
            origin: row.text("origin")?,
            // Во время смены секрета подписывается новым (`pendingHmacSecret`).
            secret: match pending {
                Some(pending) if rotating => pending,
                _ => row.text("hmac_secret")?,
            },
        })
    }
}

/// Поля содержимого, которые переносятся в событие (`buildRelayEvents`).
const CARRIED: [&str; 21] = [
    "federatedId",
    "participants",
    "message",
    "reactions",
    "reaction",
    "target",
    "membership",
    "ownership",
    "group",
    "friendship",
    "attachmentId",
    "sourceFilename",
    "rejectionReason",
    "rejectionLimit",
    "affectedUserIds",
    "affectedUsers",
    "metadata",
    "profileUpdate",
    "presenceUpdate",
    "readState",
    "dmCloseReopen",
];

/// Событие на проводе из строки очереди.
fn wire_event(item: &Item) -> Value {
    let payload: Value = serde_json::from_str(&item.payload).unwrap_or(Value::Null);
    let mut event = serde_json::Map::new();
    event.insert("eventType".to_owned(), json!(item.event_type));
    event.insert("contextType".to_owned(), json!(item.context_type));
    event.insert("messageId".to_owned(), json!(item.entity_id));
    event.insert(
        "encryptionVersion".to_owned(),
        json!(item.encryption_version),
    );
    event.insert("timestamp".to_owned(), json!(item.created_at));
    if item.context_type == "dm" && !item.context_id.is_empty() {
        event.insert("dmChannelId".to_owned(), json!(item.context_id));
    }
    for key in CARRIED {
        let Some(value) = payload.get(key) else {
            continue;
        };
        // Как `if (parsed.x)` в JavaScript: пропускаются «ложные» значения;
        // `rejectionLimit` — только `null`.
        let carried = if key == "rejectionLimit" {
            !value.is_null()
        } else {
            match value {
                Value::Null => false,
                Value::Bool(flag) => *flag,
                Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
                Value::String(text) => !text.is_empty(),
                _ => true,
            }
        };
        if carried {
            event.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(event)
}

/// Итог отправки пачки.
enum Delivery {
    Answered {
        accepted: HashSet<String>,
        rejected: Vec<(String, String)>,
        max_upload: Option<i64>,
    },
    AuthRefused(u16),
    Failed(String),
}

fn parse_answer(body: &[u8]) -> Option<Delivery> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let accepted = value.get("accepted")?.as_array()?;
    let accepted = accepted
        .iter()
        .map(|id| id.as_str().map(str::to_owned))
        .collect::<Option<HashSet<_>>>()?;
    let mut rejected = Vec::new();
    for entry in value.get("rejected")?.as_array()? {
        let id = entry.get("messageId")?.as_str()?;
        let reason = entry.get("reason")?.as_str()?;
        rejected.push((id.to_owned(), reason.to_owned()));
    }
    let max_upload = value
        .get("maxUploadSize")
        .and_then(Value::as_f64)
        .filter(|size| size.is_finite())
        .map(|size| {
            #[allow(clippy::cast_possible_truncation, reason = "размер файла")]
            let size = size as i64;
            size
        });
    Some(Delivery::Answered {
        accepted,
        rejected,
        max_upload,
    })
}

async fn send(state: &AppState, origin: &str, body: String, secret: &str) -> Delivery {
    let ours = super::our_origin(&state.config);
    let headers = crate::federation_auth::headers(&body, secret, &ours, now_ms());
    let sent = fetch::request(
        &state.config,
        origin,
        "/api/federation/relay",
        Some(body),
        &headers,
        Trust::Approved,
        DELIVERY_TIMEOUT,
    )
    .await;
    match sent {
        Ok(answer) if matches!(answer.status, 401 | 403) => Delivery::AuthRefused(answer.status),
        Ok(answer) if !answer.ok() => Delivery::Failed(format!("HTTP {}", answer.status)),
        Ok(answer) => parse_answer(&answer.body)
            .unwrap_or_else(|| Delivery::Failed("unreadable answer".to_owned())),
        Err(FetchError::Timeout) => Delivery::Failed("timeout".to_owned()),
        Err(FetchError::Failed(text)) => Delivery::Failed(text),
    }
}

fn apply_backoff(
    tx: &mut dyn Tx,
    config: &crate::config::Config,
    items: &[Item],
    now: i64,
) -> DbResult<()> {
    for item in items {
        let attempts = item.attempts + 1;
        tx.execute(
            "UPDATE federation_outbox SET attempts = ?, next_retry_at = ? WHERE id = ?",
            params![attempts, now + backoff(config, attempts), &item.id],
        )?;
    }
    Ok(())
}

/// Забирает строки пачки: по одной на сущность, отмечая их отправленными
/// (`takeOutboxBatch`).
fn take(tx: &mut dyn Tx, ids: &[String], now: i64) -> DbResult<Vec<Item>> {
    let marks = vec!["?"; ids.len()].join(", ");
    let values: Vec<Sql> = ids.iter().map(|id| Sql::from(id.as_str())).collect();
    let rows = tx.query(
        &format!(
            "SELECT {ITEM_COLUMNS} FROM federation_outbox o \
             INNER JOIN federation_peers p ON o.peer_id = p.id \
             WHERE o.id IN ({marks}) ORDER BY o.created_at ASC, o.id ASC"
        ),
        &values,
    )?;
    let mut seen = HashSet::new();
    let mut batch = Vec::new();
    for row in &rows {
        let item = Item::from_row(row)?;
        if seen.insert(item.entity_id.clone()) {
            batch.push(item);
        }
    }
    for item in &batch {
        tx.execute(
            "UPDATE federation_outbox SET offered_at = coalesce(offered_at, ?) WHERE id = ?",
            params![now, &item.id],
        )?;
    }
    Ok(batch)
}

/// Возвращает отклонённый получателем запрос дружбы (`rollbackFriendRequestCreate`).
fn rollback_friend_request(
    tx: &mut dyn Tx,
    out: &mut crate::hub::Outbox,
    message_id: &str,
    reason: &str,
) -> DbResult<()> {
    let Some(row) = tx.query_one(
        "SELECT id, from_id, to_id FROM friend_requests WHERE relay_message_id = ?",
        params![message_id],
    )?
    else {
        return Ok(());
    };
    let target = tx
        .query_one(
            "SELECT username FROM users WHERE id = ?",
            params![row.text("to_id")?],
        )?
        .map(|user| user.text("username"))
        .transpose()?
        .unwrap_or_else(|| "unknown".to_owned());
    tx.execute(
        "DELETE FROM friend_requests WHERE id = ?",
        params![row.text("id")?],
    )?;
    let (kind, message) = if reason == "recipient_not_found" {
        (
            "user_not_found",
            format!("User {target} no longer exists on the remote instance."),
        )
    } else {
        (
            "peer_rejected",
            format!("Friend request to {target} was rejected by the remote instance."),
        )
    };
    out.to_user(
        &row.text("from_id")?,
        &json!({
            "type": "friend_request_relay_failed",
            "requestId": row.text("id")?,
            "reason": kind,
            "message": message,
            "targetHandle": target,
        }),
    );
    Ok(())
}

/// Разбирает ответ получателя (`settleAnsweredBatch`).
async fn settle(
    state: &AppState,
    batch: Vec<Item>,
    accepted: HashSet<String>,
    rejected: Vec<(String, String)>,
    max_upload: Option<i64>,
) {
    let app = state.clone();
    let origin = batch
        .first()
        .map(|item| item.origin.clone())
        .unwrap_or_default();
    let peer_id = batch
        .first()
        .map(|item| item.peer_id.clone())
        .unwrap_or_default();
    let done = state
        .run(move |tx, out| {
            let reasons: HashMap<&str, &str> = rejected
                .iter()
                .map(|(id, reason)| (id.as_str(), reason.as_str()))
                .collect();
            let now = now_ms();
            let mut retry = Vec::new();
            let mut refusals = Vec::new();
            for item in &batch {
                let reason = reasons.get(item.entity_id.as_str()).copied();
                let verdict = if accepted.contains(&item.entity_id) {
                    Verdict::Taken
                } else {
                    reason.map_or(Verdict::Retry, |reason| {
                        classify(Some(&item.event_type), reason)
                    })
                };
                if verdict == Verdict::Retry {
                    retry.push(item.clone());
                    continue;
                }
                tx.execute(
                    "DELETE FROM federation_outbox WHERE id = ?",
                    params![&item.id],
                )?;
                if let (Verdict::Refused, Some(reason)) = (verdict, reason) {
                    refusals.push((item.entity_id.clone(), reason, item.event_type.clone()));
                    if refusal_ends_queue(&item.event_type)
                        && let Some(key) = &item.queue_key
                    {
                        tx.execute(
                            "DELETE FROM federation_outbox WHERE peer_id = ? AND queue_key = ?",
                            params![&peer_id, key],
                        )?;
                    }
                }
            }
            apply_backoff(tx, &app.config, &retry, now)?;
            for (message_id, reason, event_type) in refusals {
                if event_type == "friend_request_create" {
                    rollback_friend_request(tx, out, &message_id, reason)?;
                    tx.execute(
                        "DELETE FROM federation_mutation_log WHERE entity_id = ? AND mutation_type = ?",
                        params![&message_id, &event_type],
                    )?;
                }
            }
            for (id, reason) in &rejected {
                let event_type = batch
                    .iter()
                    .find(|item| item.entity_id == *id)
                    .map(|item| item.event_type.as_str());
                if classify(event_type, reason) == Verdict::Retry {
                    tracing::warn!(origin, message = id, reason, "пир отклонил событие");
                } else {
                    tracing::info!(origin, message = id, reason, "пир окончательно отклонил событие; строка очереди удалена");
                }
            }
            let mut sets = vec![
                ("last_seen_at", Sql::from(now)),
                ("consecutive_failures", Sql::from(0_i64)),
                ("consecutive_auth_failures", Sql::from(0_i64)),
            ];
            if let Some(size) = max_upload {
                sets.push(("remote_max_upload_size", Sql::from(size)));
            }
            let assignments = sets
                .iter()
                .map(|(column, _)| format!("{column} = ?"))
                .collect::<Vec<_>>()
                .join(", ");
            let mut values: Vec<Sql> = sets.into_iter().map(|(_, value)| value).collect();
            values.push(Sql::from(peer_id.as_str()));
            tx.execute(
                &format!("UPDATE federation_peers SET {assignments} WHERE id = ?"),
                &values,
            )?;
            Ok(())
        })
        .await;
    if let Err(error) = done {
        tracing::error!(?error, "не удалось записать итог доставки");
    }
}

/// Отказ в подписи (`handleRelayAuthRefusal`): после пяти подряд пир требует
/// внимания, и проверяется, не переустановлен ли он.
async fn auth_refused(state: &AppState, batch: &[Item], status: u16) {
    let Some(first) = batch.first() else { return };
    let (peer_id, origin) = (first.peer_id.clone(), first.origin.clone());
    let app = state.clone();
    let items = batch.to_vec();
    let id = peer_id.clone();
    let counted = state
        .run(move |tx, _| {
            let now = now_ms();
            apply_backoff(tx, &app.config, &items, now)?;
            let row = tx.query_one(
                "SELECT consecutive_auth_failures, peer_instance_id FROM federation_peers WHERE id = ?",
                params![&id],
            )?;
            let failures = row
                .as_ref()
                .map(|row| row.opt_int("consecutive_auth_failures"))
                .transpose()?
                .flatten()
                .unwrap_or(0)
                + 1;
            let epoch = row
                .map(|row| row.opt_text("peer_instance_id"))
                .transpose()?
                .flatten();
            if failures < AUTH_FAILURE_THRESHOLD {
                tx.execute(
                    "UPDATE federation_peers SET consecutive_auth_failures = ?, last_failure_at = ? \
                     WHERE id = ?",
                    params![failures, now, &id],
                )?;
            }
            Ok((failures, epoch, now))
        })
        .await;
    let Ok((failures, epoch, now)) = counted else {
        return;
    };
    if failures < AUTH_FAILURE_THRESHOLD {
        tracing::warn!(origin, status, failures, "пир отказал в подписи");
        return;
    }
    let change = Transition::new(&["active"], "needs_attention", "auth_threshold")
        .reason("auth_failures")
        .set("consecutive_auth_failures", failures)
        .set("last_failure_at", now);
    if peers::transition(state, &peer_id, change)
        .await
        .is_ok_and(|moved| moved.applied())
    {
        tracing::warn!(
            origin,
            failures,
            status,
            "пир требует внимания после отказов в подписи"
        );
        if let Some(known) = epoch {
            let (reachable, observed) = fetch::probe_reachable(&state.config, &origin).await;
            if let (true, Some(observed)) = (reachable, observed)
                && observed != known
            {
                super::reset::mark_and_notify(state, &peer_id, &origin, &known, &observed).await;
            }
        }
    }
}

/// Сетевая неудача (`handleOutboxDeliveryFailure`): после десяти подряд пир
/// считается недоступным.
async fn failed(state: &AppState, batch: &[Item]) {
    let Some(first) = batch.first() else { return };
    let peer_id = first.peer_id.clone();
    let app = state.clone();
    let items = batch.to_vec();
    let id = peer_id.clone();
    let failures = state
        .run(move |tx, _| {
            let now = now_ms();
            apply_backoff(tx, &app.config, &items, now)?;
            let failures = tx
                .query_one(
                    "SELECT consecutive_failures FROM federation_peers WHERE id = ?",
                    params![&id],
                )?
                .map(|row| row.opt_int("consecutive_failures"))
                .transpose()?
                .flatten()
                .unwrap_or(0)
                + 1;
            tx.execute(
                "UPDATE federation_peers SET last_failure_at = ?, consecutive_failures = ? WHERE id = ?",
                params![now, failures, &id],
            )?;
            Ok(failures)
        })
        .await
        .unwrap_or(0);
    if failures >= UNREACHABLE_THRESHOLD {
        let change = Transition::new(&["active"], "unreachable", "network_threshold");
        if peers::transition(state, &peer_id, change)
            .await
            .is_ok_and(|moved| moved.applied())
        {
            tracing::warn!(
                peer = peer_id,
                failures,
                "пир недоступен после сетевых неудач"
            );
        }
    }
}

/// Рукопожатия, начатые рабочим ради заготовок пиров.
static PENDING_HANDSHAKES: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Заготовки пиров, которым есть что отправить, получают рукопожатие
/// (`resolvePendingPeers`).
async fn resolve_pending(state: &AppState, now: i64) {
    let divisor = state.config.federation_backoff_divisor;
    let due = state
        .run(move |tx, _| {
            let rows = tx.query(
                &format!(
                    "SELECT DISTINCT {} FROM federation_peers p \
                     INNER JOIN federation_outbox o ON o.peer_id = p.id \
                     WHERE p.status = 'pending' AND NOT EXISTS (SELECT 1 FROM peer_approval_requests r \
                     WHERE r.origin = p.origin AND r.direction = 'inbound')",
                    peers::COLUMNS
                        .split(", ")
                        .map(|column| format!("p.{column}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                params![],
            )?;
            let mut due = Vec::new();
            for row in rows {
                let peer = peers::Peer::from_row(&row)?;
                if super::worker::attempt_due(&peer, true, now, divisor) {
                    due.push(peer.origin);
                }
            }
            Ok(due)
        })
        .await
        .unwrap_or_default();
    for origin in due {
        {
            let mut running = PENDING_HANDSHAKES
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let running = running.get_or_insert_with(HashSet::new);
            if !running.insert(origin.clone()) {
                continue;
            }
        }
        let app = state.clone();
        tokio::spawn(async move {
            if !app.federation.in_flight(&origin) {
                tracing::info!(origin, "рукопожатие ради очереди событий");
                match handshake::ensure_peered(&app, &origin, Intent::System).await {
                    Ensured::Active(_) => {
                        tracing::info!(
                            origin,
                            "пиринг установлен; события уйдут в следующий проход"
                        );
                    }
                    Ensured::Rejected(error) => {
                        tracing::warn!(origin, error, "пиринг для очереди событий отклонён");
                    }
                    Ensured::Failed(error) => {
                        tracing::warn!(origin, error, "рукопожатие для очереди событий не удалось");
                    }
                    Ensured::Pending(_) | Ensured::AdminRequired(_) => {}
                }
            }
            PENDING_HANDSHAKES
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_or_insert_with(HashSet::new)
                .remove(&origin);
        });
    }
}

/// Один проход доставки (`processOutboxTick`).
pub async fn tick(state: &AppState) {
    if PEERS_CHANGED.swap(false, Ordering::Relaxed) {
        let app = state.clone();
        let _ = state
            .run(move |tx, out| {
                Ok(peers::to_admins(
                    tx,
                    &app.hub,
                    out,
                    &json!({"type": "federation_peers_changed"}),
                )?)
            })
            .await;
    }
    let enabled = state
        .run(|tx, _| Ok(super::relay_enabled(tx)?))
        .await
        .unwrap_or(false);
    if !enabled {
        return;
    }
    let now = now_ms();
    resolve_pending(state, now).await;
    // Головы очередей: строки, раньше которых в той же очереди ничего нет.
    let selected = state
        .run(move |tx, _| {
            let rows = tx.query(
                "SELECT o.id, o.peer_id FROM federation_outbox o \
                 INNER JOIN federation_peers p ON o.peer_id = p.id \
                 WHERE o.next_retry_at <= ? AND p.status = 'active' AND (o.queue_key IS NULL OR NOT EXISTS (\
                   SELECT 1 FROM federation_outbox e WHERE e.peer_id = o.peer_id AND e.queue_key = o.queue_key \
                   AND e.created_at <= o.created_at \
                   AND (e.created_at < o.created_at OR (e.created_at = o.created_at AND e.id < o.id)))) \
                 ORDER BY o.created_at ASC, o.id ASC LIMIT ?",
                params![now, BATCH_LIMIT],
            )?;
            let mut by_peer: BTreeMap<usize, (String, Vec<String>)> = BTreeMap::new();
            let mut order: HashMap<String, usize> = HashMap::new();
            for row in rows {
                let peer = row.text("peer_id")?;
                let next = order.len();
                let slot = *order.entry(peer.clone()).or_insert(next);
                by_peer
                    .entry(slot)
                    .or_insert_with(|| (peer, Vec::new()))
                    .1
                    .push(row.text("id")?);
            }
            Ok(by_peer.into_values().collect::<Vec<_>>())
        })
        .await
        .unwrap_or_default();
    for (_, ids) in selected {
        let batch = state
            .run(move |tx, _| Ok(take(tx, &ids, now_ms())?))
            .await
            .unwrap_or_default();
        let Some(first) = batch.first() else {
            continue;
        };
        let (origin, secret) = (first.origin.clone(), first.secret.clone());
        let request = json!({
            "version": 1,
            "sourceInstance": super::our_origin(&state.config),
            "sourceInstanceId": &*state.instance_id,
            "capabilities": CAPABILITIES,
            "events": batch.iter().map(wire_event).collect::<Vec<_>>(),
        });
        let body = crate::jsjson::stringify(&request);
        match send(state, &origin, body, &secret).await {
            Delivery::Answered {
                accepted,
                rejected,
                max_upload,
            } => settle(state, batch, accepted, rejected, max_upload).await,
            Delivery::AuthRefused(status) => auth_refused(state, &batch, status).await,
            Delivery::Failed(reason) => {
                tracing::error!(origin, reason, "доставка пиру не удалась");
                failed(state, &batch).await;
            }
        }
    }
}

/// Запускает рабочего доставки.
pub fn spawn(state: &AppState) {
    let app = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(TICK).await;
            tick(&app).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, event: &str, offered: bool) -> Queued {
        Queued {
            id: id.into(),
            event_type: event.into(),
            offered,
        }
    }

    #[test]
    fn queue_keys_follow_the_original() {
        let reaction = json!({"reaction": {"messageId": "m", "messageHomeInstance": "https://a", "userId": "u", "emoji": "x"}});
        assert_eq!(
            queue_key("reaction_add", "r1", "c", &reaction),
            "reaction:https://a:m:u:x"
        );
        assert_eq!(
            queue_key("reaction_remove", "r1", "c", &json!({})),
            "reaction:r1"
        );
        assert_eq!(queue_key("create", "m1", "c", &json!({})), "message:m1");
        assert_eq!(
            queue_key("member_add", "e", "c", &json!({"federatedId": "f"})),
            "group:f"
        );
        assert_eq!(queue_key("member_remove", "e", "c", &json!({})), "group:c");
        assert_eq!(
            queue_key("friend_add", "e", "friend:a:b", &json!({})),
            "friendship:friend:a:b"
        );
        assert_eq!(
            queue_key("file_rejected", "e", "c", &json!({})),
            "file_rejected:e:"
        );
        let read = json!({"federatedId": "f", "readState": {"user": {"homeUserId": "u", "homeInstance": "h"}}});
        assert_eq!(
            queue_key("read_state_update", "e", "c", &read),
            "read_state:f:h:u"
        );
        let close =
            json!({"federatedId": "f", "dmCloseReopen": {"homeUserId": "u", "homeInstance": "h"}});
        assert_eq!(queue_key("dm_reopen", "e", "c", &close), "dm_open:f:h:u");
    }

    #[test]
    fn queue_plans_fold_like_the_original() {
        assert_eq!(
            plan("presence_update", &[row("a", "presence_update", true)]),
            Plan::Append(vec!["a".into()])
        );
        assert_eq!(
            plan("update", &[row("a", "create", false)]),
            Plan::Absorb("a".into())
        );
        assert_eq!(
            plan("update", &[row("a", "create", true)]),
            Plan::Append(vec![])
        );
        assert_eq!(
            plan(
                "update",
                &[row("a", "create", true), row("b", "update", false)]
            ),
            Plan::Append(vec!["b".into()])
        );
        assert_eq!(
            plan(
                "delete",
                &[row("a", "create", false), row("b", "update", false)]
            ),
            Plan::Cancel(vec!["a".into(), "b".into()])
        );
        assert_eq!(
            plan("delete", &[row("a", "create", true)]),
            Plan::Append(vec!["a".into()])
        );
        assert_eq!(plan("delete", &[]), Plan::Append(vec![]));
        assert_eq!(
            plan("member_add", &[row("a", "member_add", false)]),
            Plan::Append(vec![])
        );
    }

    #[test]
    fn rejections_are_classified() {
        assert_eq!(classify(Some("create"), "duplicate"), Verdict::Taken);
        assert_eq!(classify(Some("delete"), "unknown_message"), Verdict::Taken);
        assert_eq!(classify(Some("update"), "unknown_message"), Verdict::Retry);
        assert_eq!(
            classify(Some("create"), "recipient_not_found"),
            Verdict::Refused
        );
        assert_eq!(classify(Some("create"), "something_new"), Verdict::Retry);
        assert_eq!(classify_listed(None, "something_new"), None);
    }

    #[test]
    fn wire_events_carry_truthy_fields_only() {
        let item = Item {
            id: "o".into(),
            peer_id: "p".into(),
            context_id: "c".into(),
            entity_id: "e".into(),
            queue_key: None,
            context_type: "dm".into(),
            event_type: "create".into(),
            payload: r#"{"federatedId":"","message":{"a":1},"rejectionLimit":0,"deleted":true,"participants":[]}"#.into(),
            encryption_version: 0,
            attempts: 0,
            created_at: 5,
            origin: "o".into(),
            secret: "s".into(),
        };
        let event = wire_event(&item);
        assert_eq!(
            event,
            json!({"eventType": "create", "contextType": "dm", "messageId": "e", "encryptionVersion": 0,
                   "timestamp": 5, "dmChannelId": "c", "message": {"a": 1}, "rejectionLimit": 0, "participants": []})
        );
    }
}
