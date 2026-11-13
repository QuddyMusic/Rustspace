//! Сообщения, правки, удаления и реакции из других инстансов
//! (`routes/federation/events/dmMessages.ts`, `routes/federation/dmChannels.ts`,
//! `utils/federationMentions.ts`).

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde_json::{Map, Value, json};
use url::Url;

use super::{
    Ctx, Delivery, Step, accept, actor, has_applied, hints_of, live_channel, millis, non_empty,
    present, profile, record_applied, reject, text, truthy, user_row,
};
use crate::db::{DbResult, Row, Tx};
use crate::federation::identity::{self, Actor, Resolved};
use crate::federation::{dm_target_origins, our_origin};
use crate::federation_auth::normalize_origin;
use crate::params;
use crate::routes::users::sanitize_user;
use crate::state::now_ms;
use crate::util::utf16_len;

const MAX_MENTIONS: usize = 100;
const MAX_MENTION_FIELD: usize = 255;
const FILE_QUEUE_TTL_MS: i64 = 30 * 86_400_000;

static MENTION_SCAN: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?s)(```.*?```|`[^`]+`)|<@([a-zA-Z0-9_-]+)>").ok());
static MENTION_ID: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new("^[a-zA-Z0-9_-]+$").ok());

// ─── Общее с другими обработчиками (`dmChannels.ts`) ───────────────────────

/// Участник переписки: идентификатор и дом.
#[derive(Debug, Clone)]
pub struct Member {
    /// Локальный идентификатор.
    pub id: String,
    /// Идентификатор дома.
    pub home_user_id: Option<String>,
    /// Дом.
    pub home_instance: Option<String>,
}

impl Member {
    /// Из строки пользователя.
    ///
    /// # Errors
    /// В строке нет нужных столбцов.
    pub fn of(row: &Row) -> DbResult<Self> {
        Ok(Self {
            id: row.text("id")?,
            home_user_id: row.opt_text("home_user_id")?,
            home_instance: row.opt_text("home_instance")?,
        })
    }

    fn identity(&self, ours: &str) -> Option<Actor> {
        match self
            .home_instance
            .as_deref()
            .filter(|home| !home.is_empty())
        {
            None => Some(Actor {
                home_user_id: self.id.clone(),
                home_instance: ours.to_owned(),
            }),
            Some(home) => self
                .home_user_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .map(|id| Actor {
                    home_user_id: id.to_owned(),
                    home_instance: home.to_owned(),
                }),
        }
    }
}

/// Участники переписки (`dmChannelMembers`).
///
/// # Errors
/// Ошибка базы.
pub fn members(tx: &mut dyn Tx, channel: &str) -> DbResult<Vec<Member>> {
    tx.query(
        "SELECT u.id, u.home_user_id, u.home_instance FROM dm_members m \
         JOIN users u ON u.id = m.user_id WHERE m.dm_channel_id = ?",
        params![channel],
    )?
    .iter()
    .map(Member::of)
    .collect()
}

/// Участник с этой идентичностью (`memberWithIdentity`).
#[must_use]
pub fn member_with_identity<'a>(
    members: &'a [Member],
    ours: &str,
    actor: &Actor,
) -> Option<&'a Member> {
    members.iter().find(|member| {
        member
            .identity(ours)
            .is_some_and(|identity| identity::same_actor(&identity, actor))
    })
}

/// Инстансы, которым уходит переписка с такими участниками
/// (`relayTargetOrigins`).
#[must_use]
pub fn target_origins(members: &[Member], ours: &str) -> Vec<String> {
    let mut origins: Vec<String> = Vec::new();
    for member in members {
        let home = member
            .home_instance
            .as_deref()
            .filter(|home| !home.is_empty())
            .unwrap_or(ours);
        let normalized = if home.starts_with("http") {
            home.to_owned()
        } else {
            format!("https://{home}")
        };
        if normalized != ours && !origins.contains(&normalized) {
            origins.push(normalized);
        }
    }
    origins
}

/// Входит ли источник в эти инстансы (`isRelayTarget`).
#[must_use]
pub fn is_relay_target(origins: &[String], source: &str) -> bool {
    let source = super::domain(source);
    !source.is_empty() && origins.iter().any(|origin| super::domain(origin) == source)
}

/// Может ли источник писать в переписку от имени участника (`mayRelayInto`).
#[must_use]
pub fn may_relay_into(members: &[Member], ours: &str, actor_id: &str, source: &str) -> bool {
    members.iter().any(|member| member.id == actor_id)
        && is_relay_target(&target_origins(members, ours), source)
}

/// Отказ для не-участника (`nonMemberRefusal`).
///
/// # Errors
/// В строке нет столбца.
pub fn non_member_refusal(channel: &Row) -> DbResult<&'static str> {
    Ok(
        if channel
            .opt_text("owner_id")?
            .is_some_and(|id| !id.is_empty())
        {
            "unauthorized_source"
        } else {
            "invalid_target"
        },
    )
}

/// Адрес на том же узле, что и пир (`isUrlFromPeer`).
#[must_use]
pub fn is_url_from_peer(url: &str, peer: &str) -> bool {
    match (Url::parse(url), Url::parse(peer)) {
        (Ok(url), Ok(peer)) => url.host_str().is_some() && url.host_str() == peer.host_str(),
        _ => false,
    }
}

/// Наше сообщение по ссылке пира (`resolveLocalDmMessage`).
///
/// # Errors
/// Ошибка базы.
pub fn resolve_local_message(
    tx: &mut dyn Tx,
    ours: &str,
    message_id: &str,
    home: Option<&str>,
    source: &str,
) -> DbResult<Option<Row>> {
    let columns = crate::dm::DM_MESSAGE_COLUMNS;
    let home = home.filter(|home| !home.is_empty());
    if home == Some(ours) {
        return tx.query_one(
            &format!("SELECT {columns} FROM dm_messages WHERE id = ? AND source_instance IS NULL"),
            params![message_id],
        );
    }
    let origin = home.unwrap_or(source);
    tx.query_one(
        &format!(
            "SELECT {columns} FROM dm_messages WHERE source_instance = ? AND source_message_id = ?"
        ),
        params![origin, message_id],
    )
}

fn reply_target(
    tx: &mut dyn Tx,
    ours: &str,
    reference: Option<&Value>,
    source: &str,
    channel: &str,
) -> DbResult<Option<String>> {
    let Some(reference) = reference else {
        return Ok(None);
    };
    let (Some(id), Some(home)) = (
        text(reference, "messageId"),
        text(reference, "messageHomeInstance"),
    ) else {
        return Ok(None);
    };
    let Some(target) = resolve_local_message(tx, ours, id, Some(home), source)? else {
        return Ok(None);
    };
    (target.text("dm_channel_id")? == channel)
        .then(|| target.text("id"))
        .transpose()
}

// ─── Упоминания (`federationMentions.ts`) ───────────────────────────────────

fn mention_ids(content: &str) -> Vec<String> {
    crate::federation::relay::mention_ids(content)
}

fn bounded(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && utf16_len(text) <= MAX_MENTION_FIELD)
}

/// Упоминания из события по их идентификатору у отправителя
/// (`readRelayedMentions`).
fn read_mentions(raw: Option<&Value>) -> HashMap<String, Actor> {
    let mut by_id = HashMap::new();
    let Some(list) = raw.and_then(Value::as_array) else {
        return by_id;
    };
    for entry in list.iter().take(MAX_MENTIONS) {
        let Some(id) = bounded(entry.get("id")) else {
            continue;
        };
        if !MENTION_ID.as_ref().is_some_and(|re| re.is_match(id)) {
            continue;
        }
        let (Some(home_user), Some(home)) = (
            bounded(entry.get("homeUserId")),
            bounded(entry.get("homeInstance")),
        ) else {
            continue;
        };
        by_id.entry(id.to_owned()).or_insert_with(|| Actor {
            home_user_id: home_user.to_owned(),
            home_instance: home.to_owned(),
        });
    }
    by_id
}

/// Подставляет наши идентификаторы в упоминания (`rewriteRelayedMentions`).
///
/// # Errors
/// Ошибка базы.
pub fn rewrite_mentions(
    cx: &mut Ctx<'_>,
    content: Option<&str>,
    raw: Option<&Value>,
) -> DbResult<Option<String>> {
    let Some(content) = content.filter(|content| !content.is_empty()) else {
        return Ok(content.map(str::to_owned));
    };
    if raw.is_none() {
        return Ok(Some(content.to_owned()));
    }
    let listed = read_mentions(raw);
    if listed.is_empty() {
        return Ok(Some(content.to_owned()));
    }
    let mut local: HashMap<String, String> = HashMap::new();
    for id in mention_ids(content) {
        let Some(mention) = listed.get(&id) else {
            continue;
        };
        if let Resolved::Found(user) = identity::resolve_actor(cx.tx, &cx.app.config, mention)? {
            local.insert(id, user.text("id")?);
        }
    }
    if local.is_empty() {
        return Ok(Some(content.to_owned()));
    }
    let Some(scan) = MENTION_SCAN.as_ref() else {
        return Ok(Some(content.to_owned()));
    };
    let replaced = scan.replace_all(content, |caps: &Captures<'_>| {
        let whole = caps.get(0).map_or("", |m| m.as_str());
        if caps.get(1).is_some() {
            return whole.to_owned();
        }
        caps.get(2)
            .and_then(|id| local.get(id.as_str()))
            .map_or_else(|| whole.to_owned(), |id| format!("<@{id}>"))
    });
    Ok(Some(replaced.into_owned()))
}

// ─── Системные сообщения (`parseDmSystemEvent`) ─────────────────────────────

fn is_http_origin(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &value[8..]
    } else if lower.starts_with("http://") {
        &value[7..]
    } else {
        return false;
    };
    if rest.is_empty()
        || rest
            .chars()
            .any(|c| c.is_ascii() && !('!'..='~').contains(&c))
        || rest.contains(['/', '?', '#', '@', '\\', '%'])
        || rest.ends_with(':')
    {
        return false;
    }
    Url::parse(value).is_ok()
}

/// Каноническая запись приглашения в пространство или `None`
/// (`parseSpaceInvite`).
fn nonempty_str(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|t| !t.is_empty())
}

fn canonical_space_invite(content: Option<&str>) -> Option<String> {
    let text = nonempty_str;
    let data: Value = serde_json::from_str(content.filter(|c| !c.is_empty())?).ok()?;
    let data = data.as_object()?;
    if data.get("event").and_then(Value::as_str) != Some("space_invite") {
        return None;
    }
    let optional = |value: Option<&Value>| match value {
        Some(Value::Null) => Some(Value::Null),
        Some(Value::String(text)) => Some(json!(text)),
        _ => None,
    };
    let space_id = text(data.get("spaceId"))?;
    let origin = text(data.get("spaceInstanceOrigin")).filter(|o| is_http_origin(o))?;
    let code = text(data.get("inviteCode"))?;
    let snapshot = data.get("snapshot")?.as_object()?;
    let space_name = text(snapshot.get("spaceName"))?;
    let icon = optional(snapshot.get("icon"))?;
    let description = optional(snapshot.get("description"))?;
    let instance_name = snapshot.get("instanceName")?.as_str()?;
    let count = snapshot.get("memberCount")?.as_f64()?;
    if count.fract() != 0.0 || count < 0.0 || !count.is_finite() {
        return None;
    }
    let color = snapshot
        .get("avatarColor")
        .and_then(Value::as_str)
        .filter(|color| crate::routes::auth::AVATAR_COLORS.contains(color))
        .map_or(Value::Null, |color| json!(color));
    #[allow(clippy::cast_possible_truncation, reason = "целое проверено выше")]
    let count = if count <= 9_007_199_254_740_991.0 {
        json!(count as i64)
    } else {
        json!(count)
    };
    let inner = crate::dm::system_content(&[
        ("spaceName", json!(space_name)),
        ("icon", icon),
        ("avatarColor", color),
        ("memberCount", count),
        ("description", description),
        ("instanceName", json!(instance_name)),
    ]);
    let outer = crate::dm::system_content(&[
        ("event", json!("space_invite")),
        ("spaceId", json!(space_id)),
        ("spaceInstanceOrigin", json!(origin)),
        ("inviteCode", json!(code)),
    ]);
    Some(format!(
        "{},\"snapshot\":{inner}}}",
        outer.strip_suffix('}').unwrap_or(&outer)
    ))
}

// ─── create ─────────────────────────────────────────────────────────────────

/// Открывает переписку закрывшим её участникам (`reopenClosedDmMembers`).
fn reopen_closed(
    tx: &mut dyn Tx,
    channel: &str,
    closed_before: Option<i64>,
) -> DbResult<Vec<String>> {
    let rows = match closed_before {
        None => tx.query(
            "SELECT user_id FROM dm_members WHERE dm_channel_id = ? AND closed = 1",
            params![channel],
        )?,
        Some(before) => tx.query(
            "SELECT user_id FROM dm_members WHERE dm_channel_id = ? AND closed = 1 AND closed_changed_at < ?",
            params![channel, before],
        )?,
    };
    let at = now_ms();
    let mut reopened = Vec::new();
    for row in rows {
        let user = row.text("user_id")?;
        if tx.execute(
            "UPDATE dm_members SET closed = 0, closed_changed_at = ? \
             WHERE dm_channel_id = ? AND user_id = ? AND closed = 1",
            params![at, channel, &user],
        )? > 0
        {
            reopened.push(user);
        }
    }
    Ok(reopened)
}

fn member_ids(tx: &mut dyn Tx, channel: &str) -> DbResult<Vec<String>> {
    tx.query(
        "SELECT user_id FROM dm_members WHERE dm_channel_id = ?",
        params![channel],
    )?
    .iter()
    .map(|row| row.text("user_id"))
    .collect()
}

/// Новое сообщение (`processCreateEvent`).
#[allow(clippy::too_many_lines, reason = "порядок проверок оригинала")]
pub fn create(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let id = text(event, "messageId").unwrap_or_default().to_owned();
    let Some(message) = present(event, "message") else {
        return reject("missing_message_payload");
    };
    let participants = event
        .get("participants")
        .and_then(Value::as_array)
        .filter(|list| list.len() >= 2);
    let Some(participants) = participants else {
        return reject("missing_participants");
    };
    if let Some(refusal) = cx.refusal(Some(message))? {
        tracing::warn!(refusal, source = cx.source, "событие create отклонено");
        return reject(refusal);
    }
    if has_applied(cx.tx, cx.source, &format!("dm_delete:{id}"))? {
        return reject("duplicate");
    }
    if cx
        .tx
        .query_one(
            "SELECT id FROM dm_messages WHERE source_instance = ? AND source_message_id = ?",
            params![cx.source, &id],
        )?
        .is_some()
    {
        return reject("duplicate");
    }
    let system = text(message, "type") == Some("system");
    let mut system_content = None;
    if system {
        match canonical_space_invite(text(message, "content")) {
            Some(content) => system_content = Some(content),
            None => {
                tracing::warn!(event = id, "системное сообщение пира отклонено");
                return reject("invalid_system_message");
            }
        }
    }
    let mut resolved: Vec<Row> = Vec::new();
    for participant in participants {
        let Some(who) = actor(participant) else {
            continue;
        };
        let hints = hints_of(participant);
        let Some(replica) = cx.replica(&who, &hints)? else {
            continue;
        };
        let mut user = replica.user;
        if let Some(snapshot) = present(participant, "profile") {
            user = profile::hydrate(cx, user, snapshot)?;
        }
        resolved.push(user);
    }
    if resolved.len() < 2 {
        return reject("participant_not_found");
    }
    let author = match cx.resolve(message)? {
        Resolved::Found(user) => {
            let author_id = user.text("id")?;
            resolved
                .iter()
                .find(|row| row.text("id").is_ok_and(|id| id == author_id))
                .cloned()
        }
        _ => None,
    };
    let Some(author) = author else {
        return reject("author_not_found");
    };
    let author_id = author.text("id")?;
    let ours = our_origin(&cx.app.config);
    let mut created_copy = false;
    let channel_id = if let Some(federated) = non_empty(event, "federatedId") {
        let Some(channel) = live_channel(cx.tx, federated)? else {
            return reject("channel_not_found");
        };
        let channel_id = channel.text("id")?;
        let list = members(cx.tx, &channel_id)?;
        if !may_relay_into(&list, &ours, &author_id, cx.source) {
            let reason = non_member_refusal(&channel)?;
            tracing::warn!(
                channel = channel_id,
                reason,
                "сообщение пира в переписку отклонено"
            );
            return reject(reason);
        }
        channel_id
    } else {
        let pair = [Member::of(&resolved[0])?, Member::of(&resolved[1])?];
        if !may_relay_into(&pair, &ours, &author_id, cx.source) {
            tracing::warn!("сообщение один на один от пира отклонено");
            return reject("invalid_target");
        }
        let (channel, created) =
            crate::dm::find_or_create_pair(cx.tx, &cx.app.ids, &pair[0].id, &pair[1].id, true)?;
        created_copy = created;
        // Вызов, пришедший раньше этой копии, привязывается к ней.
        if let Some(key) = crate::federation::dm_federated_id(cx.tx, &channel)? {
            crate::voice::late_bind(&cx.app.hub, &key, &channel);
        }
        channel
    };
    let reply_to = reply_target(cx.tx, &ours, message.get("replyTo"), cx.source, &channel_id)?;
    let content = match system_content {
        Some(content) => Some(content),
        None => rewrite_mentions(cx, text(message, "content"), message.get("mentions"))?,
    };
    let created_at = millis(message.get("createdAt"));
    let local_id = cx.app.ids.next();
    cx.tx.execute(
        "INSERT INTO dm_messages (id, dm_channel_id, user_id, content, type, reply_to_id, created_at, \
         edited_at, source_instance, source_message_id, encryption_version) \
         VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, 0)",
        params![
            &local_id,
            &channel_id,
            &author_id,
            content,
            if system { "system" } else { "user" },
            reply_to,
            created_at,
            cx.source,
            &id
        ],
    )?;
    if let Some(attachments) = message.get("attachments").and_then(Value::as_array) {
        let now = now_ms();
        for attachment in attachments {
            let url = text(attachment, "sourceUrl").unwrap_or_default();
            if !is_url_from_peer(url, cx.peer) {
                tracing::warn!(url, peer = cx.peer, "вложение с чужого узла отклонено");
                continue;
            }
            let number = |key: &str| {
                attachment
                    .get(key)
                    .filter(|v| !v.is_null())
                    .and_then(Value::as_f64)
            };
            #[allow(clippy::cast_possible_truncation, reason = "размеры вложения")]
            let int = |key: &str| number(key).map(|n| n as i64);
            cx.tx.execute(
                "INSERT INTO attachments (id, dm_message_id, uploader_id, filename, original_name, mimetype, \
                 size, width, height, duration, playable, thumbnail_filename, source_url, created_at) \
                 VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?)",
                params![
                    cx.app.ids.next(),
                    &local_id,
                    url,
                    text(attachment, "originalName"),
                    text(attachment, "mimetype"),
                    int("size"),
                    int("width"),
                    int("height"),
                    number("duration"),
                    attachment.get("playable").and_then(Value::as_bool),
                    url,
                    now
                ],
            )?;
            cx.tx.execute(
                "INSERT INTO federation_file_queue (id, peer_origin, dm_message_id, source_url, original_name, \
                 mimetype, size, status, next_retry_at, expires_at, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?)",
                params![
                    cx.app.ids.next(),
                    cx.peer,
                    &local_id,
                    url,
                    text(attachment, "originalName"),
                    text(attachment, "mimetype"),
                    int("size"),
                    now,
                    now + FILE_QUEUE_TTL_MS,
                    now
                ],
            )?;
        }
    }
    let live = cx.delivery == Delivery::Live;
    let reopened = reopen_closed(cx.tx, &channel_id, (!live).then_some(created_at))?;
    let full = match crate::dm::message_row(cx.tx, &local_id)? {
        Some(row) => crate::dm::message_json(cx.tx, &row)?,
        None => None,
    };
    if created_copy && !live {
        if let Some(wire) = crate::dm::channel_wire(cx.tx, &channel_id)? {
            let event = json!({"type": "dm_channel_created", "dmChannel": wire});
            for member in wire["members"].as_array().into_iter().flatten() {
                if let Some(member) = member["id"].as_str() {
                    cx.to_user(member, &event)?;
                }
            }
        }
    } else if !reopened.is_empty()
        && let Some(mut wire) = crate::dm::channel_wire(cx.tx, &channel_id)?
    {
        if live && let Some(full) = &full {
            wire["lastMessage"] = full.clone();
        }
        let event = json!({"type": "dm_channel_created", "dmChannel": wire});
        for user in &reopened {
            cx.to_user(user, &event)?;
        }
    }
    if live && let Some(full) = &full {
        let event = json!({"type": "dm_message_created", "message": full});
        for member in member_ids(cx.tx, &channel_id)? {
            cx.to_user(&member, &event)?;
        }
    }
    for member in member_ids(cx.tx, &channel_id)? {
        if member != author_id {
            cx.to_user(
                &member,
                &json!({"type": "dm_typing_stop", "dmChannelId": channel_id, "userId": author_id}),
            )?;
        }
    }
    accept()
}

// ─── update / delete ────────────────────────────────────────────────────────

/// Цель правки или удаления в общих координатах (`FederationMessageTarget`).
fn message_target(value: &Value) -> Option<(&str, &str, &str, &Value)> {
    let federated = non_empty(value, "federatedId")?;
    let message = value.get("message")?;
    let id = non_empty(message, "messageId")?;
    let home = non_empty(message, "messageHomeInstance")?;
    let actor = value.get("actor")?;
    text(actor, "homeUserId")?;
    text(actor, "homeInstance")?;
    Some((federated, id, home, actor))
}

fn is_peer_of_message(cx: &mut Ctx<'_>, message: &Row) -> DbResult<bool> {
    let Some(source) = normalize_origin(Some(cx.source)) else {
        return Ok(false);
    };
    if normalize_origin(message.opt_text("source_instance")?.as_deref()) == Some(source) {
        return Ok(true);
    }
    let origins = dm_target_origins(cx.tx, &cx.app.config, &message.text("dm_channel_id")?)?;
    Ok(is_relay_target(&origins, cx.source))
}

enum Target {
    Found(Row),
    Refused(&'static str),
}

/// Сообщение, которое правят или удаляют (`resolveRelayedMutationTarget`).
fn mutation_target(cx: &mut Ctx<'_>, event: &Value, kind: &str) -> DbResult<Target> {
    let columns = crate::dm::DM_MESSAGE_COLUMNS;
    let Some(target) = event.get("target") else {
        let id = text(event, "messageId").unwrap_or_default();
        let legacy = cx.tx.query_one(
            &format!(
                "SELECT {columns} FROM dm_messages WHERE source_instance = ? AND source_message_id = ?"
            ),
            params![cx.source, id],
        )?;
        return Ok(legacy.map_or(Target::Refused("unknown_message"), Target::Found));
    };
    let Some((federated, message_id, home, who)) = message_target(target) else {
        return Ok(Target::Refused("invalid_target"));
    };
    if let Some(refusal) = cx.refusal(Some(who))? {
        tracing::warn!(
            refusal,
            kind,
            source = cx.source,
            "правка от пира отклонена"
        );
        return Ok(Target::Refused(refusal));
    }
    let Some(channel) = live_channel(cx.tx, federated)? else {
        return Ok(Target::Refused("unknown_message"));
    };
    let ours = our_origin(&cx.app.config);
    let Some(local) = resolve_local_message(cx.tx, &ours, message_id, Some(home), cx.source)?
    else {
        return Ok(Target::Refused("unknown_message"));
    };
    if local.text("dm_channel_id")? != channel.text("id")? {
        return Ok(Target::Refused("unknown_message"));
    }
    if !is_peer_of_message(cx, &local)? {
        tracing::warn!(kind, "правка от пира не из этой переписки отклонена");
        return Ok(Target::Refused("invalid_target"));
    }
    let author = user_row(cx.tx, &local.text("user_id")?)?;
    let author_identity = match &author {
        Some(row) => identity::actor_of_user(&cx.app.config, row)?,
        None => None,
    };
    let same = match (author_identity, actor(who)) {
        (Some(author), Some(who)) => identity::same_actor(&author, &who),
        _ => false,
    };
    if !same {
        tracing::warn!(kind, "правка от пира не автором отклонена");
        return Ok(Target::Refused("not_message_author"));
    }
    Ok(Target::Found(local))
}

fn attachments_payload(tx: &mut dyn Tx, message: &str) -> DbResult<Vec<Value>> {
    tx.query(
        "SELECT * FROM attachments WHERE dm_message_id = ?",
        params![message],
    )?
    .iter()
    .map(|row| {
        let owner = row
            .opt_text("dm_message_id")?
            .or(row.opt_text("message_id")?)
            .unwrap_or_default();
        Ok(json!({
            "id": row.text("id")?,
            "messageId": owner,
            "filename": row.text("filename")?,
            "originalName": row.opt_text("original_name")?,
            "mimetype": row.opt_text("mimetype")?,
            "size": row.opt_int("size")?,
            "thumbnailFilename": row.opt_text("thumbnail_filename")?,
            "width": row.opt_int("width")?,
            "height": row.opt_int("height")?,
            "duration": row.opt_number("duration")?,
            "playable": row.opt_flag("playable")?,
            "createdAt": row.opt_int("created_at")?,
        }))
    })
    .collect()
}

/// Правка сообщения (`processUpdateEvent`).
pub fn update(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let message = event.get("message").filter(|m| truthy(m));
    if let Some(message) = message
        && message.get("homeInstance").is_some_and(truthy)
        && let Some(refusal) = cx.refusal(Some(message))?
    {
        tracing::warn!(refusal, source = cx.source, "правка от пира отклонена");
        return reject(refusal);
    }
    let local = match mutation_target(cx, event, "update")? {
        Target::Found(row) => row,
        Target::Refused(reason) => return reject(reason),
    };
    if local.opt_text("type")?.as_deref() == Some("system") {
        return reject("system_message_immutable");
    }
    let content = rewrite_mentions(
        cx,
        message.and_then(|m| text(m, "content")),
        message.and_then(|m| m.get("mentions")),
    )?;
    let incoming = message
        .and_then(|m| m.get("editedAt"))
        .filter(|v| !v.is_null())
        .map(|v| millis(Some(v)));
    let held_edit = local.opt_int("edited_at")?;
    let already = match incoming {
        Some(incoming) => held_edit.is_some_and(|held| incoming <= held),
        None => content == local.opt_text("content")?,
    };
    if already {
        return accept();
    }
    let edited_at = incoming.unwrap_or_else(now_ms);
    let local_id = local.text("id")?;
    cx.tx.execute(
        "UPDATE dm_messages SET content = ?, edited_at = ? WHERE id = ?",
        params![&content, edited_at, &local_id],
    )?;
    let channel = local.text("dm_channel_id")?;
    if let Some(author) = user_row(cx.tx, &local.text("user_id")?)? {
        let reactions: Vec<Value> = cx
            .tx
            .query(
                "SELECT id, dm_message_id, user_id, emoji, created_at FROM dm_reactions WHERE dm_message_id = ? \
                 ORDER BY user_id, emoji",
                params![&local_id],
            )?
            .iter()
            .map(|row| {
                Ok(json!({
                    "id": row.text("id")?,
                    "messageId": row.text("dm_message_id")?,
                    "userId": row.text("user_id")?,
                    "emoji": row.text("emoji")?,
                    "createdAt": row.int("created_at")?,
                }))
            })
            .collect::<DbResult<_>>()?;
        let attachments = attachments_payload(cx.tx, &local_id)?;
        let payload = json!({
            "id": local_id,
            "dmChannelId": channel,
            "channelId": channel,
            "userId": local.text("user_id")?,
            "content": content,
            "replyToId": local.opt_text("reply_to_id")?,
            "editedAt": edited_at,
            "createdAt": local.int("created_at")?,
            "user": sanitize_user(&author, false)?,
            "attachments": attachments,
            "embeds": [],
            "reactions": reactions,
        });
        cx.to_dm_members(
            &channel,
            &json!({"type": "dm_message_updated", "message": payload}),
        )?;
    }
    accept()
}

fn delete_coordinates(event: &Value, source: &str) -> Option<(String, String)> {
    match event.get("target") {
        None => Some((
            text(event, "messageId").unwrap_or_default().to_owned(),
            source.to_owned(),
        )),
        Some(target) => {
            let (_, id, home, _) = message_target(target)?;
            Some((id.to_owned(), home.to_owned()))
        }
    }
}

fn record_tombstone(cx: &mut Ctx<'_>, id: &str, home: &str) -> DbResult<()> {
    if normalize_origin(Some(home)) != normalize_origin(Some(cx.source)) {
        return Ok(());
    }
    record_applied(cx.tx, cx.source, &format!("dm_delete:{id}"))
}

/// Удаление сообщения (`processDeleteEvent`).
pub fn delete(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let local = match mutation_target(cx, event, "delete")? {
        Target::Found(row) => row,
        Target::Refused(reason) => {
            if reason == "unknown_message"
                && let Some((id, home)) = delete_coordinates(event, cx.source)
            {
                let ours = our_origin(&cx.app.config);
                if resolve_local_message(cx.tx, &ours, &id, Some(&home), cx.source)?.is_none() {
                    record_tombstone(cx, &id, &home)?;
                    return accept();
                }
            }
            return reject(reason);
        }
    };
    if let (Some(instance), Some(source_id)) = (
        local.opt_text("source_instance")?.filter(|s| !s.is_empty()),
        local
            .opt_text("source_message_id")?
            .filter(|s| !s.is_empty()),
    ) {
        record_tombstone(cx, &source_id, &instance)?;
    }
    let local_id = local.text("id")?;
    let files: Vec<String> = cx
        .tx
        .query(
            "SELECT filename FROM attachments WHERE dm_message_id = ?",
            params![&local_id],
        )?
        .iter()
        .map(|row| row.text("filename"))
        .collect::<DbResult<_>>()?;
    cx.tx.execute(
        "DELETE FROM attachments WHERE dm_message_id = ?",
        params![&local_id],
    )?;
    cx.tx.execute(
        "DELETE FROM dm_reactions WHERE dm_message_id = ?",
        params![&local_id],
    )?;
    cx.tx
        .execute("DELETE FROM dm_messages WHERE id = ?", params![&local_id])?;
    for file in files {
        if !file.starts_with("http") {
            cx.discard(&file);
        }
    }
    let channel = local.text("dm_channel_id")?;
    cx.to_dm_members(
        &channel,
        &json!({"type": "dm_message_deleted", "messageId": local_id, "dmChannelId": channel}),
    )?;
    accept()
}

// ─── Реакции ────────────────────────────────────────────────────────────────

fn reaction_scope_refusal(
    cx: &mut Ctx<'_>,
    message: &Row,
    reactor: &Value,
) -> DbResult<Option<&'static str>> {
    if !is_peer_of_message(cx, message)? {
        return Ok(Some("invalid_target"));
    }
    let channel = message.text("dm_channel_id")?;
    let list = members(cx.tx, &channel)?;
    let ours = our_origin(&cx.app.config);
    if let Some(who) = actor(reactor)
        && member_with_identity(&list, &ours, &who).is_some()
    {
        return Ok(None);
    }
    let row = cx.tx.query_one(
        "SELECT owner_id FROM dm_channels WHERE id = ?",
        params![&channel],
    )?;
    Ok(Some(match row {
        Some(row) => non_member_refusal(&row)?,
        None => "invalid_target",
    }))
}

/// Реакция поставлена или снята (`processReactionAddEvent`,
/// `processReactionRemoveEvent`).
pub fn reaction(cx: &mut Ctx<'_>, event: &Value, add: bool) -> Step {
    let Some(reaction) = present(event, "reaction") else {
        return reject("missing_reaction_payload");
    };
    if let Some(refusal) = cx.refusal(Some(reaction))? {
        tracing::warn!(refusal, source = cx.source, "реакция от пира отклонена");
        return reject(refusal);
    }
    let canonical = text(reaction, "messageId")
        .or_else(|| text(event, "messageId"))
        .unwrap_or_default()
        .to_owned();
    let ours = our_origin(&cx.app.config);
    let local = resolve_local_message(
        cx.tx,
        &ours,
        &canonical,
        text(reaction, "messageHomeInstance"),
        cx.source,
    )?;
    let Some(local) = local else {
        return reject("unknown_message");
    };
    if let Some(refusal) = reaction_scope_refusal(cx, &local, reaction)? {
        tracing::warn!(refusal, "реакция от пира вне переписки отклонена");
        return reject(refusal);
    }
    let user = match cx.resolve(reaction)? {
        Resolved::Found(user) => user,
        Resolved::Mismatch => return reject("attribution_mismatch"),
        Resolved::Unknown => return reject("user_not_found"),
    };
    let user_id = user.text("id")?;
    let local_id = local.text("id")?;
    let channel = local.text("dm_channel_id")?;
    let emoji = reaction.get("emoji").cloned().unwrap_or(Value::Null);
    let emoji_text = emoji.as_str().map(str::to_owned);
    if add {
        let created = Some(millis(reaction.get("createdAt")))
            .filter(|at| *at != 0)
            .unwrap_or_else(now_ms);
        let reaction_id = cx.app.ids.next();
        let inserted = cx.tx.execute(
            "INSERT INTO dm_reactions (id, dm_message_id, user_id, emoji, created_at) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT DO NOTHING",
            params![&reaction_id, &local_id, &user_id, &emoji_text, created],
        )?;
        if inserted == 0 {
            return accept();
        }
        let mut wire = Map::new();
        wire.insert("id".into(), json!(reaction_id));
        wire.insert("messageId".into(), json!(local_id));
        wire.insert("userId".into(), json!(user_id));
        wire.insert("emoji".into(), emoji);
        wire.insert("createdAt".into(), json!(created));
        wire.insert("user".into(), sanitize_user(&user, false)?);
        cx.to_dm_members(
            &channel,
            &json!({"type": "reaction_added", "messageId": local_id, "reaction": wire}),
        )?;
    } else {
        let removed = cx.tx.execute(
            "DELETE FROM dm_reactions WHERE dm_message_id = ? AND user_id = ? AND emoji = ?",
            params![&local_id, &user_id, &emoji_text],
        )?;
        if removed > 0 {
            cx.to_dm_members(
                &channel,
                &json!({"type": "reaction_removed", "messageId": local_id, "userId": user_id, "emoji": emoji}),
            )?;
        }
    }
    accept()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_invites_are_canonical() {
        let raw = r#"{"snapshot":{"memberCount":3,"instanceName":"X","description":null,"icon":null,"spaceName":"S","extra":1},"inviteCode":"c","spaceInstanceOrigin":"https://x.test","spaceId":"1","event":"space_invite"}"#;
        assert_eq!(
            canonical_space_invite(Some(raw)).as_deref(),
            Some(
                r#"{"event":"space_invite","spaceId":"1","spaceInstanceOrigin":"https://x.test","inviteCode":"c","snapshot":{"spaceName":"S","icon":null,"avatarColor":null,"memberCount":3,"description":null,"instanceName":"X"}}"#
            )
        );
        assert_eq!(
            canonical_space_invite(Some(r#"{"event":"member_added"}"#)),
            None
        );
        assert!(!is_http_origin("https://x.test/"));
        assert!(!is_http_origin("https://u@x.test"));
        assert!(is_http_origin("https://x.test:8443"));
    }

    #[test]
    fn peer_urls() {
        assert!(is_url_from_peer(
            "https://y.test/api/uploads/a.png",
            "https://y.test"
        ));
        assert!(!is_url_from_peer("https://z.test/a.png", "https://y.test"));
    }
}
