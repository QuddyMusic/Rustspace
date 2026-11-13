//! Состояние пира (`utils/federationPeerState.ts`).
//!
//! Статус пира меняется только здесь: переход проверяет, что пир всё ещё в
//! ожидаемом статусе (и с ожидаемым секретом), и возвращает описание
//! последствий — их выполняет [`run_effects`] уже после записи.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::db::{DbResult, Row, Tx, Value as Sql};
use crate::hub::{Hub, Outbox};
use crate::params;
use crate::snowflake::Snowflake;
use crate::state::{AppState, now_ms};

/// Все статусы пира.
pub const STATUSES: [&str; 7] = [
    "pending",
    "awaiting_approval",
    "active",
    "unreachable",
    "needs_attention",
    "rejected",
    "revoked",
];

/// Строка пира.
#[derive(Debug, Clone)]
pub struct Peer {
    /// Идентификатор.
    pub id: String,
    /// Адрес инстанса.
    pub origin: String,
    /// Статус; неизвестное значение читается как `needs_attention`.
    pub status: &'static str,
    /// Причина статуса.
    pub reason: Option<String>,
    /// Общий секрет.
    pub secret: String,
    /// Новый секрет на время смены.
    pub pending_secret: Option<String>,
    /// Когда началась смена секрета.
    pub rotation_at: Option<i64>,
    /// Кто начал пиринг: `admin`, `auto` или `remote`.
    pub initiated_by: String,
    /// Название инстанса.
    pub instance_name: Option<String>,
    /// Эпоха пира.
    pub epoch: Option<String>,
    /// Токен одобрения, выданный удалённой стороной.
    pub approval_token: Option<String>,
    /// Присылал ли пир одноразовые числа.
    pub nonce_supported: bool,
    /// Число попыток связаться подряд.
    pub probe_attempts: i64,
    /// Когда была последняя попытка.
    pub last_probe_at: Option<i64>,
    /// Вид для клиента администратора (`sanitizePeer`).
    pub public: Value,
}

/// Столбцы строки пира.
pub const COLUMNS: &str = "id, origin, instance_name, hmac_secret, status, initiated_by, \
    last_seen_at, last_failure_at, consecutive_failures, consecutive_auth_failures, last_probe_at, \
    probe_attempts, last_synced_at, nonce_supported, pending_hmac_secret, secret_rotation_at, \
    secret_rotated_at, auto_rotate_interval_days, created_at, approval_token, peer_instance_id, \
    observed_peer_instance_id, status_reason";

fn status_of(text: &str) -> &'static str {
    STATUSES
        .iter()
        .find(|known| **known == text)
        .copied()
        .unwrap_or("needs_attention")
}

impl Peer {
    /// Разбирает строку таблицы `federation_peers`.
    ///
    /// # Errors
    /// В строке нет ожидаемого столбца.
    pub fn from_row(row: &Row) -> DbResult<Self> {
        let pending_secret = row.opt_text("pending_hmac_secret")?;
        let reason = row.opt_text("status_reason")?;
        let raw_status = row.text("status")?;
        let public = json!({
            "id": row.text("id")?,
            "origin": row.text("origin")?,
            "instanceName": row.opt_text("instance_name")?,
            "status": raw_status,
            "lastSeenAt": row.opt_int("last_seen_at")?,
            "lastFailureAt": row.opt_int("last_failure_at")?,
            "consecutiveFailures": row.int("consecutive_failures")?,
            "lastSyncedAt": row.opt_int("last_synced_at")?,
            "createdAt": row.int("created_at")?,
            "rotationInProgress": pending_secret.is_some(),
            "secretRotatedAt": row.opt_int("secret_rotated_at")?,
            "autoRotateIntervalDays": row.int("auto_rotate_interval_days")?,
            "statusReason": reason,
        });
        Ok(Self {
            id: row.text("id")?,
            origin: row.text("origin")?,
            status: status_of(&raw_status),
            reason,
            secret: row.text("hmac_secret")?,
            pending_secret,
            rotation_at: row.opt_int("secret_rotation_at")?,
            initiated_by: row.text("initiated_by")?,
            instance_name: row.opt_text("instance_name")?,
            epoch: row.opt_text("peer_instance_id")?,
            approval_token: row.opt_text("approval_token")?,
            nonce_supported: row.opt_flag("nonce_supported")? == Some(true),
            probe_attempts: row.int("probe_attempts")?,
            last_probe_at: row.opt_int("last_probe_at")?,
            public,
        })
    }

    /// Секреты для проверки подписи.
    #[must_use]
    pub fn secrets(&self) -> crate::federation_auth::PeerSecrets<'_> {
        crate::federation_auth::PeerSecrets {
            current: &self.secret,
            pending: self.pending_secret.as_deref(),
            rotation_at: self.rotation_at,
        }
    }
}

/// Пир по идентификатору.
///
/// # Errors
/// Ошибка базы.
pub fn by_id(tx: &mut dyn Tx, id: &str) -> DbResult<Option<Peer>> {
    tx.query_one(
        &format!("SELECT {COLUMNS} FROM federation_peers WHERE id = ?"),
        params![id],
    )?
    .map(|row| Peer::from_row(&row))
    .transpose()
}

/// Пир по адресу.
///
/// # Errors
/// Ошибка базы.
pub fn by_origin(tx: &mut dyn Tx, origin: &str) -> DbResult<Option<Peer>> {
    tx.query_one(
        &format!("SELECT {COLUMNS} FROM federation_peers WHERE origin = ?"),
        params![origin],
    )?
    .map(|row| Peer::from_row(&row))
    .transpose()
}

/// Все пиры с данным статусом.
///
/// # Errors
/// Ошибка базы.
pub fn with_status(tx: &mut dyn Tx, status: &str) -> DbResult<Vec<Peer>> {
    tx.query(
        &format!("SELECT {COLUMNS} FROM federation_peers WHERE status = ?"),
        params![status],
    )?
    .iter()
    .map(Peer::from_row)
    .collect()
}

/// Переход пира в новый статус.
#[derive(Debug, Clone, Default)]
pub struct Transition {
    /// Из каких статусов переход допустим.
    pub from: Vec<&'static str>,
    /// Секрет, который должен быть у пира (защита от гонки рукопожатий).
    pub expect_secret: Option<String>,
    /// Новый статус.
    pub to: &'static str,
    /// Причина: только для `needs_attention` и `rejected`.
    pub reason: Option<&'static str>,
    /// Что вызвало переход (причина активации или выхода).
    pub cause: &'static str,
    /// Столбцы, которые меняются вместе со статусом.
    pub fields: Vec<(&'static str, Sql)>,
}

impl Transition {
    /// Переход в статус `to` из перечисленных статусов.
    #[must_use]
    pub fn new(from: &[&'static str], to: &'static str, cause: &'static str) -> Self {
        Self {
            from: from.to_vec(),
            to,
            cause,
            ..Self::default()
        }
    }

    /// С причиной статуса.
    #[must_use]
    pub const fn reason(mut self, reason: &'static str) -> Self {
        self.reason = Some(reason);
        self
    }

    /// Только если секрет пира не сменился.
    #[must_use]
    pub fn expect(mut self, secret: &str) -> Self {
        self.expect_secret = Some(secret.to_owned());
        self
    }

    /// Со значением столбца.
    #[must_use]
    pub fn set(mut self, column: &'static str, value: impl Into<Sql>) -> Self {
        self.fields.push((column, value.into()));
        self
    }
}

/// Последствия перехода, выполняемые после записи.
#[derive(Debug, Clone, Default)]
pub struct Effects {
    /// Пир.
    pub peer_id: String,
    /// Его адрес.
    pub origin: String,
    /// Прежний статус; `None` — строка только что создана.
    pub previous: Option<&'static str>,
    /// Новый статус; `None` — строка удалена.
    pub next: Option<&'static str>,
    /// Причина нового статуса.
    pub reason: Option<&'static str>,
    /// Причина активации.
    pub activation: Option<&'static str>,
    /// Причина выхода из активного статуса.
    pub exit: Option<&'static str>,
    /// Контексты, чьи события пиру уже не уйдут: идентификатор → вид.
    pub refused: Option<BTreeMap<String, String>>,
}

/// Исход перехода.
#[derive(Debug, Clone)]
pub enum Moved {
    /// Переход записан.
    Applied {
        /// Прежний статус.
        previous: &'static str,
        /// Что сделать после записи.
        effects: Box<Effects>,
    },
    /// Пир не в том состоянии; его текущее состояние, если он есть.
    Refused(Option<Box<Peer>>),
}

impl Moved {
    /// Записан ли переход.
    #[must_use]
    pub const fn applied(&self) -> bool {
        matches!(self, Self::Applied { .. })
    }
}

fn queued_contexts(tx: &mut dyn Tx, peer_id: &str) -> DbResult<BTreeMap<String, String>> {
    let mut contexts = BTreeMap::new();
    for row in tx.query(
        "SELECT context_id, context_type FROM federation_outbox WHERE peer_id = ?",
        params![peer_id],
    )? {
        contexts
            .entry(row.text("context_id")?)
            .or_insert(row.text("context_type")?);
    }
    Ok(contexts)
}

/// Переход пира (`applyPeerTransition`).
///
/// # Errors
/// Ошибка базы.
pub fn apply(tx: &mut dyn Tx, peer_id: &str, change: &Transition) -> DbResult<Moved> {
    let Some(before) = by_id(tx, peer_id)? else {
        return Ok(Moved::Refused(None));
    };
    if !change.from.contains(&before.status)
        || change
            .expect_secret
            .as_ref()
            .is_some_and(|secret| *secret != before.secret)
    {
        return Ok(Moved::Refused(Some(Box::new(before))));
    }
    let reason = if matches!(change.to, "needs_attention" | "rejected") {
        change.reason
    } else {
        None
    };
    let mut assignments = Vec::new();
    let mut values: Vec<Sql> = Vec::new();
    for (column, value) in &change.fields {
        assignments.push(format!("{column} = ?"));
        values.push(value.clone());
    }
    // Смена статуса начинает отсчёт попыток заново.
    if before.status != change.to && matches!(change.to, "pending" | "unreachable" | "active") {
        assignments.push("probe_attempts = 0".to_owned());
        assignments.push("last_probe_at = NULL".to_owned());
    }
    assignments.push("status = ?".to_owned());
    values.push(change.to.into());
    assignments.push("status_reason = ?".to_owned());
    values.push(reason.into());
    values.push(peer_id.into());
    values.push(before.status.into());
    tx.execute(
        &format!(
            "UPDATE federation_peers SET {} WHERE id = ? AND status = ?",
            assignments.join(", ")
        ),
        &values,
    )?;
    let mut refused = None;
    let enters_refusal = before.status != change.to && matches!(change.to, "rejected" | "revoked");
    if enters_refusal {
        if change.to == "rejected" {
            refused = Some(queued_contexts(tx, peer_id)?);
        }
        tx.execute(
            "DELETE FROM federation_outbox WHERE peer_id = ?",
            params![peer_id],
        )?;
    } else if change.to == "needs_attention"
        && reason == Some("auth_failures")
        && before.status != "needs_attention"
    {
        refused = Some(queued_contexts(tx, peer_id)?);
    }
    let active = change.to == "active";
    Ok(Moved::Applied {
        previous: before.status,
        effects: Box::new(Effects {
            peer_id: peer_id.to_owned(),
            origin: before.origin,
            previous: Some(before.status),
            next: Some(change.to),
            reason,
            activation: active.then_some(change.cause),
            exit: (!active).then_some(change.cause),
            refused,
        }),
    })
}

/// Отмечает попытку связаться с пиром (`recordPeerAttempt`).
///
/// # Errors
/// Ошибка базы.
pub fn record_attempt(
    tx: &mut dyn Tx,
    peer_id: &str,
    from: &[&str],
    started_at: i64,
) -> DbResult<bool> {
    let Some(peer) = by_id(tx, peer_id)? else {
        return Ok(false);
    };
    if !from.contains(&peer.status) {
        return Ok(false);
    }
    tx.execute(
        "UPDATE federation_peers SET probe_attempts = probe_attempts + 1, last_probe_at = ? \
         WHERE id = ?",
        params![started_at, peer_id],
    )?;
    Ok(true)
}

/// Новая строка пира.
#[derive(Debug, Clone, Default)]
pub struct NewPeer {
    /// Адрес.
    pub origin: String,
    /// Общий секрет.
    pub secret: String,
    /// Кто начал: `admin`, `auto`, `remote`.
    pub initiated_by: &'static str,
    /// Название инстанса.
    pub instance_name: Option<String>,
    /// Эпоха пира.
    pub epoch: Option<String>,
    /// Когда пир отвечал последний раз.
    pub last_seen_at: Option<i64>,
    /// Статус: `pending`, `active` или `rejected`.
    pub status: &'static str,
    /// Причина активации (для `active`).
    pub cause: Option<&'static str>,
    /// Причина отказа (для `rejected`).
    pub reason: Option<&'static str>,
}

/// Создаёт пира (`insertPeer`). `None` — пир с таким адресом уже есть.
///
/// # Errors
/// Ошибка базы.
pub fn insert(
    tx: &mut dyn Tx,
    ids: &Snowflake,
    new: &NewPeer,
) -> DbResult<Option<(Peer, Effects)>> {
    if by_origin(tx, &new.origin)?.is_some() {
        return Ok(None);
    }
    let id = ids.next();
    let reason = if new.status == "rejected" {
        new.reason
    } else {
        None
    };
    tx.execute(
        "INSERT INTO federation_peers (id, origin, hmac_secret, initiated_by, instance_name, \
         peer_instance_id, last_seen_at, status, status_reason, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            &id,
            &new.origin,
            &new.secret,
            new.initiated_by,
            &new.instance_name,
            &new.epoch,
            new.last_seen_at,
            new.status,
            reason,
            now_ms()
        ],
    )?;
    let Some(peer) = by_id(tx, &id)? else {
        return Ok(None);
    };
    let effects = Effects {
        peer_id: id,
        origin: new.origin.clone(),
        previous: None,
        next: Some(new.status),
        reason,
        activation: if new.status == "active" {
            new.cause
        } else {
            None
        },
        exit: None,
        refused: None,
    };
    Ok(Some((peer, effects)))
}

/// Удаляет пира (`removePeer`), если он в одном из статусов `from` и, при
/// `unless_queued`, ему нечего отправлять.
///
/// # Errors
/// Ошибка базы.
pub fn remove(
    tx: &mut dyn Tx,
    peer_id: &str,
    from: &[&str],
    unless_queued: bool,
) -> DbResult<Option<Effects>> {
    let Some(before) = by_id(tx, peer_id)? else {
        return Ok(None);
    };
    if !from.contains(&before.status) {
        return Ok(None);
    }
    if unless_queued
        && tx
            .query_one(
                "SELECT id FROM federation_outbox WHERE peer_id = ? LIMIT 1",
                params![peer_id],
            )?
            .is_some()
    {
        return Ok(None);
    }
    tx.execute(
        "DELETE FROM federation_peers WHERE id = ?",
        params![peer_id],
    )?;
    Ok(Some(Effects {
        peer_id: peer_id.to_owned(),
        origin: before.origin,
        previous: Some(before.status),
        ..Effects::default()
    }))
}

/// Текст причины для пользователя (`peerStatusReasonText`).
#[must_use]
pub fn reason_text(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "denied_by_local_admin" => {
            "This instance's admin declined peering with the remote instance"
        }
        "denied_by_remote" => "The remote instance's admin declined peering",
        "revoked_by_remote" => "The remote instance has revoked peering with this instance",
        "expired_on_remote" => "The peering request expired before the remote admin answered",
        "stale_peering_on_remote" => {
            "The remote instance still holds an older peering with this instance; its admin has to reset it"
        }
        "auth_failures" => "Federation trust broke; an admin has to reset peering",
        "peer_reset_detected" => "The remote instance was reinstalled; an admin has to re-peer",
        "repeer_incomplete" => "Re-peering could not be verified; an admin has to reset peering",
        _ => return None,
    })
}

/// Событие всем администраторам на связи (`sendToAdmins`).
///
/// # Errors
/// Ошибка базы.
pub fn to_admins(tx: &mut dyn Tx, hub: &Hub, out: &mut Outbox, event: &Value) -> DbResult<()> {
    for user in hub.online_users() {
        if crate::routes::invites::is_admin(tx, &user)? {
            out.to_user(&user, event);
        }
    }
    Ok(())
}

/// Сбрасывает отсрочки очереди исходящих пира (`resetOutboxBackoff`).
fn reset_outbox_backoff(tx: &mut dyn Tx, peer_id: &str) -> DbResult<()> {
    let changed = tx.execute(
        "UPDATE federation_outbox SET next_retry_at = ?, attempts = 0 WHERE peer_id = ?",
        params![now_ms(), peer_id],
    )?;
    if changed > 0 {
        tracing::info!(
            peer = peer_id,
            entries = changed,
            "отсрочки очереди исходящих сброшены"
        );
    }
    Ok(())
}

/// Сообщает подписчикам исходящего запроса, что пиринг состоялся
/// (`fanoutOutboundSubscribers`).
fn fanout_outbound_subscribers(
    tx: &mut dyn Tx,
    ids: &Snowflake,
    out: &mut Outbox,
    origin: &str,
) -> DbResult<()> {
    let Some(parent) = tx.query_one(
        "SELECT id FROM peer_approval_requests WHERE origin = ? AND direction = 'outbound'",
        params![origin],
    )?
    else {
        return Ok(());
    };
    let parent = parent.text("id")?;
    let subscribers = tx.query(
        "SELECT user_id, trigger_reason, trigger_target FROM peer_approval_subscribers \
         WHERE request_id = ?",
        params![&parent],
    )?;
    let now = now_ms();
    for row in subscribers {
        let user = row.text("user_id")?;
        tx.execute(
            "INSERT INTO peer_approval_notifications (id, user_id, kind, peer_origin, \
             trigger_reason, trigger_target, created_at, read_at) \
             VALUES (?, ?, 'approved', ?, ?, ?, ?, NULL)",
            params![
                ids.next(),
                &user,
                origin,
                row.text("trigger_reason")?,
                row.text("trigger_target")?,
                now
            ],
        )?;
        out.to_user(
            &user,
            &json!({"type": "peering_notification_received", "kind": "approved"}),
        );
        out.to_user(&user, &json!({"type": "peering_subscription_changed"}));
    }
    tx.execute(
        "DELETE FROM peer_approval_subscribers WHERE request_id = ?",
        params![&parent],
    )?;
    tx.execute(
        "DELETE FROM peer_approval_requests WHERE id = ?",
        params![&parent],
    )?;
    Ok(())
}

/// Событие `federation_peer_rejected` местным участникам затронутых бесед
/// (`pushPeerRejectedEvent`).
fn push_rejected(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    origin: &str,
    contexts: &BTreeMap<String, String>,
    reason: &str,
) -> DbResult<()> {
    let mut affected = Vec::new();
    let mut users = std::collections::BTreeSet::new();
    for (context_id, context_type) in contexts {
        match context_type.as_str() {
            "dm" => {
                let members = tx.query(
                    "SELECT m.user_id, u.username, u.display_name, u.home_instance \
                     FROM dm_members m INNER JOIN users u ON m.user_id = u.id \
                     WHERE m.dm_channel_id = ?",
                    params![context_id],
                )?;
                let mut names = Vec::new();
                for member in &members {
                    let name = member
                        .opt_text("display_name")?
                        .filter(|name| !name.is_empty())
                        .or(member.opt_text("username")?.filter(|name| !name.is_empty()))
                        .unwrap_or_else(|| "Unknown".to_owned());
                    names.push(name);
                    if member
                        .opt_text("home_instance")?
                        .is_none_or(|home| home.is_empty())
                    {
                        users.insert(member.text("user_id")?);
                    }
                }
                names.truncate(4);
                affected.push(json!({
                    "contextType": "dm",
                    "contextId": context_id,
                    "contextLabel": names.join(", "),
                }));
            }
            "friend" => affected.push(json!({
                "contextType": "friend",
                "contextId": context_id,
                "contextLabel": context_id,
            })),
            _ => {}
        }
    }
    let label = by_origin(tx, origin)?.and_then(|peer| peer.instance_name);
    let mut event = json!({
        "type": "federation_peer_rejected",
        "peerOrigin": origin,
        "reason": reason_text(reason).unwrap_or_default(),
        "reasonCode": reason,
        "affectedContexts": affected,
    });
    if let (Some(label), Some(fields)) = (label, event.as_object_mut()) {
        fields.insert("peerLabel".to_owned(), label.into());
    }
    for user in users {
        out.to_user(&user, &event);
    }
    Ok(())
}

/// Копии пользователей пира показываются не в сети (`markPeerStubsOffline`):
/// дом, объявлявший их статус, больше не отвечает.
fn mark_stubs_offline(
    tx: &mut dyn Tx,
    app: &AppState,
    out: &mut Outbox,
    origin: &str,
) -> DbResult<()> {
    let domain = super::reset::domain_of(origin);
    let stubs = tx.query(
        "SELECT id, federation_home_orphaned FROM users WHERE home_instance = ? AND is_deleted = 0",
        params![&domain],
    )?;
    for stub in stubs {
        if stub.opt_flag("federation_home_orphaned")? == Some(true) {
            continue;
        }
        let id = stub.text("id")?;
        app.federation
            .projections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        let connected = app.hub.is_online(&id);
        let shown = if connected { "online" } else { "offline" };
        tx.execute(
            "UPDATE users SET status = ? WHERE id = ?",
            params![shown, &id],
        )?;
        if connected {
            app.hub.set_status(&id, shown);
        }
        app.hub.set_activities(&id, Vec::new());
        let event = crate::ws::presence_update(tx, &id, shown, Some(&[]))?;
        for user in crate::ws::profile_targets(tx, &id)? {
            out.to_user(&user, &event);
        }
    }
    Ok(())
}

/// Выполняет последствия перехода (`runPeerTransitionEffects`). Ошибка
/// только записывается в журнал: сам переход уже состоялся.
pub async fn run_effects(state: &AppState, effects: Effects) {
    let app = state.clone();
    let origin = effects.origin.clone();
    let peer_id = effects.peer_id.clone();
    let activation = (effects.next == Some("active") && effects.previous != Some("active"))
        .then_some(effects.activation)
        .flatten();
    let done = state
        .run(move |tx, out| {
            let changed = json!({"type": "federation_peers_changed"});
            to_admins(tx, &app.hub, out, &changed)?;
            if activation.is_some() {
                let event = json!({"type": "federation_peer_active", "peerOrigin": effects.origin});
                for user in app.hub.online_users() {
                    out.to_user(&user, &event);
                }
            }
            if effects.previous == Some("active")
                && effects.next.is_some_and(|next| next != "active")
                && effects.exit.is_some()
            {
                // Звонки, которые ведёт пир, забываются, а его участники
                // выходят из звонков отсюда (`onPeerDeactivated`).
                let label = tx
                    .query_one(
                        "SELECT instance_name FROM federation_peers WHERE origin = ?",
                        crate::params![&effects.origin],
                    )?
                    .map(|row| row.opt_text("instance_name"))
                    .transpose()?
                    .flatten();
                super::calls::peer_deactivated(
                    tx,
                    &app,
                    out,
                    &effects.origin,
                    effects.next,
                    label.as_deref(),
                )?;
                mark_stubs_offline(tx, &app, out, &effects.origin)?;
                to_admins(tx, &app.hub, out, &changed)?;
            }
            if let (Some(contexts), Some(reason)) = (&effects.refused, effects.reason)
                && !contexts.is_empty()
            {
                push_rejected(tx, out, &effects.origin, contexts, reason)?;
            }
            Ok(())
        })
        .await;
    if let Err(error) = done {
        tracing::error!(
            peer = origin,
            ?error,
            "последствия перехода пира не выполнены"
        );
    } else if let Some(cause) = activation {
        // Как и в оригинале, переход не ждёт догонялки: ответ на рукопожатие
        // уходит сразу, а `onPeerActivated` идёт следом.
        let app = state.clone();
        tokio::spawn(async move { on_activated(&app, &peer_id, cause).await });
    }
}

/// Пир стал действующим (`onPeerActivated`): сброс пауз очереди, лечение
/// после сброса пира, догонялка, рассылка подписчикам, проход по копиям,
/// присутствие.
pub async fn on_activated(state: &AppState, peer_id: &str, cause: &'static str) {
    let (app, id) = (state.clone(), peer_id.to_owned());
    let origin = state
        .run(move |tx, _| {
            reset_outbox_backoff(tx, &id)?;
            let Some(peer) = by_id(tx, &id)? else {
                return Ok(None);
            };
            if let Some(epoch) = &peer.epoch {
                super::reset::heal(tx, &peer.origin, epoch, cause)?;
            }
            Ok(Some(peer.origin))
        })
        .await;
    let origin = match origin {
        Ok(Some(origin)) => origin,
        Ok(None) => return,
        Err(error) => {
            tracing::error!(peer = peer_id, ?error, "включение пира не выполнено");
            return;
        }
    };
    super::sync::sync_peer(
        state,
        peer_id,
        cause,
        &super::sync::ALL_CONTEXTS,
        super::sync::PASS_BUDGET,
    )
    .await;
    let at = origin.clone();
    let done = state
        .run(move |tx, out| {
            fanout_outbound_subscribers(tx, &app.ids, out, &at)?;
            super::relay::presence_for_peer(
                tx,
                super::relay::Relay {
                    config: &app.config,
                    ids: &app.ids,
                },
                &app.hub,
                &at,
            )?;
            to_admins(
                tx,
                &app.hub,
                out,
                &json!({"type": "federation_peers_changed"}),
            )?;
            Ok(())
        })
        .await;
    tokio::spawn(super::lookup::backfill_for_peer(
        state.clone(),
        origin.clone(),
    ));
    if let Err(error) = done {
        tracing::error!(peer = origin, ?error, "включение пира не выполнено");
    }
}

/// При запуске: пиры, с которыми догонялка ещё ни разу не прошла, включаются
/// заново; по копиям всех действующих пиров — проход (`startupBootstrapSync`).
pub async fn startup_bootstrap(state: &AppState) {
    let rows = state
        .run(|tx, _| {
            Ok(tx.query(
                "SELECT id, origin, last_synced_at FROM federation_peers WHERE status = 'active'",
                params![],
            )?)
        })
        .await
        .unwrap_or_default();
    for row in &rows {
        if row.opt_int("last_synced_at").ok().flatten().unwrap_or(0) == 0
            && let Ok(id) = row.text("id")
        {
            on_activated(state, &id, "startup_bootstrap").await;
        }
    }
    for row in rows {
        if let Ok(origin) = row.text("origin") {
            tokio::spawn(super::lookup::backfill_for_peer(state.clone(), origin));
        }
    }
}

/// Переход с выполнением последствий (`transitionPeer`).
///
/// # Errors
/// Ошибка базы.
pub async fn transition(
    state: &AppState,
    peer_id: &str,
    change: Transition,
) -> crate::error::ApiResult<Moved> {
    let id = peer_id.to_owned();
    let moved = state.run(move |tx, _| Ok(apply(tx, &id, &change)?)).await?;
    if let Moved::Applied { effects, .. } = &moved {
        run_effects(state, (**effects).clone()).await;
    }
    Ok(moved)
}

/// Решение о входящем рукопожатии.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inbound {
    /// Создать активного пира.
    Create,
    /// Активировать существующую строку.
    Activate {
        /// Из какого статуса.
        from: &'static str,
        /// Причина активации.
        cause: &'static str,
    },
    /// Поставить в очередь на одобрение администратором.
    Queue,
    /// Пиринг уже есть: нужен сброс.
    RefuseExists,
    /// Пир отозван.
    RefuseRevoked,
    /// Администратор отказал.
    RefuseDenied,
    /// Идёт собственное рукопожатие с этим инстансом.
    RefuseInProgress,
}

/// Что известно о входящем рукопожатии.
#[derive(Debug, Clone, Copy)]
pub struct InboundInput<'a> {
    /// Существующая строка пира.
    pub row: Option<&'a Peer>,
    /// Принимает ли инстанс пиринг без одобрения.
    pub auto_accept: bool,
    /// Токен одобрения из запроса.
    pub token: Option<&'a str>,
    /// Идёт ли наше рукопожатие с этим инстансом.
    pub own_in_flight: bool,
    /// Наш адрес.
    pub our_origin: &'a str,
    /// Адрес запросившего.
    pub source_origin: &'a str,
}

/// Решение о входящем рукопожатии (`decideInboundHandshake`).
#[must_use]
pub fn decide_inbound(input: InboundInput<'_>) -> Inbound {
    let Some(row) = input.row else {
        return if input.auto_accept {
            Inbound::Create
        } else {
            Inbound::Queue
        };
    };
    let authorized = input.auto_accept || row.initiated_by == "admin";
    let uncompleted = |from, cause| {
        if !authorized {
            Inbound::Queue
        } else if input.own_in_flight && input.our_origin < input.source_origin {
            // Из двух встречных рукопожатий уступает то, чей адрес меньше.
            Inbound::RefuseInProgress
        } else {
            Inbound::Activate { from, cause }
        }
    };
    match row.status {
        "revoked" => Inbound::RefuseRevoked,
        "pending" => uncompleted("pending", "accept_pending"),
        "awaiting_approval" => {
            if !authorized {
                return Inbound::Queue;
            }
            let valid = row
                .approval_token
                .as_deref()
                .is_some_and(|token| !token.is_empty() && Some(token) == input.token);
            if valid {
                Inbound::Activate {
                    from: "awaiting_approval",
                    cause: "accept_awaiting_approval",
                }
            } else if input.auto_accept {
                Inbound::Activate {
                    from: "awaiting_approval",
                    cause: "accept_awaiting_approval_fallback",
                }
            } else {
                Inbound::Queue
            }
        }
        "rejected" => match row.reason.as_deref() {
            Some("denied_by_local_admin") => Inbound::RefuseDenied,
            Some(
                "denied_by_remote"
                | "revoked_by_remote"
                | "expired_on_remote"
                | "stale_peering_on_remote",
            ) => uncompleted("rejected", "accept_rejected_override"),
            _ if input.auto_accept => Inbound::Activate {
                from: "rejected",
                cause: "accept_rejected_override",
            },
            _ => Inbound::RefuseDenied,
        },
        // active, unreachable, needs_attention
        _ => Inbound::RefuseExists,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    fn peer(status: &'static str, reason: Option<&str>, by: &str, token: Option<&str>) -> Peer {
        Peer {
            id: "1".into(),
            origin: "https://b.example".into(),
            status,
            reason: reason.map(str::to_owned),
            secret: "s".into(),
            pending_secret: None,
            rotation_at: None,
            initiated_by: by.into(),
            instance_name: None,
            epoch: None,
            approval_token: token.map(str::to_owned),
            nonce_supported: false,
            probe_attempts: 0,
            last_probe_at: None,
            public: Value::Null,
        }
    }

    fn decide(row: Option<&Peer>, auto: bool, token: Option<&str>, in_flight: bool) -> Inbound {
        decide_inbound(InboundInput {
            row,
            auto_accept: auto,
            token,
            own_in_flight: in_flight,
            our_origin: "https://a.example",
            source_origin: "https://b.example",
        })
    }

    #[test]
    fn inbound_decisions_follow_the_original_table() {
        assert_eq!(decide(None, true, None, false), Inbound::Create);
        assert_eq!(decide(None, false, None, false), Inbound::Queue);
        for status in ["active", "unreachable", "needs_attention"] {
            let row = peer(status, None, "auto", None);
            assert_eq!(decide(Some(&row), true, None, false), Inbound::RefuseExists);
        }
        let revoked = peer("revoked", None, "admin", None);
        assert_eq!(
            decide(Some(&revoked), true, None, false),
            Inbound::RefuseRevoked
        );

        let pending = peer("pending", None, "auto", None);
        assert_eq!(decide(Some(&pending), false, None, false), Inbound::Queue);
        assert_eq!(
            decide(Some(&pending), true, None, false),
            Inbound::Activate {
                from: "pending",
                cause: "accept_pending"
            }
        );
        // Наш адрес меньше: встречное рукопожатие уступает.
        assert_eq!(
            decide(Some(&pending), true, None, true),
            Inbound::RefuseInProgress
        );
        let by_admin = peer("pending", None, "admin", None);
        assert_eq!(
            decide(Some(&by_admin), false, None, false),
            Inbound::Activate {
                from: "pending",
                cause: "accept_pending"
            }
        );

        let waiting = peer("awaiting_approval", None, "admin", Some("tok"));
        assert_eq!(
            decide(Some(&waiting), false, Some("tok"), false),
            Inbound::Activate {
                from: "awaiting_approval",
                cause: "accept_awaiting_approval"
            }
        );
        assert_eq!(
            decide(Some(&waiting), false, Some("bad"), false),
            Inbound::Queue
        );
        assert_eq!(
            decide(Some(&waiting), true, None, false),
            Inbound::Activate {
                from: "awaiting_approval",
                cause: "accept_awaiting_approval_fallback"
            }
        );
        let stranger = peer("awaiting_approval", None, "auto", Some("tok"));
        assert_eq!(
            decide(Some(&stranger), false, Some("tok"), false),
            Inbound::Queue
        );

        let denied = peer("rejected", Some("denied_by_local_admin"), "admin", None);
        assert_eq!(
            decide(Some(&denied), true, None, false),
            Inbound::RefuseDenied
        );
        let remote = peer("rejected", Some("denied_by_remote"), "auto", None);
        assert_eq!(
            decide(Some(&remote), true, None, false),
            Inbound::Activate {
                from: "rejected",
                cause: "accept_rejected_override"
            }
        );
        assert_eq!(decide(Some(&remote), false, None, false), Inbound::Queue);
        let odd = peer("rejected", None, "auto", None);
        assert_eq!(
            decide(Some(&odd), false, None, false),
            Inbound::RefuseDenied
        );
    }

    #[test]
    fn unknown_status_reads_as_needs_attention() {
        assert_eq!(status_of("active"), "active");
        assert_eq!(status_of("something_new"), "needs_attention");
    }
}
