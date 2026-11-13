//! Содержимое событий для других инстансов: переписки, реакции, позиции
//! чтения, группы, дружба, профиль и присутствие (`queueDmRelay`,
//! `queueReadStateRelay`, `queueDmCloseRelay`, `queueGroupMetadataRelay`,
//! `routes/social.ts`, `routes/users.ts`, `utils/federationPresence.ts`).
//!
//! Каждая функция пишет запись в журнал мутаций (по нему пир догоняет
//! пропущенное) и ставит событие в очереди пиров ([`super::outbox::queue`])
//! в той же транзакции, что и само изменение.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};

use super::outbox::{self, Event};
use super::{append_mutation_log_in, dm_federated_id, our_origin, relay_enabled};
use crate::config::Config;
use crate::db::{DbResult, Row, Tx};
use crate::params;
use crate::snowflake::Snowflake;
use crate::state::now_ms;

/// Сколько упоминаний уходит с сообщением (`MAX_RELAYED_MENTIONS`).
const MAX_MENTIONS: usize = 100;

/// Всё, что нужно для постановки события.
#[derive(Debug, Clone, Copy)]
pub struct Relay<'a> {
    /// Настройки сервера.
    pub config: &'a Config,
    /// Источник идентификаторов.
    pub ids: &'a Snowflake,
}

impl Relay<'_> {
    fn origin(&self) -> String {
        our_origin(self.config)
    }

    #[allow(clippy::too_many_arguments, reason = "поля записи журнала и события")]
    fn log_and_queue(
        &self,
        tx: &mut dyn Tx,
        entity: &str,
        context: &str,
        kind: &str,
        log: Option<&Value>,
        payload: &Value,
        targets: Option<&[String]>,
        context_type: &str,
    ) -> DbResult<()> {
        append_mutation_log_in(tx, self.ids, entity, context, context_type, kind, log)?;
        outbox::queue(
            tx,
            self.ids,
            &Event {
                entity_id: entity,
                context_id: context,
                event_type: kind,
                payload,
                targets,
                context_type,
            },
        )
    }
}

// ─── Общие части ───────────────────────────────────────────────────────────

/// `{homeUserId, homeInstance}` пользователя: свой — нашим адресом.
fn identity_of(origin: &str, user: &Row) -> DbResult<Value> {
    Ok(json!({
        "homeUserId": user.opt_text("home_user_id")?.filter(|id| !id.is_empty()).map_or_else(|| user.text("id"), Ok)?,
        "homeInstance": user.opt_text("home_instance")?.filter(|home| !home.is_empty()).unwrap_or_else(|| origin.to_owned()),
    }))
}

fn user_row(tx: &mut dyn Tx, id: &str) -> DbResult<Option<Row>> {
    tx.query_one("SELECT * FROM users WHERE id = ?", params![id])
}

fn identity(tx: &mut dyn Tx, origin: &str, id: &str) -> DbResult<Value> {
    match user_row(tx, id)? {
        Some(row) => identity_of(origin, &row),
        None => Ok(json!({"homeUserId": id, "homeInstance": origin})),
    }
}

static HANDLE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new("^[a-z0-9_]+$").ok());
static SUFFIXED: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new("^([a-z0-9_]+)~([0-9a-f]+)$").ok());

/// Имя из подсказки, если это допустимое имя (`handleFromHint`).
fn handle_from_hint(name: &str) -> Option<String> {
    let handle = name.trim().to_lowercase();
    HANDLE
        .as_ref()
        .is_some_and(|re| re.is_match(&handle))
        .then_some(handle)
}

/// Имя пользователя для других инстансов (`relayHandleOf`).
///
/// # Errors
/// В строке нет нужных столбцов.
pub fn relay_handle(user: &Row) -> DbResult<Option<String>> {
    let username = user.text("username")?;
    let Some(home) = user
        .opt_text("home_instance")?
        .filter(|home| !home.is_empty())
    else {
        let local = username.split('@').next().unwrap_or(&username);
        return Ok(handle_from_hint(local));
    };
    let suffix = format!("@{home}").to_lowercase();
    let lower = username.to_lowercase();
    let Some(local) = lower.strip_suffix(&suffix) else {
        return Ok(None);
    };
    if user
        .opt_text("home_user_id")?
        .is_some_and(|id| !id.is_empty() && id.to_lowercase() == local)
    {
        return Ok(None);
    }
    let suffixed = SUFFIXED
        .as_ref()
        .and_then(|re| re.captures(local))
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_owned());
    Ok(suffixed.or_else(|| handle_from_hint(local)))
}

/// Участник переписки для других инстансов (`FederationRelayParticipant`).
fn participant(origin: &str, user: &Row) -> DbResult<Value> {
    let mut entry = identity_of(origin, user)?;
    let profile = if user.opt_flag("is_deleted")? == Some(true) {
        json!({"deleted": true})
    } else {
        let native = user
            .opt_text("home_instance")?
            .is_none_or(|home| home.is_empty());
        json!({
            "username": relay_handle(user)?,
            "displayName": user.opt_text("display_name")?,
            "avatar": user.opt_text("avatar")?,
            "avatarColor": user.opt_text("avatar_color")?,
            "status": if native { user.opt_text("status")? } else { None },
        })
    };
    if let Some(fields) = entry.as_object_mut() {
        fields.insert("profile".to_owned(), profile);
    }
    Ok(entry)
}

/// Участники переписки (`getDmParticipants`).
///
/// # Errors
/// Ошибка базы.
pub fn participants(tx: &mut dyn Tx, origin: &str, channel: &str) -> DbResult<Vec<Value>> {
    let rows = tx.query(
        "SELECT users.* FROM dm_members INNER JOIN users ON dm_members.user_id = users.id \
         WHERE dm_members.dm_channel_id = ?",
        params![channel],
    )?;
    rows.iter().map(|row| participant(origin, row)).collect()
}

/// Инстансы участников, кроме нашего (`relayTargetOrigins`).
fn target_origins(origin: &str, members: &[Value]) -> Vec<String> {
    let mut origins = Vec::new();
    for member in members {
        let home = member["homeInstance"].as_str().unwrap_or(origin);
        let normalized = if home.starts_with("http") {
            home.to_owned()
        } else {
            format!("https://{home}")
        };
        if normalized != origin && !origins.contains(&normalized) {
            origins.push(normalized);
        }
    }
    origins
}

/// Инстансы участников переписки (`getGroupDmTargetOrigins`).
///
/// # Errors
/// Ошибка базы.
pub fn dm_targets(tx: &mut dyn Tx, origin: &str, channel: &str) -> DbResult<Vec<String>> {
    Ok(target_origins(origin, &participants(tx, origin, channel)?))
}

/// Адрес значка группы на проводе (`normalizeIconForWire`).
fn wire_icon(icon: Option<String>, origin: &str) -> Option<String> {
    let icon = icon.filter(|icon| !icon.is_empty())?;
    if icon.starts_with("http://") || icon.starts_with("https://") {
        Some(icon)
    } else {
        Some(format!("{origin}/api/uploads/{icon}"))
    }
}

// ─── Часы субъектов ─────────────────────────────────────────────────────────

/// Узел адреса, как его сравнивают часы (`identityPart`).
fn identity_part(origin: &str, config: &Config, actor: &Value) -> Option<(String, String)> {
    let user = actor["homeUserId"].as_str().filter(|id| !id.is_empty())?;
    let home = actor["homeInstance"].as_str()?;
    let domain = super::reset::domain_of(home.trim()).to_lowercase();
    if domain.is_empty() {
        return None;
    }
    let ours = super::reset::domain_of(origin).to_lowercase();
    let identity_domain = config
        .domain
        .as_deref()
        .filter(|domain| !domain.is_empty())
        .map_or_else(|| ours.clone(), str::to_lowercase);
    let own = domain == ours || domain == identity_domain;
    Some((user.to_owned(), if own { identity_domain } else { domain }))
}

/// Сдвигает часы субъекта к `at` (`recordSubjectChange`).
pub fn record_subject(tx: &mut dyn Tx, subject: &str, at: i64) -> DbResult<()> {
    let now = now_ms();
    let current = tx
        .query_one(
            "SELECT changed_at FROM federation_subject_clocks WHERE subject_key = ?",
            params![subject],
        )?
        .map(|row| row.int("changed_at"))
        .transpose()?;
    match current {
        Some(changed) => {
            tx.execute(
                "UPDATE federation_subject_clocks SET changed_at = ?, recorded_at = ? WHERE subject_key = ?",
                params![changed.max(at), now, subject],
            )?;
        }
        None => {
            tx.execute(
                "INSERT INTO federation_subject_clocks (subject_key, changed_at, recorded_at) VALUES (?, ?, ?)",
                params![subject, at, now],
            )?;
        }
    }
    Ok(())
}

/// Ключ часов участника группы (`memberClockSubject`).
#[must_use]
pub fn member_subject(config: &Config, federated: Option<&str>, member: &Value) -> Option<String> {
    let federated = federated.filter(|id| !id.is_empty())?;
    let (user, home) = identity_part(&our_origin(config), config, member)?;
    Some(js_array(&["member", federated, &user, &home]))
}

/// Ключ часов пары друзей (`friendPairClockSubject`).
#[must_use]
pub fn friend_subject(config: &Config, a: &Value, b: &Value) -> Option<String> {
    let origin = our_origin(config);
    let first = identity_part(&origin, config, a)?;
    let second = identity_part(&origin, config, b)?;
    let key = |side: &(String, String)| js_array(&[&side.0, &side.1]);
    let (low, high) = if key(&first) <= key(&second) {
        (first, second)
    } else {
        (second, first)
    };
    Some(js_array(&["friend", &low.0, &low.1, &high.0, &high.1]))
}

/// Изменение `at` старше уже известного (`isSubjectChangeStale`).
///
/// # Errors
/// Ошибка базы.
pub fn subject_stale(tx: &mut dyn Tx, subject: &str, at: i64) -> DbResult<bool> {
    Ok(tx
        .query_one(
            "SELECT changed_at FROM federation_subject_clocks WHERE subject_key = ?",
            params![subject],
        )?
        .map(|row| row.int("changed_at"))
        .transpose()?
        .is_some_and(|changed| at < changed))
}

/// Принимает изменение, если оно не устарело (`claimSubjectChange`).
///
/// # Errors
/// Ошибка базы.
pub fn claim_subject(tx: &mut dyn Tx, subject: &str, at: i64) -> DbResult<bool> {
    if subject_stale(tx, subject, at)? {
        return Ok(false);
    }
    record_subject(tx, subject, at)?;
    Ok(true)
}

fn js_array(items: &[&str]) -> String {
    Value::Array(items.iter().map(|item| json!(item)).collect()).to_string()
}

/// Изменение участника группы здесь (`recordLocalMemberChange`).
///
/// # Errors
/// Ошибка базы.
pub fn record_member_change(
    tx: &mut dyn Tx,
    config: &Config,
    federated: &str,
    member: &Value,
    at: i64,
) -> DbResult<()> {
    let origin = our_origin(config);
    let Some((user, home)) = identity_part(&origin, config, member) else {
        return Ok(());
    };
    if federated.is_empty() {
        return Ok(());
    }
    record_subject(tx, &js_array(&["member", federated, &user, &home]), at)
}

/// Изменение дружбы здесь (`recordLocalFriendPairChange`).
fn record_friend_change(
    tx: &mut dyn Tx,
    config: &Config,
    a: &Value,
    b: &Value,
    at: i64,
) -> DbResult<()> {
    let origin = our_origin(config);
    let (Some(first), Some(second)) = (
        identity_part(&origin, config, a),
        identity_part(&origin, config, b),
    ) else {
        return Ok(());
    };
    let key = |side: &(String, String)| js_array(&[&side.0, &side.1]);
    let (low, high) = if key(&first) <= key(&second) {
        (first, second)
    } else {
        (second, first)
    };
    record_subject(
        tx,
        &js_array(&["friend", &low.0, &low.1, &high.0, &high.1]),
        at,
    )
}

// ─── Сообщения ──────────────────────────────────────────────────────────────

static MENTION_SCAN: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?s)(```.*?```|`[^`]+`)|<@([a-zA-Z0-9_-]+)>").ok());

/// Идентификаторы упоминаний вне кода (`mentionTokenIds`).
pub fn mention_ids(content: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let Some(scan) = MENTION_SCAN.as_ref() else {
        return ids;
    };
    for caps in scan.captures_iter(content) {
        if let Some(id) = caps.get(2)
            && !ids.iter().any(|seen| seen == id.as_str())
        {
            ids.push(id.as_str().to_owned());
        }
    }
    ids
}

/// Упоминания для других инстансов (`relayMentionsOf`).
fn mentions(
    tx: &mut dyn Tx,
    origin: &str,
    kind: &str,
    content: Option<&str>,
) -> DbResult<Option<Value>> {
    let Some(content) = content.filter(|content| !content.is_empty() && kind != "system") else {
        return Ok(None);
    };
    let mut found = Vec::new();
    for id in mention_ids(content) {
        if found.len() >= MAX_MENTIONS {
            break;
        }
        let Some(row) = tx.query_one(
            "SELECT id, home_user_id, home_instance, is_deleted FROM users WHERE id = ?",
            params![&id],
        )?
        else {
            continue;
        };
        if row.opt_flag("is_deleted")? == Some(true) {
            continue;
        }
        let actor = match row
            .opt_text("home_instance")?
            .filter(|home| !home.is_empty())
        {
            None => json!({"homeUserId": id, "homeInstance": origin}),
            Some(home) => match row
                .opt_text("home_user_id")?
                .filter(|user| !user.is_empty())
            {
                Some(user) => json!({"homeUserId": user, "homeInstance": home}),
                None => continue,
            },
        };
        found.push(json!({"id": id, "homeUserId": actor["homeUserId"], "homeInstance": actor["homeInstance"]}));
    }
    Ok((!found.is_empty()).then_some(Value::Array(found)))
}

/// Ссылка на сообщение-ответ (`dmReplyRefForRelay`).
fn reply_ref(
    tx: &mut dyn Tx,
    config: &Config,
    channel: &str,
    reply_to: Option<&str>,
) -> DbResult<Option<Value>> {
    let Some(reply_to) = reply_to.filter(|id| !id.is_empty()) else {
        return Ok(None);
    };
    let exists = tx
        .query_one(
            "SELECT id FROM dm_messages WHERE id = ? AND dm_channel_id = ?",
            params![reply_to, channel],
        )?
        .is_some();
    if !exists {
        return Ok(None);
    }
    super::message_ref(tx, config, reply_to).map(Some)
}

/// Сообщение для других инстансов (`buildRelayPayload`).
fn relay_message(
    tx: &mut dyn Tx,
    config: &Config,
    message: &Row,
    author: Option<&Row>,
) -> DbResult<Value> {
    let origin = our_origin(config);
    let kind = message
        .opt_text("type")?
        .unwrap_or_else(|| "user".to_owned());
    let content = message.opt_text("content")?;
    let channel = message.text("dm_channel_id")?;
    let reply_to = message.opt_text("reply_to_id")?;
    let user_id = message.text("user_id")?;
    let (home_user, home_instance) = match author {
        Some(author) => (
            author.opt_text("home_user_id")?.filter(|id| !id.is_empty()),
            author
                .opt_text("home_instance")?
                .filter(|home| !home.is_empty()),
        ),
        None => (None, None),
    };
    let mut payload = Map::new();
    payload.insert("userId".to_owned(), json!(user_id));
    payload.insert(
        "homeUserId".to_owned(),
        json!(home_user.unwrap_or_else(|| user_id.clone())),
    );
    payload.insert(
        "homeInstance".to_owned(),
        json!(home_instance.unwrap_or_else(|| origin.clone())),
    );
    if kind == "system" {
        payload.insert("type".to_owned(), json!("system"));
    }
    payload.insert("content".to_owned(), json!(content));
    payload.insert("replyToId".to_owned(), json!(reply_to));
    if let Some(reference) = reply_ref(tx, config, &channel, reply_to.as_deref())? {
        payload.insert("replyTo".to_owned(), reference);
    }
    if let Some(found) = mentions(tx, &origin, &kind, content.as_deref())? {
        payload.insert("mentions".to_owned(), found);
    }
    payload.insert("editedAt".to_owned(), json!(message.opt_int("edited_at")?));
    payload.insert("createdAt".to_owned(), json!(message.int("created_at")?));
    Ok(Value::Object(payload))
}

/// Вложения сообщения для других инстансов (`FederationRelayAttachment`).
fn relay_attachments(tx: &mut dyn Tx, origin: &str, message_id: &str) -> DbResult<Vec<Value>> {
    let rows = tx.query(
        "SELECT id, filename, original_name, mimetype, size, width, height, duration, playable, \
         thumbnail_filename FROM attachments WHERE dm_message_id = ?",
        params![message_id],
    )?;
    let mut out = Vec::new();
    for row in rows {
        let filename = row.text("filename")?;
        let mut item = Map::new();
        item.insert("id".to_owned(), json!(row.text("id")?));
        item.insert("filename".to_owned(), json!(filename));
        item.insert("originalName".to_owned(), json!(row.text("original_name")?));
        item.insert("mimetype".to_owned(), json!(row.text("mimetype")?));
        item.insert("size".to_owned(), json!(row.int("size")?));
        if let Some(width) = row.opt_int("width")? {
            item.insert("width".to_owned(), json!(width));
        }
        if let Some(height) = row.opt_int("height")? {
            item.insert("height".to_owned(), json!(height));
        }
        if let Some(duration) = row.opt_number("duration")? {
            item.insert("duration".to_owned(), duration);
        }
        item.insert("playable".to_owned(), json!(row.opt_flag("playable")?));
        if let Some(thumbnail) = row.opt_text("thumbnail_filename")? {
            item.insert("thumbnailFilename".to_owned(), json!(thumbnail));
        }
        item.insert(
            "sourceUrl".to_owned(),
            json!(format!("{origin}/api/uploads/{filename}")),
        );
        out.push(Value::Object(item));
    }
    Ok(out)
}

/// Создание или правка личного сообщения (`queueDmRelay`).
///
/// # Errors
/// Ошибка базы.
pub fn dm_message(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    channel: &str,
    message_id: &str,
    update: bool,
) -> DbResult<()> {
    let kind = if update { "update" } else { "create" };
    let origin = relay.origin();
    let Some(message) = tx.query_one(
        "SELECT * FROM dm_messages WHERE id = ?",
        params![message_id],
    )?
    else {
        return Ok(());
    };
    let author = user_row(tx, &message.text("user_id")?)?;
    let attachments = relay_attachments(tx, &origin, message_id)?;
    let members = participants(tx, &origin, channel)?;
    let targets = target_origins(&origin, &members);
    let group = tx.query_one(
        "SELECT federated_id, owner_id FROM dm_channels WHERE id = ?",
        params![channel],
    )?;
    let target = if update {
        super::mutation_target(
            tx,
            relay.config,
            channel,
            message_id,
            &message.text("user_id")?,
        )?
    } else {
        None
    };
    append_mutation_log_in(tx, relay.ids, message_id, channel, "dm", kind, None)?;
    let mut payload = Map::new();
    if let Some(group) = group
        && let (Some(federated), Some(_)) = (
            group.opt_text("federated_id")?.filter(|id| !id.is_empty()),
            group.opt_text("owner_id")?.filter(|id| !id.is_empty()),
        )
    {
        payload.insert("federatedId".to_owned(), json!(federated));
    }
    if let Some(target) = target {
        payload.insert("target".to_owned(), target);
    }
    let mut wire = relay_message(tx, relay.config, &message, author.as_ref())?;
    if !attachments.is_empty()
        && let Some(fields) = wire.as_object_mut()
    {
        fields.insert("attachments".to_owned(), Value::Array(attachments));
    }
    payload.insert("message".to_owned(), wire);
    payload.insert("participants".to_owned(), Value::Array(members));
    outbox::queue(
        tx,
        relay.ids,
        &Event {
            entity_id: message_id,
            context_id: channel,
            event_type: kind,
            payload: &Value::Object(payload),
            targets: Some(&targets),
            context_type: "dm",
        },
    )
}

/// Удаление личного сообщения (`queueDmMessageDeleteRelay`).
///
/// # Errors
/// Ошибка базы.
pub fn dm_delete(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    channel: &str,
    message_id: &str,
    target: Option<Value>,
) -> DbResult<()> {
    let origin = relay.origin();
    let log = target.as_ref().map(|target| json!({"target": target}));
    let mut payload = json!({"deleted": true});
    if let (Some(target), Some(fields)) = (target, payload.as_object_mut()) {
        fields.insert("target".to_owned(), target);
    }
    let targets = dm_targets(tx, &origin, channel)?;
    relay.log_and_queue(
        tx,
        message_id,
        channel,
        "delete",
        log.as_ref(),
        &payload,
        Some(&targets),
        "dm",
    )
}

/// Реакция на личное сообщение поставлена или снята.
///
/// # Errors
/// Ошибка базы.
#[allow(clippy::too_many_arguments, reason = "поля реакции")]
pub fn reaction(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    channel: &str,
    message_id: &str,
    user_id: &str,
    emoji: &str,
    added: Option<(&str, i64)>,
) -> DbResult<()> {
    let origin = relay.origin();
    let person = identity(tx, &origin, user_id)?;
    let reference = super::message_ref(tx, relay.config, message_id)?;
    let mut log = json!({
        "userId": user_id,
        "homeUserId": person["homeUserId"],
        "homeInstance": person["homeInstance"],
        "emoji": emoji,
    });
    let mut reaction = json!({
        "messageId": reference["messageId"],
        "messageHomeInstance": reference["messageHomeInstance"],
        "userId": user_id,
        "homeUserId": person["homeUserId"],
        "homeInstance": person["homeInstance"],
        "emoji": emoji,
    });
    let targets = dm_targets(tx, &origin, channel)?;
    match added {
        Some((reaction_id, created_at)) => {
            log["createdAt"] = json!(created_at);
            reaction["createdAt"] = json!(created_at);
            append_mutation_log_in(
                tx,
                relay.ids,
                message_id,
                channel,
                "dm",
                "reaction_add",
                Some(&log),
            )?;
            outbox::queue(
                tx,
                relay.ids,
                &Event {
                    entity_id: reaction_id,
                    context_id: channel,
                    event_type: "reaction_add",
                    payload: &json!({"reaction": reaction}),
                    targets: Some(&targets),
                    context_type: "dm",
                },
            )
        }
        None => {
            append_mutation_log_in(
                tx,
                relay.ids,
                message_id,
                channel,
                "dm",
                "reaction_remove",
                Some(&log),
            )?;
            outbox::queue(
                tx,
                relay.ids,
                &Event {
                    entity_id: &format!("{message_id}:{user_id}:{emoji}"),
                    context_id: channel,
                    event_type: "reaction_remove",
                    payload: &json!({"reaction": reaction}),
                    targets: Some(&targets),
                    context_type: "dm",
                },
            )
        }
    }
}

/// Позиция чтения переписки (`queueReadStateRelay`).
///
/// # Errors
/// Ошибка базы.
pub fn read_state(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    channel: &str,
    message_id: &str,
    user_id: &str,
) -> DbResult<()> {
    if !relay_enabled(tx)? {
        return Ok(());
    }
    let Some(federated) = dm_federated_id(tx, channel)? else {
        return Ok(());
    };
    let Some(user) = user_row(tx, user_id)? else {
        return Ok(());
    };
    let origin = relay.origin();
    let person = identity_of(&origin, &user)?;
    let reference = super::message_ref(tx, relay.config, message_id)?;
    let state = json!({
        "user": person,
        "messageRef": {
            "sourceInstance": reference["messageHomeInstance"],
            "sourceMessageId": reference["messageId"],
        },
    });
    let now = now_ms();
    let payload = json!({
        "eventType": "read_state_update",
        "dmChannelId": channel,
        "messageId": format!("read_state:{user_id}:{now}"),
        "federatedId": federated,
        "encryptionVersion": 0,
        "timestamp": now,
        "readState": state,
    });
    let targets = dm_targets(tx, &origin, channel)?;
    relay.log_and_queue(
        tx,
        &format!("read_state:{federated}:{user_id}"),
        channel,
        "read_state_update",
        Some(&state),
        &payload,
        Some(&targets),
        "dm",
    )
}

/// Переписку закрыли или открыли снова (`queueDmCloseRelay`).
///
/// # Errors
/// Ошибка базы.
pub fn dm_close(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    channel: &str,
    user_id: &str,
    reopen: bool,
) -> DbResult<()> {
    if !relay_enabled(tx)? {
        return Ok(());
    }
    let Some(federated) = dm_federated_id(tx, channel)? else {
        return Ok(());
    };
    let Some(user) = user_row(tx, user_id)? else {
        return Ok(());
    };
    let origin = relay.origin();
    let kind = if reopen { "dm_reopen" } else { "dm_close" };
    let person = identity_of(&origin, &user)?;
    let now = now_ms();
    let payload = json!({
        "eventType": kind,
        "dmChannelId": channel,
        "messageId": format!("{kind}:{federated}:{user_id}:{now}"),
        "federatedId": federated,
        "encryptionVersion": 0,
        "timestamp": now,
        "dmCloseReopen": person,
    });
    let targets = dm_targets(tx, &origin, channel)?;
    relay.log_and_queue(
        tx,
        &format!("{kind}:{federated}:{user_id}"),
        channel,
        kind,
        Some(&person),
        &payload,
        Some(&targets),
        "dm",
    )
}

// ─── Группы ─────────────────────────────────────────────────────────────────

/// Название и значок группы изменены (`queueGroupMetadataRelay`).
///
/// # Errors
/// Ошибка базы.
pub fn group_metadata(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    channel: &str,
    actor_id: &str,
    name: Option<&str>,
    icon: Option<&str>,
    updated_at: i64,
) -> DbResult<()> {
    if !relay_enabled(tx)? {
        return Ok(());
    }
    let Some(federated) = dm_federated_id(tx, channel)? else {
        tracing::warn!(
            channel,
            "у группы нет общего ключа: изменение не пересылается"
        );
        return Ok(());
    };
    let origin = relay.origin();
    let targets = dm_targets(tx, &origin, channel)?;
    if targets.is_empty() {
        return Ok(());
    }
    let Some(actor) = user_row(tx, actor_id)? else {
        tracing::warn!(actor = actor_id, "автор изменения группы не найден");
        return Ok(());
    };
    let metadata = json!({
        "name": name,
        "icon": wire_icon(icon.map(str::to_owned), &origin),
        "metadataUpdatedAt": updated_at,
        "actor": participant(&origin, &actor)?,
    });
    let entity = format!("group_metadata:{federated}:{updated_at}");
    relay.log_and_queue(
        tx,
        &entity,
        channel,
        "group_metadata_update",
        Some(&metadata),
        &json!({"federatedId": federated, "metadata": metadata}),
        Some(&targets),
        "dm",
    )
}

/// Участника добавили в группу (`member_add`): `added_by` — кто добавил,
/// `owner` — владелец для получателя.
///
/// # Errors
/// Ошибка базы.
#[allow(clippy::too_many_arguments, reason = "поля события")]
pub fn member_add(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    channel: &str,
    target_id: &str,
    added_by: &Value,
    owner: &Value,
    at: i64,
) -> DbResult<()> {
    let Some(federated) = dm_federated_id(tx, channel)? else {
        return Ok(());
    };
    let origin = relay.origin();
    let members = participants(tx, &origin, channel)?;
    let group = tx.query_one(
        "SELECT name, icon, metadata_updated_at FROM dm_channels WHERE id = ?",
        params![channel],
    )?;
    let (name, icon, updated) = match &group {
        Some(row) => (
            row.opt_text("name")?,
            row.opt_text("icon")?,
            row.opt_int("metadata_updated_at")?.unwrap_or(0),
        ),
        None => (None, None, 0),
    };
    let target = user_row(tx, target_id)?;
    let user = match &target {
        Some(row) => identity_of(&origin, row)?,
        None => json!({"homeUserId": target_id, "homeInstance": origin}),
    };
    let payload = json!({
        "eventType": "member_add",
        "dmChannelId": channel,
        "messageId": format!("member_add:{target_id}:{at}"),
        "federatedId": federated,
        "encryptionVersion": 0,
        "timestamp": at,
        "membership": {"user": user, "addedBy": added_by},
        "group": {
            "owner": owner,
            "members": members,
            "name": name,
            "icon": wire_icon(icon, &origin),
            "metadataUpdatedAt": updated,
        },
    });
    let mut targets = target_origins(&origin, &members);
    let raw = target
        .as_ref()
        .map(|row| row.opt_text("home_instance"))
        .transpose()?
        .flatten()
        .filter(|home| !home.is_empty())
        .unwrap_or_else(|| origin.clone());
    let new_home = if raw.starts_with("http") {
        raw
    } else {
        format!("https://{raw}")
    };
    if new_home != origin && !targets.contains(&new_home) {
        targets.push(new_home);
    }
    let entity = format!("member_add:{target_id}:{at}");
    relay.log_and_queue(
        tx,
        &entity,
        channel,
        "member_add",
        Some(&payload),
        &payload,
        Some(&targets),
        "dm",
    )
}

/// Идентичность пользователя для событий групп.
///
/// # Errors
/// Ошибка базы.
pub fn identity_value(tx: &mut dyn Tx, config: &Config, user_id: &str) -> DbResult<Value> {
    identity(tx, &our_origin(config), user_id)
}

/// Участника убрали из группы (`member_remove`). `targets` — инстансы
/// участников до удаления.
///
/// # Errors
/// Ошибка базы.
#[allow(clippy::too_many_arguments, reason = "поля события")]
pub fn member_remove(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    channel: &str,
    federated: &str,
    removed: &Value,
    removed_id: &str,
    actor: &Value,
    reason: &str,
    at: i64,
    targets: &[String],
) -> DbResult<()> {
    let payload = json!({
        "eventType": "member_remove",
        "dmChannelId": channel,
        "messageId": format!("member_remove:{removed_id}:{at}"),
        "federatedId": federated,
        "encryptionVersion": 0,
        "timestamp": at,
        "membership": {"user": removed, "removedBy": actor, "reason": reason},
    });
    let entity = format!("member_remove:{removed_id}:{at}");
    relay.log_and_queue(
        tx,
        &entity,
        channel,
        "member_remove",
        Some(&payload),
        &payload,
        Some(targets),
        "dm",
    )
}

/// Владение группой передано (`ownership_transfer`).
///
/// # Errors
/// Ошибка базы.
#[allow(clippy::too_many_arguments, reason = "поля события")]
pub fn ownership_transfer(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    channel: &str,
    federated: &str,
    new_owner_id: &str,
    new_owner: &Value,
    previous_owner: &Value,
    at: i64,
    targets: &[String],
) -> DbResult<()> {
    let payload = json!({
        "eventType": "ownership_transfer",
        "dmChannelId": channel,
        "messageId": format!("ownership_transfer:{new_owner_id}:{at}"),
        "federatedId": federated,
        "encryptionVersion": 0,
        "timestamp": at,
        "ownership": {"newOwner": new_owner, "previousOwner": previous_owner},
    });
    let entity = format!("ownership_transfer:{new_owner_id}:{at}");
    relay.log_and_queue(
        tx,
        &entity,
        channel,
        "ownership_transfer",
        Some(&payload),
        &payload,
        Some(targets),
        "dm",
    )
}

// ─── Дружба ─────────────────────────────────────────────────────────────────

/// Профиль в событиях дружбы (`buildProfileSnapshot`).
fn profile_snapshot(user: &Row) -> DbResult<Value> {
    if user.opt_flag("is_deleted")? == Some(true) {
        return Ok(json!({"deleted": true}));
    }
    let native = user
        .opt_text("home_instance")?
        .is_none_or(|home| home.is_empty());
    let status = if native {
        user.opt_text("status")?.filter(|status| !status.is_empty())
    } else {
        None
    };
    Ok(json!({
        "username": relay_handle(user)?,
        "displayName": user.opt_text("display_name")?,
        "avatar": user.opt_text("avatar")?,
        "avatarColor": user.opt_text("avatar_color")?,
        "banner": user.opt_text("banner")?,
        "bio": user.opt_text("bio")?,
        "status": status,
    }))
}

/// Вид события дружбы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Friendship<'a> {
    /// Запрос отправлен.
    Request,
    /// На запрос ответили: `accepted` или `declined`; при согласии следом
    /// уходит `friend_add`. Время создания запроса.
    Answer(&'a str, i64),
    /// Запрос отозван; время его создания.
    Cancel(i64),
    /// Дружба прекращена.
    Remove,
}

fn instance_origin(home: &str) -> String {
    if home.starts_with("http") {
        home.to_owned()
    } else {
        format!("https://{home}")
    }
}

/// Запрос дружбы пользователю другого инстанса после поиска на нём
/// (`handleFederatedFriendRequest`). Возвращает идентификатор события.
///
/// # Errors
/// Ошибка базы.
pub fn remote_friend_request(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    sender: &Row,
    stub: &Row,
    peer_origin: &str,
    profile: &Value,
    now: i64,
) -> DbResult<String> {
    let origin = relay.origin();
    let from_id = sender
        .opt_text("home_user_id")?
        .filter(|id| !id.is_empty())
        .map_or_else(|| sender.text("id"), Ok)?;
    let to_id = stub.opt_text("home_user_id")?.unwrap_or_default();
    let from_identity = json!({"homeUserId": from_id, "homeInstance": origin});
    let to_identity = json!({"homeUserId": to_id, "homeInstance": peer_origin});
    let (low, high) = if from_id <= to_id {
        (&from_id, &to_id)
    } else {
        (&to_id, &from_id)
    };
    let context = format!("friend:{low}:{high}");
    let entity = format!("friend_req:{low}:{high}:{now}");
    let field = |key: &str| profile.get(key).cloned().unwrap_or(Value::Null);
    let status = profile
        .get("status")
        .filter(|v| !v.is_null())
        .cloned()
        .unwrap_or(Value::Null);
    let payload = json!({
        "eventType": "friend_request_create",
        "contextType": "friend",
        "messageId": entity,
        "encryptionVersion": 0,
        "timestamp": now,
        "friendship": {
            "from": from_identity,
            "to": to_identity,
            "fromProfile": profile_snapshot(sender)?,
            "toProfile": {
                "username": stub.text("username")?,
                "displayName": field("displayName"),
                "avatar": field("avatar"),
                "avatarColor": field("avatarColor"),
                "banner": field("banner"),
                "bio": field("bio"),
                "status": status,
            },
            "status": "pending",
            "createdAt": now,
        },
    });
    record_friend_change(tx, relay.config, &from_identity, &to_identity, now)?;
    let targets = [peer_origin.to_owned()];
    relay.log_and_queue(
        tx,
        &entity,
        &context,
        "friend_request_create",
        Some(&payload),
        &payload,
        Some(&targets),
        "friend",
    )?;
    Ok(entity)
}

/// Событие дружбы между `from` и `to` (`routes/social.ts`). Возвращает
/// идентификатор события запроса — под ним его знает получатель.
///
/// # Errors
/// Ошибка базы.
pub fn friendship(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    kind: Friendship<'_>,
    from_id: &str,
    to_id: &str,
) -> DbResult<Option<String>> {
    let origin = relay.origin();
    let (Some(from), Some(to)) = (user_row(tx, from_id)?, user_row(tx, to_id)?) else {
        return Ok(None);
    };
    let from_identity = identity_of(&origin, &from)?;
    let to_identity = identity_of(&origin, &to)?;
    let mut targets = Vec::new();
    for side in [&from_identity, &to_identity] {
        let home = instance_origin(side["homeInstance"].as_str().unwrap_or(&origin));
        if home != origin && !targets.contains(&home) {
            targets.push(home);
        }
    }
    if targets.is_empty() {
        return Ok(None);
    }
    let a = from_identity["homeUserId"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let b = to_identity["homeUserId"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let (low, high) = if a <= b { (&a, &b) } else { (&b, &a) };
    let context = format!("friend:{low}:{high}");
    let now = now_ms();
    record_friend_change(tx, relay.config, &from_identity, &to_identity, now)?;
    let event = |kind: &str, entity: &str, friendship: Value| {
        json!({
            "eventType": kind,
            "contextType": "friend",
            "messageId": entity,
            "encryptionVersion": 0,
            "timestamp": now,
            "friendship": friendship,
        })
    };
    let queue = |tx: &mut dyn Tx, kind: &str, entity: &str, payload: &Value| {
        relay.log_and_queue(
            tx,
            entity,
            &context,
            kind,
            Some(payload),
            payload,
            Some(&targets),
            "friend",
        )
    };
    match kind {
        Friendship::Request => {
            let entity = format!("friend_req:{low}:{high}:{now}");
            let payload = event(
                "friend_request_create",
                &entity,
                json!({
                    "from": from_identity,
                    "to": to_identity,
                    "fromProfile": profile_snapshot(&from)?,
                    "toProfile": profile_snapshot(&to)?,
                    "status": "pending",
                    "createdAt": now,
                }),
            );
            queue(tx, "friend_request_create", &entity, &payload)?;
            Ok(Some(entity))
        }
        Friendship::Answer(status, created_at) => {
            let entity = format!("friend_req:{low}:{high}:{now}");
            let payload = event(
                "friend_request_update",
                &entity,
                json!({
                    "from": from_identity,
                    "to": to_identity,
                    "fromProfile": profile_snapshot(&from)?,
                    "toProfile": profile_snapshot(&to)?,
                    "status": status,
                    "createdAt": created_at,
                }),
            );
            queue(tx, "friend_request_update", &entity, &payload)?;
            if status == "accepted" {
                let entity = format!("friend:{low}:{high}:{now}");
                let payload = event(
                    "friend_add",
                    &entity,
                    json!({
                        "from": from_identity,
                        "to": to_identity,
                        "fromProfile": profile_snapshot(&from)?,
                        "toProfile": profile_snapshot(&to)?,
                        "createdAt": now,
                    }),
                );
                queue(tx, "friend_add", &entity, &payload)?;
            }
            Ok(Some(entity))
        }
        Friendship::Cancel(created_at) => {
            let entity = format!("friend_req:{low}:{high}:{now}");
            let payload = event(
                "friend_request_cancel",
                &entity,
                json!({"from": from_identity, "to": to_identity, "createdAt": created_at}),
            );
            queue(tx, "friend_request_cancel", &entity, &payload)?;
            Ok(Some(entity))
        }
        Friendship::Remove => {
            let entity = format!("friend:{low}:{high}:{now}");
            let payload = event(
                "friend_remove",
                &entity,
                json!({"from": from_identity, "to": to_identity, "createdAt": now}),
            );
            queue(tx, "friend_remove", &entity, &payload)?;
            Ok(Some(entity))
        }
    }
}

// ─── Профиль и присутствие ──────────────────────────────────────────────────

/// Профиль пользователя изменён. `targets` — `None`: всем пирам; иначе
/// по событию каждому из перечисленных инстансов (новые инстансы реплик).
///
/// # Errors
/// Ошибка базы.
pub fn profile_update(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    user: &Row,
    targets: Option<&[String]>,
) -> DbResult<()> {
    let payload = super::profile_payload(relay.config, user)?;
    let id = user.text("id")?;
    append_mutation_log_in(
        tx,
        relay.ids,
        &id,
        &id,
        "profile",
        "profile_update",
        Some(&payload),
    )?;
    match targets {
        None => outbox::queue(
            tx,
            relay.ids,
            &Event {
                entity_id: &id,
                context_id: &id,
                event_type: "profile_update",
                payload: &payload,
                targets: None,
                context_type: "profile",
            },
        ),
        Some(targets) => {
            for target in targets {
                outbox::queue(
                    tx,
                    relay.ids,
                    &Event {
                        entity_id: &id,
                        context_id: &id,
                        event_type: "profile_update",
                        payload: &payload,
                        targets: Some(std::slice::from_ref(target)),
                        context_type: "profile",
                    },
                )?;
            }
            Ok(())
        }
    }
}

fn presence_event(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    user_id: &str,
    status: &str,
    activities: &[Value],
    targets: Option<&[String]>,
    suffix: &str,
) -> DbResult<()> {
    let now = now_ms();
    let payload = json!({
        "eventType": "presence_update",
        "contextType": "profile",
        "messageId": format!("presence:{user_id}:{now}{suffix}"),
        "encryptionVersion": 0,
        "timestamp": now,
        "presenceUpdate": {
            "homeUserId": user_id,
            "homeInstance": relay.origin(),
            "status": status,
            "ts": now,
            "activities": activities,
        },
    });
    outbox::queue(
        tx,
        relay.ids,
        &Event {
            entity_id: user_id,
            context_id: user_id,
            event_type: "presence_update",
            payload: &payload,
            targets,
            context_type: "profile",
        },
    )
}

/// Присутствие своего пользователя изменилось (`queuePresenceRelay`).
///
/// # Errors
/// Ошибка базы.
pub fn presence(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    user_id: &str,
    status: &str,
    activities: &[Value],
) -> DbResult<()> {
    if !relay_enabled(tx)? {
        return Ok(());
    }
    let Some(user) = user_row(tx, user_id)? else {
        return Ok(());
    };
    if user
        .opt_text("home_instance")?
        .is_some_and(|home| !home.is_empty())
    {
        return Ok(());
    }
    presence_event(tx, relay, user_id, status, activities, None, "")
}

/// Новому другу с другого инстанса — текущее присутствие
/// (`snapshotPresenceForFriend`).
///
/// # Errors
/// Ошибка базы.
pub fn presence_for_friend(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    native_id: &str,
    friend_id: &str,
    activities: &[Value],
) -> DbResult<()> {
    if !relay_enabled(tx)? {
        return Ok(());
    }
    let (Some(native), Some(friend)) = (user_row(tx, native_id)?, user_row(tx, friend_id)?) else {
        return Ok(());
    };
    if native
        .opt_text("home_instance")?
        .is_some_and(|home| !home.is_empty())
    {
        return Ok(());
    }
    let Some(home) = friend
        .opt_text("home_instance")?
        .filter(|home| !home.is_empty())
    else {
        return Ok(());
    };
    let Some(status) = native
        .opt_text("status")?
        .filter(|status| !status.is_empty() && status != "offline")
    else {
        return Ok(());
    };
    let target = instance_origin(&home);
    if target == relay.origin() {
        return Ok(());
    }
    presence_event(
        tx,
        relay,
        native_id,
        &status,
        activities,
        Some(&[target]),
        ":friend",
    )
}

/// Новому пиру — присутствие наших пользователей, которые с ним связаны
/// (`snapshotPresenceForPeer`).
///
/// # Errors
/// Ошибка базы.
pub fn presence_for_peer(
    tx: &mut dyn Tx,
    relay: Relay<'_>,
    hub: &crate::hub::Hub,
    peer_origin: &str,
) -> DbResult<()> {
    if !relay_enabled(tx)? {
        return Ok(());
    }
    let domain = super::reset::domain_of(peer_origin);
    let stubs: Vec<String> = tx
        .query(
            "SELECT id FROM users WHERE home_instance = ? AND is_deleted = 0",
            params![&domain],
        )?
        .iter()
        .map(|row| row.text("id"))
        .collect::<DbResult<_>>()?;
    let mut related = std::collections::HashSet::new();
    for stub in &stubs {
        for row in tx.query(
            "SELECT user_id, friend_id FROM friends WHERE user_id = ? OR friend_id = ?",
            params![stub, stub],
        )? {
            let (a, b) = (row.text("user_id")?, row.text("friend_id")?);
            related.insert(if &a == stub { b } else { a });
        }
        for row in tx.query(
            "SELECT m.user_id FROM dm_members s INNER JOIN dm_members m ON m.dm_channel_id = s.dm_channel_id \
             WHERE s.user_id = ? AND s.closed = 0 AND m.closed = 0",
            params![stub],
        )? {
            related.insert(row.text("user_id")?);
        }
    }
    let natives = tx.query(
        "SELECT id, status, replicated_instances FROM users WHERE home_instance IS NULL AND is_deleted = 0",
        params![],
    )?;
    for user in &natives {
        let Some(list) = user.opt_text("replicated_instances")? else {
            continue;
        };
        if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&list)
            && items
                .iter()
                .any(|item| item["origin"].as_str() == Some(peer_origin))
        {
            related.insert(user.text("id")?);
        }
    }
    for user in natives {
        let id = user.text("id")?;
        if !related.contains(&id) {
            continue;
        }
        let Some(status) = user
            .opt_text("status")?
            .filter(|status| !status.is_empty() && status != "offline")
        else {
            continue;
        };
        let activities = hub.activities(&id);
        presence_event(
            tx,
            relay,
            &id,
            &status,
            &activities,
            Some(&[peer_origin.to_owned()]),
            ":snap",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_skip_code() {
        assert_eq!(
            mention_ids("hi <@a1> `<@b2>` ```x\n<@c3>``` <@d-4> <@a1> <@bad!>"),
            vec!["a1".to_owned(), "d-4".to_owned()]
        );
        // Незакрытый блок кода не прячет упоминание.
        assert_eq!(mention_ids("``` <@a>"), vec!["a".to_owned()]);
    }

    #[test]
    fn handles_follow_the_hint_rules() {
        assert_eq!(handle_from_hint(" Kai_1 ").as_deref(), Some("kai_1"));
        assert_eq!(handle_from_hint("kai.1"), None);
        assert_eq!(handle_from_hint(""), None);
    }

    #[test]
    fn subject_keys_are_json_arrays() {
        assert_eq!(
            js_array(&["member", "f", "u", "a.example"]),
            r#"["member","f","u","a.example"]"#
        );
    }
}
