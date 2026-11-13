//! Приём событий от других инстансов (`routes/federation/handlers/relay.ts`,
//! `routes/federation/events/dispatch.ts`, `utils/federationAppliedEvents.ts`).
//!
//! Каждое событие применяется в своей транзакции. Если для события нужны
//! файлы с другого инстанса (аватар, баннер, значок группы), транзакция
//! откатывается, файлы скачиваются вне её, и событие применяется заново —
//! уже со скачанными файлами.

pub mod files;
pub mod friends;
pub mod membership;
pub mod messages;
pub mod profile;
pub mod state;

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};

use super::identity::{self, Actor, Hints, Replica, Resolved};
use crate::db::{DbError, DbResult, Row, Tx};
use crate::federation_auth::normalize_origin;
use crate::hub::Outbox;
use crate::params;
use crate::state::{AppState, now_ms};

/// Ответ на событие.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Принято (в том числе «уже было»).
    Accepted,
    /// Отклонено с причиной.
    Rejected(String),
}

/// Почему обработка события прервалась.
#[derive(Debug)]
pub enum Stop {
    /// Ошибка базы.
    Db(DbError),
    /// Нужен файл с другого инстанса: `(адрес, инстанс-источник)`.
    Need(String, String),
}

impl From<DbError> for Stop {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

/// Итог обработки одного события.
pub type Step = Result<Verdict, Stop>;

/// Отказ с причиной.
#[allow(clippy::unnecessary_wraps, reason = "единый вид с принятием")]
fn reject(reason: &str) -> Step {
    Ok(Verdict::Rejected(reason.to_owned()))
}

/// Принятие.
#[allow(clippy::unnecessary_wraps, reason = "единый вид с отказом")]
const fn accept() -> Step {
    Ok(Verdict::Accepted)
}

/// Как событие дошло (`RelayDelivery`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Прислано пиром.
    Live,
    /// Забрано из журнала пира при догонялке.
    CatchUp,
}

/// Всё, что нужно обработчику события.
pub struct Ctx<'a> {
    /// Транзакция.
    pub tx: &'a mut dyn Tx,
    /// События клиентам после фиксации.
    pub out: &'a mut Outbox,
    /// Состояние сервера.
    pub app: &'a AppState,
    /// Инстанс, от имени которого пришло событие.
    pub source: &'a str,
    /// Адрес пира, подписавшего запрос.
    pub peer: &'a str,
    /// Как дошло событие.
    pub delivery: Delivery,
    assets: &'a HashMap<String, Option<String>>,
    kept: Vec<String>,
    stale: Vec<String>,
}

impl std::fmt::Debug for Ctx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx")
            .field("source", &self.source)
            .field("peer", &self.peer)
            .finish_non_exhaustive()
    }
}

impl Ctx<'_> {
    /// Скачанный файл с другого инстанса (`downloadProfileAsset`): имя
    /// локального файла или `None`, если скачать не удалось.
    ///
    /// # Errors
    /// [`Stop::Need`], если файл ещё не скачивали.
    pub fn asset(&mut self, url: &str, base: &str) -> Result<Option<String>, Stop> {
        let key = format!("{base}\n{url}");
        match self.assets.get(&key) {
            Some(found) => Ok(found.clone()),
            None => Err(Stop::Need(url.to_owned(), base.to_owned())),
        }
    }

    /// Скачанный файл записан в базу: его не стирать.
    pub fn keep(&mut self, file: &str) {
        self.kept.push(file.to_owned());
    }

    /// Файл больше не нужен: стереть после фиксации.
    pub fn discard(&mut self, file: &str) {
        self.stale.push(file.to_owned());
    }

    /// Находит пользователя или создаёт копию.
    ///
    /// # Errors
    /// Ошибка базы.
    pub fn replica(&mut self, actor: &Actor, hints: &Hints) -> DbResult<Option<Replica>> {
        identity::resolve_or_create(self.tx, self.app, self.out, actor, hints)
    }

    /// Пользователь по идентичности.
    ///
    /// # Errors
    /// Ошибка базы.
    pub fn resolve(&mut self, actor: &Value) -> DbResult<Resolved> {
        match Actor::from_value(Some(actor)) {
            Some(actor) => identity::resolve_actor(self.tx, &self.app.config, &actor),
            None => Ok(Resolved::Unknown),
        }
    }

    /// Почему пир не может говорить за участника (`attributionRefusal`).
    ///
    /// # Errors
    /// Ошибка базы.
    pub fn refusal(&mut self, actor: Option<&Value>) -> DbResult<Option<&'static str>> {
        identity::attribution_refusal(self.tx, &self.app.config, actor, self.source)
    }

    /// Всем соединениям пользователя.
    ///
    /// # Errors
    /// Ошибка базы.
    pub fn to_user(&mut self, user: &str, event: &Value) -> DbResult<()> {
        crate::broadcast::to_user(self.tx, self.out, user, event)
    }

    /// Всем участникам переписки (`sendToDmMembers`).
    ///
    /// # Errors
    /// Ошибка базы.
    pub fn to_dm_members(&mut self, channel: &str, event: &Value) -> DbResult<()> {
        crate::broadcast::to_dm_members(self.tx, self.out, channel, event, None)
    }
}

// ─── Чтение событий ─────────────────────────────────────────────────────────

/// Строковое поле.
pub fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// Непустое строковое поле.
pub fn non_empty<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    text(value, key).filter(|text| !text.is_empty())
}

/// Объект или массив, который в JavaScript был бы «истинным».
pub fn present<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.get(key).filter(|value| truthy(value))
}

/// Истинность значения по правилам JavaScript.
#[must_use]
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// Число в миллисекундах; дробное отбрасывается, нечисло — 0 (`clockValue`).
#[must_use]
pub fn millis(value: Option<&Value>) -> i64 {
    #[allow(clippy::cast_possible_truncation, reason = "время в миллисекундах")]
    value
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .map_or(0, |n| n as i64)
}

/// Время события (`event.timestamp`).
#[must_use]
pub fn timestamp(event: &Value) -> i64 {
    millis(event.get("timestamp"))
}

/// Подсказки о пользователе из `profile` участника.
#[must_use]
pub fn hints_of(participant: &Value) -> Hints {
    Hints::from_profile(participant.get("profile"))
}

/// Объект `{homeUserId, homeInstance}` из события.
#[must_use]
pub fn actor(value: &Value) -> Option<Actor> {
    Actor::from_value(Some(value))
}

/// Узел адреса в нижнем регистре.
#[must_use]
pub fn domain(origin: &str) -> String {
    super::reset::domain_of(origin).to_lowercase()
}

/// Узел адреса как есть (`extractDomain`).
#[must_use]
pub fn raw_domain(origin: &str) -> String {
    super::reset::domain_of(origin)
}

/// Строка пользователя.
///
/// # Errors
/// Ошибка базы.
pub fn user_row(tx: &mut dyn Tx, id: &str) -> DbResult<Option<Row>> {
    tx.query_one("SELECT * FROM users WHERE id = ?", params![id])
}

/// Живая переписка по федеративному идентификатору.
///
/// # Errors
/// Ошибка базы.
pub fn live_channel(tx: &mut dyn Tx, federated: &str) -> DbResult<Option<Row>> {
    tx.query_one(
        "SELECT * FROM dm_channels WHERE federated_id = ? AND deleted_at IS NULL",
        params![federated],
    )
}

/// Переписка по федеративному идентификатору, удалённая тоже.
///
/// # Errors
/// Ошибка базы.
pub fn any_channel(tx: &mut dyn Tx, federated: &str) -> DbResult<Option<Row>> {
    tx.query_one(
        "SELECT * FROM dm_channels WHERE federated_id = ?",
        params![federated],
    )
}

// ─── Журнал применённых событий ─────────────────────────────────────────────

/// Срок хранения записей журнала (`APPLIED_EVENT_RETENTION_MS`).
pub const APPLIED_RETENTION_MS: i64 = 100 * 24 * 60 * 60 * 1000;

/// Записан ли ключ для инстанса (`hasAppliedEvent`).
///
/// # Errors
/// Ошибка базы.
pub fn has_applied(tx: &mut dyn Tx, origin: &str, key: &str) -> DbResult<bool> {
    let Some(source) = normalize_origin(Some(origin)) else {
        return Ok(false);
    };
    Ok(tx
        .query_one(
            "SELECT event_key FROM federation_applied_events WHERE source_origin = ? AND event_key = ?",
            params![source, key],
        )?
        .is_some())
}

/// Записывает ключ для инстанса (`recordAppliedEvent`).
///
/// # Errors
/// Ошибка базы.
pub fn record_applied(tx: &mut dyn Tx, origin: &str, key: &str) -> DbResult<()> {
    let Some(source) = normalize_origin(Some(origin)) else {
        return Ok(());
    };
    tx.execute(
        "INSERT INTO federation_applied_events (source_origin, event_key, applied_at) VALUES (?, ?, ?) \
         ON CONFLICT DO NOTHING",
        params![source, key, now_ms()],
    )?;
    Ok(())
}

/// Удаляет устаревшие записи журнала (`sweepAppliedEvents`).
///
/// # Errors
/// Ошибка базы.
pub fn sweep_applied(tx: &mut dyn Tx, now: i64) -> DbResult<u64> {
    tx.execute(
        "DELETE FROM federation_applied_events WHERE applied_at < ?",
        params![now - APPLIED_RETENTION_MS],
    )
}

/// События, которые применяются один раз (`LEDGERED_EVENT_TYPES`).
fn ledgered(kind: &str) -> bool {
    matches!(
        kind,
        "friend_request_create"
            | "friend_request_update"
            | "friend_request_cancel"
            | "friend_add"
            | "friend_remove"
    )
}

// ─── Разбор пачки ───────────────────────────────────────────────────────────

/// Итог пачки событий.
#[derive(Debug, Default, Clone)]
pub struct Outcome {
    /// Принятые события.
    pub accepted: Vec<String>,
    /// Отклонённые события с причинами.
    pub rejected: Vec<(String, String)>,
    /// Не доставленные адресатам.
    pub undeliverable: Vec<(String, String)>,
}

fn message_id(event: &Value) -> String {
    match event.get("messageId") {
        Some(Value::String(id)) => id.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn process(cx: &mut Ctx<'_>, event: &Value) -> Step {
    match text(event, "eventType").unwrap_or_default() {
        "create" => messages::create(cx, event),
        "update" => messages::update(cx, event),
        "delete" => messages::delete(cx, event),
        "reaction_add" => messages::reaction(cx, event, true),
        "reaction_remove" => messages::reaction(cx, event, false),
        "member_add" => membership::add(cx, event),
        "member_remove" => membership::remove(cx, event),
        "ownership_transfer" => membership::ownership(cx, event),
        "group_metadata_update" => membership::metadata(cx, event),
        "friend_request_create" => friends::request_create(cx, event),
        "friend_request_update" => friends::request_update(cx, event),
        "friend_request_cancel" => friends::request_cancel(cx, event),
        "friend_add" => friends::add(cx, event),
        "friend_remove" => friends::remove(cx, event),
        "file_rejected" => state::file_rejected(cx, event),
        "presence_update" => state::presence(cx, event),
        "read_state_update" => state::read_state(cx, event),
        "dm_close" => state::close(cx, event, true),
        "dm_reopen" => state::close(cx, event, false),
        "profile_update" => profile::update(cx, event),
        // Звонки и «печатает» между инстансами ещё не перенесены: событие
        // принимается и ничего не меняет.
        "dm_call_start" | "dm_call_accept" | "dm_call_reject" | "dm_call_end"
        | "dm_typing_start" | "dm_typing_stop" => {
            tracing::debug!(kind = ?text(event, "eventType"), "событие звонка или набора текста от пира пропущено");
            accept()
        }
        _ => reject("unknown_event_type"),
    }
}

/// Применяет одно событие, скачивая нужные файлы между попытками.
async fn apply_one(
    app: &AppState,
    event: Arc<Value>,
    source: Arc<str>,
    peer: Arc<str>,
    delivery: Delivery,
) -> Result<Verdict, DbError> {
    let mut assets: HashMap<String, Option<String>> = HashMap::new();
    let mut downloaded: Vec<String> = Vec::new();
    let result = loop {
        let cache = Arc::new(assets.clone());
        let (app_tx, event_tx, source_tx, peer_tx) =
            (app.clone(), event.clone(), source.clone(), peer.clone());
        let attempt = app
            .db
            .transaction(move |tx| {
                let mut out = Outbox::new();
                let mut cx = Ctx {
                    tx,
                    out: &mut out,
                    app: &app_tx,
                    source: &source_tx,
                    peer: &peer_tx,
                    delivery,
                    assets: &cache,
                    kept: Vec::new(),
                    stale: Vec::new(),
                };
                let verdict = process(&mut cx, &event_tx)?;
                let (kept, stale) = (cx.kept, cx.stale);
                Ok::<_, Stop>((verdict, out, kept, stale))
            })
            .await;
        match attempt {
            Ok((verdict, out, kept, stale)) => {
                app.hub.flush(out);
                downloaded.retain(|file| !kept.contains(file));
                for file in stale {
                    crate::files::delete_upload_file(&app.config, &file).await;
                }
                break Ok(verdict);
            }
            Err(Stop::Need(url, base)) => {
                let key = format!("{base}\n{url}");
                if assets.contains_key(&key) || assets.len() > 16 {
                    break Err(DbError::Cancelled);
                }
                let file = profile::download_asset(app, &url, &base).await;
                if let Some(file) = &file {
                    downloaded.push(file.clone());
                }
                assets.insert(key, file);
            }
            Err(Stop::Db(error)) => break Err(error),
        }
    };
    for file in downloaded {
        crate::files::delete_upload_file(&app.config, &file).await;
    }
    result
}

/// Обрабатывает пачку событий (`processRelayEvents`).
pub async fn process_events(
    app: &AppState,
    events: &[Value],
    source: &str,
    peer: &str,
    delivery: Delivery,
) -> Outcome {
    let mut outcome = Outcome::default();
    if normalize_origin(Some(source)) != normalize_origin(Some(peer)) {
        tracing::error!(
            source,
            peer,
            "пачка отклонена: источник не совпадает с пиром"
        );
        for event in events {
            outcome
                .rejected
                .push((message_id(event), "source_peer_mismatch".to_owned()));
        }
        return outcome;
    }
    let (source_arc, peer_arc): (Arc<str>, Arc<str>) = (source.into(), peer.into());
    for event in events {
        let id = message_id(event);
        let kind = text(event, "eventType").unwrap_or_default().to_owned();
        let ledger_key = ledgered(&kind).then(|| format!("{kind}:{id}"));
        if let Some(key) = &ledger_key {
            let (origin, key) = (source.to_owned(), key.clone());
            let seen = app
                .db
                .transaction(move |tx| has_applied(tx, &origin, &key))
                .await
                .unwrap_or(false);
            if seen {
                outcome.rejected.push((id, "duplicate".to_owned()));
                continue;
            }
        }
        let verdict = apply_one(
            app,
            Arc::new(event.clone()),
            source_arc.clone(),
            peer_arc.clone(),
            delivery,
        )
        .await;
        let accepted = match verdict {
            Ok(Verdict::Accepted) => {
                outcome.accepted.push(id.clone());
                true
            }
            Ok(Verdict::Rejected(reason)) => {
                outcome.rejected.push((id.clone(), reason));
                false
            }
            Err(error) => {
                tracing::error!(event = id, %error, "не удалось применить событие пира");
                outcome
                    .rejected
                    .push((id.clone(), "processing_error".to_owned()));
                false
            }
        };
        if accepted && let Some(key) = ledger_key {
            let origin = source.to_owned();
            if let Err(error) = app
                .db
                .transaction(move |tx| record_applied(tx, &origin, &key))
                .await
            {
                tracing::error!(%error, "не удалось записать применённое событие");
            }
        }
    }
    outcome
}

/// Ответ пиру: отказы в виде, который он понимает (`rejectionsForSender`).
#[must_use]
pub fn rejections_for_sender(rejected: &[(String, String)], capabilities: Option<&Value>) -> Value {
    let retries_unproven = capabilities
        .and_then(Value::as_array)
        .is_some_and(|list| list.iter().any(|item| item == "attribution_unproven"));
    Value::Array(
        rejected
            .iter()
            .map(|(id, reason)| {
                let reason = if !retries_unproven && reason == "attribution_unproven" {
                    "attribution_mismatch"
                } else {
                    reason.as_str()
                };
                json!({"messageId": id, "reason": reason})
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unproven_is_translated_for_old_senders() {
        let rejected = vec![("1".to_owned(), "attribution_unproven".to_owned())];
        assert_eq!(
            rejections_for_sender(&rejected, None)[0]["reason"],
            "attribution_mismatch"
        );
        let caps = json!(["attribution_unproven"]);
        assert_eq!(
            rejections_for_sender(&rejected, Some(&caps))[0]["reason"],
            "attribution_unproven"
        );
    }

    #[test]
    fn js_truthiness() {
        assert!(!truthy(&json!(0)));
        assert!(!truthy(&json!("")));
        assert!(truthy(&json!([])));
        assert_eq!(millis(Some(&json!(12.7))), 12);
    }
}
