//! События клиента по WebSocket (`ws/events.ts`).

use std::net::IpAddr;
use std::time::Duration;

use bs_permissions::Permissions;
use serde_json::{Map, Value, json};

use crate::db::{DbResult, Tx};
use crate::error::{ApiError, error_text};
use crate::hub::Outbox;
use crate::messages::{
    MAX_MESSAGE_LENGTH, Shape, channel_space, message_json, message_row, poke_content,
    reply_target_in_channel, system_name,
};
use crate::routes::users::{sanitize_user, user_row};
use crate::state::{AppState, now_ms};
use crate::util::{js_trim, utf16_len};
use crate::{broadcast, embeds, files, mentions, params, perms, ws};

/// Кто и откуда прислал событие.
#[derive(Debug, Clone)]
pub struct Context {
    /// Пользователь.
    pub user_id: String,
    /// Имя из токена.
    pub username: String,
    /// Реплицированный аккаунт другого инстанса.
    pub federated: bool,
    /// Номер соединения в концентраторе.
    pub connection: u64,
    /// Адрес клиента.
    pub ip: IpAddr,
}

/// Что надо сделать после транзакции события.
#[derive(Debug, Default)]
struct After {
    /// Файлы вложений, которые больше не нужны.
    stale_files: Vec<String>,
    /// Сообщение, для которого надо посчитать встраивания.
    embeds: Option<(String, Option<String>, embeds::Place)>,
}

/// Самая длинная строка реакции. Оригинал длину не проверяет и хранит любую
/// строку вплоть до мегабайта; эмодзи и имена своих значков много короче.
const MAX_EMOJI_LENGTH: usize = 256;
/// Идентификаторы сообщений — числа до 2^64: длиннее 20 цифр не бывают.
const MAX_ID_DIGITS: usize = 20;
/// Промежуток между обновлениями активности одного пользователя.
const ACTIVITY_INTERVAL: Duration = Duration::from_secs(3);
/// Тычки: не больше пяти в минуту.
const POKE_MAX: u32 = 5;
const POKE_WINDOW: Duration = Duration::from_mins(1);

fn text<'a>(event: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    event
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn error(out: &mut Outbox, user_id: &str, message: &str) {
    out.to_user(user_id, &json!({"type": "error", "message": message}));
}

fn coded_error(code: &'static str, details: Option<Value>) -> Value {
    let details_map = details.as_ref().and_then(Value::as_object);
    let mut event =
        json!({"type": "error", "message": error_text(code, details_map), "code": code});
    if let Some(details) = details {
        event["details"] = details;
    }
    event
}

fn has(
    tx: &mut dyn Tx,
    user: &str,
    space: &str,
    channel: &str,
    flag: Permissions,
) -> DbResult<bool> {
    perms::has_channel_permission(tx, user, space, channel, flag)
}

/// Обрабатывает одно событие. На неизвестный тип клиент получает ошибку.
///
/// # Errors
/// Внутренняя ошибка (клиент получит `Internal server error`).
pub async fn handle(
    state: &AppState,
    context: Context,
    event: Map<String, Value>,
) -> Result<(), ApiError> {
    let kind = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let app = state.clone();
    let after = state
        .run(move |tx, out| {
            let mut after = After::default();
            let cx = &context;
            match kind.as_str() {
                "message_create" => message_create(&app, tx, out, cx, &event, &mut after)?,
                "message_edit" => message_edit(&app, tx, out, cx, &event, &mut after)?,
                "message_delete" => message_delete(&app, tx, out, cx, &event, &mut after)?,
                "channel_poke" => channel_poke(&app, tx, out, cx, &event)?,
                "typing_start" => typing_start(&app, tx, out, cx, &event)?,
                "presence_update" => presence_update(&app, tx, out, cx, &event)?,
                "activity_update" => activity_update(&app, tx, out, cx, &event)?,
                "reaction_add" => reaction_add(&app, tx, out, cx, &event)?,
                "reaction_remove" => reaction_remove(&app, tx, out, cx, &event)?,
                "channel_ack" => channel_ack(tx, out, cx, &event, false)?,
                "mark_unread" => channel_ack(tx, out, cx, &event, true)?,
                other => {
                    if !crate::dm_events::handle(
                        &app,
                        tx,
                        out,
                        cx,
                        other,
                        &event,
                        &mut after.stale_files,
                    )? {
                        error(out, &cx.user_id, &format!("Unknown event type: {other}"));
                    }
                }
            }
            Ok(after)
        })
        .await?;
    for file in &after.stale_files {
        files::delete_upload_file(&state.config, file).await;
    }
    if let Some((message_id, content, place)) = after.embeds {
        embeds::schedule(state, message_id, content, place);
    }
    Ok(())
}

/// Соединение закрылось: снять с него то, что к нему привязано (голос).
pub fn connection_closed(state: &AppState, user_id: &str, connection: u64) {
    let _ = (state, user_id, connection);
}

/// Текст сообщения из события, проверенный так же, как в оригинале.
fn content_of<'a>(
    out: &mut Outbox,
    cx: &Context,
    event: &'a Map<String, Value>,
) -> Option<&'a str> {
    let Some(content) = text(event, "content").filter(|content| !js_trim(content).is_empty())
    else {
        error(out, &cx.user_id, "content is required");
        return None;
    };
    if utf16_len(content) > MAX_MESSAGE_LENGTH {
        error(
            out,
            &cx.user_id,
            &format!("Message content must be {MAX_MESSAGE_LENGTH} characters or less"),
        );
        return None;
    }
    Some(content)
}

fn missing_mention_everyone(out: &mut Outbox, user_id: &str) {
    out.to_user(
        user_id,
        &coded_error(
            "missing_permission",
            Some(json!({"permission": "MENTION_EVERYONE"})),
        ),
    );
}

fn message_create(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    cx: &Context,
    event: &Map<String, Value>,
    after: &mut After,
) -> DbResult<()> {
    let Some(channel) = text(event, "channelId") else {
        error(out, &cx.user_id, "channelId is required");
        return Ok(());
    };
    let Some(content) = content_of(out, cx, event) else {
        return Ok(());
    };
    let Some(space) = channel_space(tx, channel)? else {
        error(out, &cx.user_id, "Channel not found");
        return Ok(());
    };
    if !has(tx, &cx.user_id, &space, channel, Permissions::SEND_MESSAGES)? {
        error(out, &cx.user_id, "Missing SEND_MESSAGES permission");
        return Ok(());
    }
    if mentions::has_mass(Some(content))
        && !has(
            tx,
            &cx.user_id,
            &space,
            channel,
            Permissions::MENTION_EVERYONE,
        )?
    {
        missing_mention_everyone(out, &cx.user_id);
        return Ok(());
    }
    let reply_to = event
        .get("replyToId")
        .filter(|value| crate::util::truthy(Some(value)));
    let reply_to = match reply_to {
        Some(target) => {
            let target = target.as_str().unwrap_or_default();
            if !reply_target_in_channel(tx, channel, target)? {
                error(out, &cx.user_id, "Invalid reply target");
                return Ok(());
            }
            Some(target.to_owned())
        }
        None => None,
    };
    let trimmed = js_trim(content).to_owned();
    let message_id = app.ids.next();
    tx.execute(
        "INSERT INTO messages (id, channel_id, user_id, reply_to_id, content, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
        params![
            &message_id,
            channel,
            &cx.user_id,
            &reply_to,
            &trimmed,
            now_ms()
        ],
    )?;
    let Some(row) = message_row(tx, &message_id)? else {
        return Ok(());
    };
    if let Some(message) = message_json(tx, &row, Shape::Event, true, true)? {
        broadcast::to_channel(
            tx,
            &app.hub,
            out,
            &space,
            channel,
            &json!({"type": "message_created", "message": message}),
            None,
        )?;
        after.embeds = Some((
            message_id,
            Some(trimmed),
            embeds::Place::Channel {
                space_id: space,
                channel_id: channel.to_owned(),
            },
        ));
    }
    Ok(())
}

fn message_edit(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    cx: &Context,
    event: &Map<String, Value>,
    after: &mut After,
) -> DbResult<()> {
    let Some(message_id) = text(event, "messageId") else {
        error(out, &cx.user_id, "messageId is required");
        return Ok(());
    };
    let Some(content) = content_of(out, cx, event) else {
        return Ok(());
    };
    let Some(message) = message_row(tx, message_id)? else {
        error(out, &cx.user_id, "Message not found");
        return Ok(());
    };
    if message.text("type")? == "system" {
        out.to_user(&cx.user_id, &coded_error("system_message_immutable", None));
        return Ok(());
    }
    if message.text("user_id")? != cx.user_id {
        error(out, &cx.user_id, "You can only edit your own messages");
        return Ok(());
    }
    let channel = message.text("channel_id")?;
    let space = channel_space(tx, &channel)?;
    if !crate::routes::messages::author_still_sees(tx, &cx.user_id, &channel)? {
        out.to_user(
            &cx.user_id,
            &coded_error(
                "missing_permission",
                Some(json!({"permission": "VIEW_CHANNEL"})),
            ),
        );
        return Ok(());
    }
    if mentions::adds_mass(message.opt_text("content")?.as_deref(), Some(content)) {
        let allowed = match &space {
            Some(space) => has(
                tx,
                &cx.user_id,
                space,
                &channel,
                Permissions::MENTION_EVERYONE,
            )?,
            None => false,
        };
        if !allowed {
            missing_mention_everyone(out, &cx.user_id);
            return Ok(());
        }
    }
    let trimmed = js_trim(content).to_owned();
    tx.execute(
        "UPDATE messages SET content = ?, edited_at = ? WHERE id = ?",
        params![&trimmed, now_ms(), message_id],
    )?;
    let Some(space) = space else { return Ok(()) };
    tx.execute(
        "DELETE FROM embeds WHERE message_id = ?",
        params![message_id],
    )?;
    let Some(row) = message_row(tx, message_id)? else {
        return Ok(());
    };
    if let Some(updated) = message_json(tx, &row, Shape::Event, true, true)? {
        broadcast::to_channel(
            tx,
            &app.hub,
            out,
            &space,
            &channel,
            &json!({"type": "message_updated", "message": updated}),
            None,
        )?;
        after.embeds = Some((
            message_id.to_owned(),
            Some(trimmed),
            embeds::Place::Channel {
                space_id: space,
                channel_id: channel,
            },
        ));
    }
    Ok(())
}

fn message_delete(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    cx: &Context,
    event: &Map<String, Value>,
    after: &mut After,
) -> DbResult<()> {
    let Some(message_id) = text(event, "messageId") else {
        error(out, &cx.user_id, "messageId is required");
        return Ok(());
    };
    let Some(message) = message_row(tx, message_id)? else {
        error(out, &cx.user_id, "Message not found");
        return Ok(());
    };
    let channel = message.text("channel_id")?;
    let Some(space) = channel_space(tx, &channel)? else {
        return Ok(());
    };
    let author = message.text("user_id")? == cx.user_id;
    if !author
        && !has(
            tx,
            &cx.user_id,
            &space,
            &channel,
            Permissions::MANAGE_MESSAGES,
        )?
    {
        error(out, &cx.user_id, "You cannot delete this message");
        return Ok(());
    }
    after.stale_files = crate::routes::messages::delete_message(tx, message_id)?;
    broadcast::to_channel(
        tx,
        &app.hub,
        out,
        &space,
        &channel,
        &json!({"type": "message_deleted", "messageId": message_id, "channelId": channel}),
        None,
    )
}

fn channel_poke(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    cx: &Context,
    event: &Map<String, Value>,
) -> DbResult<()> {
    let refuse = |out: &mut Outbox, code: &'static str, details: Option<Value>| {
        out.to_connection(cx.connection, &coded_error(code, details));
    };
    let (Some(channel), Some(target)) = (
        event.get("channelId").and_then(Value::as_str),
        event.get("targetUserId").and_then(Value::as_str),
    ) else {
        refuse(out, "validation_failed", None);
        return Ok(());
    };
    let space = channel_space(tx, channel)?;
    let Some(space) = space else {
        refuse(out, "channel_not_found", None);
        return Ok(());
    };
    if !has(tx, &cx.user_id, &space, channel, Permissions::VIEW_CHANNEL)? {
        refuse(out, "channel_not_found", None);
        return Ok(());
    }
    if !has(tx, &cx.user_id, &space, channel, Permissions::SEND_MESSAGES)? {
        refuse(
            out,
            "missing_permission",
            Some(json!({"permission": "SEND_MESSAGES"})),
        );
        return Ok(());
    }
    let member = tx.query_one(
        "SELECT m.nickname, u.display_name, u.username FROM space_members m \
         JOIN users u ON u.id = m.user_id WHERE m.space_id = ? AND m.user_id = ?",
        params![&space, target],
    )?;
    let Some(member) = member else {
        refuse(out, "member_not_found", None);
        return Ok(());
    };
    if !has(tx, target, &space, channel, Permissions::VIEW_CHANNEL)? {
        refuse(out, "poke_target_no_access", None);
        return Ok(());
    }
    if !app
        .hub
        .allow(&format!("poke:{}", cx.user_id), POKE_MAX, POKE_WINDOW)
    {
        refuse(out, "rate_limited", None);
        return Ok(());
    }
    let name = match member.opt_text("nickname")?.filter(|nick| !nick.is_empty()) {
        Some(nick) => nick,
        None => system_name(
            member.opt_text("display_name")?.as_deref(),
            &member.text("username")?,
        ),
    };
    let message_id = app.ids.next();
    tx.execute(
        "INSERT INTO messages (id, channel_id, user_id, type, content, created_at) \
         VALUES (?, ?, ?, 'system', ?, ?)",
        params![
            &message_id,
            channel,
            &cx.user_id,
            poke_content(target, &name),
            now_ms()
        ],
    )?;
    let Some(row) = message_row(tx, &message_id)? else {
        return Ok(());
    };
    if let Some(message) = message_json(tx, &row, Shape::Rest, false, false)? {
        broadcast::to_channel(
            tx,
            &app.hub,
            out,
            &space,
            channel,
            &json!({"type": "channel_poke", "message": message}),
            None,
        )?;
    }
    Ok(())
}

fn typing_start(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    cx: &Context,
    event: &Map<String, Value>,
) -> DbResult<()> {
    let Some(channel) = text(event, "channelId") else {
        return Ok(());
    };
    let Some(space) = channel_space(tx, channel)? else {
        return Ok(());
    };
    if !has(tx, &cx.user_id, &space, channel, Permissions::SEND_MESSAGES)? {
        return Ok(());
    }
    broadcast::to_channel(
        tx,
        &app.hub,
        out,
        &space,
        channel,
        &json!({"type": "typing", "channelId": channel, "userId": cx.user_id, "username": cx.username}),
        Some(&cx.user_id),
    )
}

/// Применяет выбранный пользователем статус (`applyChosenStatus`).
///
/// # Errors
/// Ошибка базы.
pub fn apply_chosen_status(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    user_id: &str,
    status: &str,
) -> DbResult<()> {
    let Some(row) = tx.query_one(
        "SELECT home_instance, federation_home_orphaned FROM users WHERE id = ?",
        params![user_id],
    )?
    else {
        return Ok(());
    };
    let owns = row
        .opt_text("home_instance")?
        .is_none_or(|home| home.is_empty())
        || row.opt_flag("federation_home_orphaned")? == Some(true);
    let connected = app.hub.is_online(user_id);
    if owns && connected {
        tx.execute(
            "UPDATE users SET chosen_status = ?, status = ? WHERE id = ?",
            params![status, status, user_id],
        )?;
    } else if owns {
        tx.execute(
            "UPDATE users SET chosen_status = ? WHERE id = ?",
            params![status, user_id],
        )?;
    } else if connected {
        tx.execute(
            "UPDATE users SET status = ? WHERE id = ?",
            params![status, user_id],
        )?;
    }
    if !connected {
        return Ok(());
    }
    app.hub.set_status(user_id, status);
    let activities = app.hub.activities(user_id);
    let payload = ws::presence_update(
        tx,
        user_id,
        status,
        (!activities.is_empty()).then_some(activities.as_slice()),
    )?;
    ws::to_profile_audience(tx, out, user_id, &payload)?;
    out.to_user(user_id, &payload);
    Ok(())
}

fn presence_update(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    cx: &Context,
    event: &Map<String, Value>,
) -> DbResult<()> {
    let status = event.get("status").and_then(Value::as_str);
    let Some(status) = status.filter(|status| matches!(*status, "online" | "idle" | "dnd")) else {
        error(
            out,
            &cx.user_id,
            "Status must be \"online\", \"idle\", or \"dnd\"",
        );
        return Ok(());
    };
    apply_chosen_status(app, tx, out, &cx.user_id, status)
}

/// Проверяет и приводит список активностей (`validateActivities`).
#[must_use]
pub fn validate_activities(raw: Option<&Value>) -> Option<Vec<Value>> {
    const MAX_TEXT: usize = 128;
    const MAX_URL: usize = 512;
    const MAX_TIMESTAMP: f64 = 4_102_444_800_000.0;
    let list = raw?.as_array()?;
    if list.len() > 5 {
        return None;
    }
    let mut validated = Vec::with_capacity(list.len());
    for item in list {
        let object = item.as_object()?;
        let kind = object.get("type").and_then(Value::as_str)?;
        if !matches!(
            kind,
            "custom" | "playing" | "listening" | "watching" | "streaming"
        ) {
            return None;
        }
        let name = object.get("name").and_then(Value::as_str)?;
        if js_trim(name).is_empty() || utf16_len(name) > MAX_TEXT {
            return None;
        }
        let mut activity = json!({"type": kind, "name": js_trim(name)});
        let short = |key: &str, max: usize| {
            object
                .get(key)
                .and_then(Value::as_str)
                .filter(|text| utf16_len(text) <= max)
        };
        if let Some(details) = short("details", MAX_TEXT) {
            activity["details"] = json!(js_trim(details));
        }
        if let Some(state) = short("state", MAX_TEXT) {
            activity["state"] = json!(js_trim(state));
        }
        if let Some(url) = short("url", MAX_URL)
            && (url.starts_with("https://") || url.starts_with("http://"))
        {
            activity["url"] = json!(url);
        }
        if let Some(stamps) = object.get("timestamps").and_then(Value::as_object) {
            let mut kept = Map::new();
            for key in ["start", "end"] {
                if let Some(value) = stamps.get(key).filter(|value| {
                    value
                        .as_f64()
                        .is_some_and(|n| (0.0..=MAX_TIMESTAMP).contains(&n))
                }) {
                    kept.insert(key.to_owned(), value.clone());
                }
            }
            if !kept.is_empty() {
                activity["timestamps"] = Value::Object(kept);
            }
        }
        if let Some(assets) = object.get("assets").and_then(Value::as_object) {
            let mut kept = Map::new();
            for (key, max) in [
                ("largeImage", MAX_URL),
                ("largeText", MAX_TEXT),
                ("smallImage", MAX_URL),
                ("smallText", MAX_TEXT),
            ] {
                if let Some(value) = assets
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|text| utf16_len(text) <= max)
                {
                    kept.insert(key.to_owned(), json!(value));
                }
            }
            if !kept.is_empty() {
                activity["assets"] = Value::Object(kept);
            }
        }
        validated.push(activity);
    }
    Some(validated)
}

fn activity_update(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    cx: &Context,
    event: &Map<String, Value>,
) -> DbResult<()> {
    if !app.hub.show_activity(&cx.user_id) {
        return Ok(());
    }
    if !app
        .hub
        .allow(&format!("activity:{}", cx.user_id), 1, ACTIVITY_INTERVAL)
    {
        error(out, &cx.user_id, "Activity update rate limited");
        return Ok(());
    }
    let Some(activities) = validate_activities(event.get("activities")) else {
        error(out, &cx.user_id, "Invalid activity payload");
        return Ok(());
    };
    app.hub.set_activities(&cx.user_id, activities.clone());
    let status = app
        .hub
        .status(&cx.user_id)
        .unwrap_or_else(|| "online".to_owned());
    let payload = ws::presence_update(tx, &cx.user_id, &status, Some(&activities))?;
    ws::to_profile_audience(tx, out, &cx.user_id, &payload)?;
    out.to_user(&cx.user_id, &payload);
    Ok(())
}

fn reaction_add(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    cx: &Context,
    event: &Map<String, Value>,
) -> DbResult<()> {
    let (Some(message_id), Some(emoji)) = (text(event, "messageId"), text(event, "emoji")) else {
        return Ok(());
    };
    if utf16_len(emoji) > MAX_EMOJI_LENGTH {
        return Ok(());
    }
    let Some(message) = message_row(tx, message_id)? else {
        return crate::dm_events::reaction(app, tx, out, cx, message_id, emoji, true);
    };
    let channel = message.text("channel_id")?;
    let Some(space) = channel_space(tx, &channel)? else {
        return Ok(());
    };
    if !perms::is_member(tx, &space, &cx.user_id)? {
        return Ok(());
    }
    // Оригинал смотрит только на `ADD_REACTIONS`, так что участник ставит
    // реакции и в канале, которого не видит. Здесь нужен и просмотр.
    let channel_perms = perms::compute(tx, &cx.user_id, &space, Some(&channel))?;
    if !perms::has(channel_perms, Permissions::ADD_REACTIONS) {
        error(out, &cx.user_id, "Missing ADD_REACTIONS permission");
        return Ok(());
    }
    if !perms::can_view(channel_perms) {
        return Ok(());
    }
    let reaction_id = app.ids.next();
    let now = now_ms();
    let inserted = tx.execute(
        "INSERT INTO reactions (id, message_id, user_id, emoji, created_at) VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT DO NOTHING",
        params![&reaction_id, message_id, &cx.user_id, emoji, now],
    )?;
    if inserted == 0 {
        return Ok(());
    }
    let mut reaction = json!({
        "id": reaction_id,
        "messageId": message_id,
        "userId": cx.user_id,
        "emoji": emoji,
        "createdAt": now,
    });
    if let Some(user) = user_row(tx, &cx.user_id)? {
        reaction["user"] = sanitize_user(&user, false)?;
    }
    broadcast::to_channel(
        tx,
        &app.hub,
        out,
        &space,
        &channel,
        &json!({"type": "reaction_added", "messageId": message_id, "reaction": reaction}),
        None,
    )
}

fn reaction_remove(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    cx: &Context,
    event: &Map<String, Value>,
) -> DbResult<()> {
    let (Some(message_id), Some(emoji)) = (text(event, "messageId"), text(event, "emoji")) else {
        return Ok(());
    };
    let Some(message) = message_row(tx, message_id)? else {
        return crate::dm_events::reaction(app, tx, out, cx, message_id, emoji, false);
    };
    let channel = message.text("channel_id")?;
    let Some(space) = channel_space(tx, &channel)? else {
        return Ok(());
    };
    if !perms::is_member(tx, &space, &cx.user_id)? {
        return Ok(());
    }
    let removed = tx.execute(
        "DELETE FROM reactions WHERE message_id = ? AND user_id = ? AND emoji = ?",
        params![message_id, &cx.user_id, emoji],
    )?;
    if removed > 0 {
        broadcast::to_channel(
            tx,
            &app.hub,
            out,
            &space,
            &channel,
            &json!({
                "type": "reaction_removed",
                "messageId": message_id,
                "userId": cx.user_id,
                "emoji": emoji,
            }),
            None,
        )?;
    }
    Ok(())
}

/// Сравнение идентификаторов как чисел: `a > b`.
fn id_greater(a: &str, b: &str) -> bool {
    let strip = |text: &str| text.trim_start_matches('0').to_owned();
    crate::messages::compare_snowflakes(&strip(a), &strip(b)) == std::cmp::Ordering::Greater
}

/// `channel_ack` и `mark_unread`: позиция чтения пользователя в канале или
/// переписке.
fn channel_ack(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    cx: &Context,
    event: &Map<String, Value>,
    mark_unread: bool,
) -> DbResult<()> {
    let (Some(channel), Some(message_id)) = (text(event, "channelId"), text(event, "messageId"))
    else {
        return Ok(());
    };
    // Оригинал разбирает любое число цифр как BigInt и на мегабайтной строке
    // надолго занимает единственный поток. Идентификаторы короче.
    if message_id.len() > MAX_ID_DIGITS || !message_id.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(());
    }
    let allowed = match channel_space(tx, channel)? {
        Some(space) => perms::is_member(tx, &space, &cx.user_id)?,
        None => crate::dm::is_member(tx, channel, &cx.user_id)?,
    };
    if !allowed {
        return Ok(());
    }
    let existing = tx
        .query_one(
            "SELECT last_read_message_id FROM read_states WHERE user_id = ? AND channel_id = ?",
            params![&cx.user_id, channel],
        )?
        .map(|row| row.text("last_read_message_id"))
        .transpose()?;
    let now = now_ms();
    if mark_unread && message_id == "0" {
        tx.execute(
            "DELETE FROM read_states WHERE user_id = ? AND channel_id = ?",
            params![&cx.user_id, channel],
        )?;
    } else if let Some(existing) = existing {
        if mark_unread || id_greater(message_id, &existing) {
            tx.execute(
                "UPDATE read_states SET last_read_message_id = ?, updated_at = ? \
                 WHERE user_id = ? AND channel_id = ?",
                params![message_id, now, &cx.user_id, channel],
            )?;
        }
    } else {
        tx.execute(
            "INSERT INTO read_states (user_id, channel_id, last_read_message_id, updated_at) \
             VALUES (?, ?, ?, ?)",
            params![&cx.user_id, channel, message_id, now],
        )?;
    }
    let kind = if mark_unread {
        "mark_unread"
    } else {
        "channel_ack"
    };
    broadcast::to_user(
        tx,
        out,
        &cx.user_id,
        &json!({"type": kind, "channelId": channel, "messageId": message_id}),
    )?;
    if !(mark_unread && message_id == "0") {
        crate::federation::queue_read_state(tx, channel, message_id, &cx.user_id)?;
    }
    Ok(())
}
