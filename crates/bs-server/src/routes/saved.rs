//! «Избранное», закреплённые сообщения и закладки (Rustspace).
//!
//! **Избранное** — переписка с самим собой, как в Telegram: личная группа из
//! одного участника. Оригинал запрещает ЛС с собой (`cannot_dm_self`), а
//! группа из одного человека ему понятна, поэтому веб-клиент оригинала
//! покажет «Избранное» как обычную группу. Добавить в неё никого нельзя.
//!
//! - `GET /api/rustspace/saved` — переписка «Избранное» (создаётся при
//!   первом обращении).
//!
//! **Закреплённые** — сообщения, закреплённые в чате. Общие видят все
//! участники; личные — только закрепивший.
//!
//! - `GET /api/rustspace/pins/:kind/:channelId` — закреплённые, новые сверху,
//!   с самими сообщениями;
//! - `PUT /api/rustspace/pins/:kind/:channelId/:messageId` `{shared}`;
//! - `DELETE /api/rustspace/pins/:kind/:channelId/:messageId?shared=`.
//!
//! Общие закрепы в канале требуют права «Управлять сообщениями», в ЛС —
//! участия. Об изменении узнают участники (`rs_pins_updated`).
//!
//! **Закладки** — личные копии сообщений из всех чатов. Маршруты и форма —
//! как в PR оригинала #474 (`/api/bookmarks`), чтобы веб-клиент с этой
//! функцией работал и с нашим сервером.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use bs_permissions::Permissions;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Row, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::JsonBody;
use crate::messages::{Shape, channel_space};
use crate::state::{AppState, now_ms};
use crate::{broadcast, dm, params, perms};

/// Название переписки «Избранное» для клиентов, которые о ней не знают.
const SAVED_NAME: &str = "Избранное";
/// Общих закрепов в одном чате.
const MAX_SHARED_PINS: i64 = 50;
/// Страница закладок по умолчанию и наибольшая (как в #474).
const BOOKMARKS_PAGE_DEFAULT: i64 = 30;
const BOOKMARKS_PAGE_MAX: i64 = 100;

/// Является ли переписка чьим-то «Избранным».
///
/// # Errors
/// Ошибка базы.
pub fn is_saved(tx: &mut dyn Tx, dm_channel: &str) -> DbResult<bool> {
    Ok(tx
        .query_one(
            "SELECT 1 AS one FROM bs_saved_messages WHERE dm_channel_id = ?",
            params![dm_channel],
        )?
        .is_some())
}

/// `GET /api/rustspace/saved`.
pub async fn saved(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    let app = state.clone();
    state
        .run(move |tx, out| {
            let existing = tx
                .query_one(
                    "SELECT s.dm_channel_id FROM bs_saved_messages s \
                     JOIN dm_channels c ON c.id = s.dm_channel_id \
                     WHERE s.user_id = ? AND c.deleted_at IS NULL",
                    params![&user.id],
                )?
                .map(|row| row.text("dm_channel_id"))
                .transpose()?;
            let channel = if let Some(channel) = existing {
                dm::set_closed(tx, &channel, &user.id, false)?;
                channel
            } else {
                tx.execute(
                    "DELETE FROM bs_saved_messages WHERE user_id = ?",
                    params![&user.id],
                )?;
                let channel = app.ids.next();
                let now = now_ms();
                tx.execute(
                    "INSERT INTO dm_channels (id, owner_id, name, created_at) VALUES (?, ?, ?, ?)",
                    params![&channel, &user.id, SAVED_NAME, now],
                )?;
                dm::insert_member(tx, &channel, &user.id, false, now)?;
                tx.execute(
                    "INSERT INTO bs_saved_messages (user_id, dm_channel_id) VALUES (?, ?)",
                    params![&user.id, &channel],
                )?;
                let wire = dm::channel_wire(tx, &channel)?.ok_or_else(ApiError::internal)?;
                broadcast::to_user(
                    tx,
                    out,
                    &user.id,
                    &json!({"type": "dm_channel_created", "dmChannel": wire}),
                )?;
                channel
            };
            let mut wire = dm::channel_wire(tx, &channel)?.ok_or_else(ApiError::internal)?;
            wire["savedMessages"] = json!(true);
            Ok(Json(wire))
        })
        .await
}

/// Чат: канал пространства или переписка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Channel,
    Dm,
}

impl Kind {
    fn parse(value: &str) -> ApiResult<Self> {
        match value {
            "channel" => Ok(Self::Channel),
            "dm" => Ok(Self::Dm),
            _ => Err(ApiError::bad_request("validation_failed")),
        }
    }

    const fn code(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Dm => "dm",
        }
    }
}

/// Может ли пользователь читать чат; для канала — его пространство.
fn readable(tx: &mut dyn Tx, user: &str, kind: Kind, channel: &str) -> ApiResult<Option<String>> {
    match kind {
        Kind::Channel => {
            let space = channel_space(tx, channel)?
                .ok_or_else(|| ApiError::not_found("channel_not_found"))?;
            if !perms::has_channel_permission(
                tx,
                user,
                &space,
                channel,
                Permissions::VIEW_CHANNEL | Permissions::READ_MESSAGE_HISTORY,
            )? {
                return Err(ApiError::not_found("channel_not_found"));
            }
            Ok(Some(space))
        }
        Kind::Dm => {
            if !dm::is_member(tx, channel, user)? {
                return Err(ApiError::not_found("channel_not_found"));
            }
            Ok(None)
        }
    }
}

/// Сообщение, как его отдаёт лента (`None` — его больше нет).
fn message_value(tx: &mut dyn Tx, kind: Kind, id: &str) -> DbResult<Option<Value>> {
    match kind {
        Kind::Channel => match crate::messages::message_row(tx, id)? {
            Some(row) => crate::messages::message_json(tx, &row, Shape::Event, true, true),
            None => Ok(None),
        },
        Kind::Dm => match dm::message_row(tx, id)? {
            Some(row) => dm::message_json(tx, &row),
            None => Ok(None),
        },
    }
}

/// Канал сообщения (`None` — сообщения нет).
fn message_channel(tx: &mut dyn Tx, kind: Kind, id: &str) -> DbResult<Option<String>> {
    let sql = match kind {
        Kind::Channel => "SELECT channel_id AS c FROM messages WHERE id = ?",
        Kind::Dm => "SELECT dm_channel_id AS c FROM dm_messages WHERE id = ?",
    };
    tx.query_one(sql, params![id])?
        .map(|row| row.text("c"))
        .transpose()
}

/// Сообщает об изменении закрепов: общих — всем участникам, личных — себе.
fn announce_pins(
    tx: &mut dyn Tx,
    out: &mut crate::hub::Outbox,
    state: &AppState,
    user: &str,
    (kind, channel, space): (Kind, &str, Option<&str>),
    shared: bool,
) -> DbResult<()> {
    let event = json!({"type": "rs_pins_updated", "kind": kind.code(), "channelId": channel});
    if !shared {
        return broadcast::to_user(tx, out, user, &event);
    }
    match (kind, space) {
        (Kind::Channel, Some(space)) => {
            broadcast::to_channel(tx, &state.hub, out, space, channel, &event, None)
        }
        _ => {
            for row in tx.query(
                "SELECT user_id FROM dm_members WHERE dm_channel_id = ?",
                params![channel],
            )? {
                broadcast::to_user(tx, out, &row.text("user_id")?, &event)?;
            }
            Ok(())
        }
    }
}

/// `GET /api/rustspace/pins/:kind/:channelId`.
pub async fn pins(
    State(state): State<AppState>,
    user: AuthUser,
    Path((kind, channel)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let kind = Kind::parse(&kind)?;
    state
        .run(move |tx, _| {
            readable(tx, &user.id, kind, &channel)?;
            let rows = tx.query(
                "SELECT id, message_id, user_id, pinned_by, created_at FROM bs_pins \
                 WHERE kind = ? AND channel_id = ? AND (user_id = '' OR user_id = ?) \
                 ORDER BY created_at DESC, id DESC",
                params![kind.code(), &channel, &user.id],
            )?;
            let mut list = Vec::with_capacity(rows.len());
            for row in &rows {
                let message_id = row.text("message_id")?;
                let Some(message) = message_value(tx, kind, &message_id)? else {
                    continue;
                };
                list.push(json!({
                    "id": row.text("id")?,
                    "kind": kind.code(),
                    "channelId": &channel,
                    "messageId": message_id,
                    "shared": row.text("user_id")?.is_empty(),
                    "pinnedBy": row.text("pinned_by")?,
                    "createdAt": row.int("created_at")?,
                    "message": message,
                }));
            }
            Ok(Json(json!(list)))
        })
        .await
}

/// `?shared=` у снятия закрепа.
#[derive(Debug, Default, serde::Deserialize)]
pub struct SharedQuery {
    shared: Option<String>,
}

/// Проверки закрепа: чат читается, сообщение в нём, право на общий закреп.
fn pin_access(
    tx: &mut dyn Tx,
    user: &str,
    kind: Kind,
    channel: &str,
    message: &str,
    shared: bool,
) -> ApiResult<Option<String>> {
    let space = readable(tx, user, kind, channel)?;
    if message_channel(tx, kind, message)?.as_deref() != Some(channel) {
        return Err(ApiError::not_found("message_not_found"));
    }
    if shared
        && let Some(space) = &space
        && !perms::has_channel_permission(tx, user, space, channel, Permissions::MANAGE_MESSAGES)?
    {
        return Err(perms::missing("MANAGE_MESSAGES"));
    }
    Ok(space)
}

/// `PUT /api/rustspace/pins/:kind/:channelId/:messageId` `{shared}`.
pub async fn pin(
    State(state): State<AppState>,
    user: AuthUser,
    Path((kind, channel, message)): Path<(String, String, String)>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let kind = Kind::parse(&kind)?;
    let shared = body.get("shared").and_then(Value::as_bool).unwrap_or(true);
    let app = state.clone();
    state
        .run(move |tx, out| {
            let space = pin_access(tx, &user.id, kind, &channel, &message, shared)?;
            let owner = if shared { String::new() } else { user.id.clone() };
            if tx
                .query_one(
                    "SELECT id FROM bs_pins WHERE kind = ? AND channel_id = ? AND message_id = ? AND user_id = ?",
                    params![kind.code(), &channel, &message, &owner],
                )?
                .is_none()
            {
                if shared {
                    let count = tx
                        .query_one(
                            "SELECT COUNT(*) AS n FROM bs_pins WHERE kind = ? AND channel_id = ? AND user_id = ''",
                            params![kind.code(), &channel],
                        )?
                        .map_or(Ok(0), |row| row.int("n"))?;
                    if count >= MAX_SHARED_PINS {
                        return Err(ApiError::bad_request("pin_limit").detail("max", MAX_SHARED_PINS));
                    }
                }
                tx.execute(
                    "INSERT INTO bs_pins (id, kind, channel_id, message_id, user_id, pinned_by, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    params![
                        app.ids.next(),
                        kind.code(),
                        &channel,
                        &message,
                        &owner,
                        &user.id,
                        now_ms()
                    ],
                )?;
                announce_pins(tx, out, &app, &user.id, (kind, &channel, space.as_deref()), shared)?;
            }
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `DELETE /api/rustspace/pins/:kind/:channelId/:messageId?shared=`.
pub async fn unpin(
    State(state): State<AppState>,
    user: AuthUser,
    Path((kind, channel, message)): Path<(String, String, String)>,
    Query(query): Query<SharedQuery>,
) -> ApiResult<Json<Value>> {
    let kind = Kind::parse(&kind)?;
    let shared = query.shared.as_deref() != Some("false");
    let app = state.clone();
    state
        .run(move |tx, out| {
            let space = readable(tx, &user.id, kind, &channel)?;
            if shared
                && let Some(space) = &space
                && !perms::has_channel_permission(
                    tx,
                    &user.id,
                    space,
                    &channel,
                    Permissions::MANAGE_MESSAGES,
                )?
            {
                return Err(perms::missing("MANAGE_MESSAGES"));
            }
            let owner = if shared { String::new() } else { user.id.clone() };
            let removed = tx.execute(
                "DELETE FROM bs_pins WHERE kind = ? AND channel_id = ? AND message_id = ? AND user_id = ?",
                params![kind.code(), &channel, &message, &owner],
            )?;
            if removed > 0 {
                announce_pins(tx, out, &app, &user.id, (kind, &channel, space.as_deref()), shared)?;
            }
            Ok(Json(json!({"success": true})))
        })
        .await
}

// ─── Закладки (#474) ─────────────────────────────────────────────────────────

/// Исходное сообщение закладки.
struct Source {
    channel: String,
    space: Option<String>,
    content: Option<String>,
    author: String,
    created_at: i64,
}

fn source(tx: &mut dyn Tx, kind: Kind, id: &str) -> ApiResult<Source> {
    let sql = match kind {
        Kind::Channel => {
            "SELECT m.channel_id AS channel, c.space_id AS space, m.content, m.user_id, \
             m.created_at, m.type FROM messages m JOIN channels c ON c.id = m.channel_id \
             WHERE m.id = ?"
        }
        Kind::Dm => {
            "SELECT dm_channel_id AS channel, NULL AS space, content, user_id, created_at, type \
             FROM dm_messages WHERE id = ?"
        }
    };
    let row = tx
        .query_one(sql, params![id])?
        .ok_or_else(|| ApiError::not_found("message_not_found"))?;
    if row.opt_text("type")?.as_deref() == Some("system") {
        return Err(ApiError::bad_request("validation_failed"));
    }
    Ok(Source {
        channel: row.text("channel")?,
        space: row.opt_text("space")?,
        content: row.opt_text("content")?,
        author: row.text("user_id")?,
        created_at: row.int("created_at")?,
    })
}

/// Имя пользователя для копии: отображаемое, иначе логин.
fn name_of(tx: &mut dyn Tx, user: &str) -> DbResult<String> {
    let row = tx.query_one(
        "SELECT display_name, username, is_deleted FROM users WHERE id = ?",
        params![user],
    )?;
    Ok(row.map_or_else(
        || "Deleted User".to_owned(),
        |row| {
            if row.opt_flag("is_deleted").ok().flatten() == Some(true) {
                return "Deleted User".to_owned();
            }
            row.opt_text("display_name")
                .ok()
                .flatten()
                .filter(|name| !name.is_empty())
                .or_else(|| row.text("username").ok())
                .unwrap_or_default()
        },
    ))
}

/// Имена пространства и чата для копии.
fn conversation_names(
    tx: &mut dyn Tx,
    user: &str,
    kind: Kind,
    src: &Source,
) -> DbResult<(Option<String>, Option<String>)> {
    if kind == Kind::Channel {
        let row = tx.query_one(
            "SELECT s.name AS space_name, c.name AS channel_name FROM channels c \
             JOIN spaces s ON s.id = c.space_id WHERE c.id = ?",
            params![&src.channel],
        )?;
        return Ok(row.map_or((None, None), |row| {
            (
                row.opt_text("space_name").ok().flatten(),
                row.opt_text("channel_name").ok().flatten(),
            )
        }));
    }
    let group = tx
        .query_one(
            "SELECT name FROM dm_channels WHERE id = ?",
            params![&src.channel],
        )?
        .and_then(|row| row.opt_text("name").ok().flatten())
        .filter(|name| !name.is_empty());
    if group.is_some() {
        return Ok((None, group));
    }
    let mut names = Vec::new();
    for row in tx.query(
        "SELECT user_id FROM dm_members WHERE dm_channel_id = ? AND user_id <> ?",
        params![&src.channel, user],
    )? {
        names.push(name_of(tx, &row.text("user_id")?)?);
    }
    Ok((None, (!names.is_empty()).then(|| names.join(", "))))
}

const BOOKMARK_COLUMNS: &str = "id, kind, message_id, channel_id, space_id, content, author_id, \
     author_name, space_name, channel_name, instance_origin, message_created_at, created_at";

/// Закладка в форме #474: состояние оригинала считается при чтении.
fn bookmark_json(tx: &mut dyn Tx, user: &str, row: &Row) -> ApiResult<Value> {
    let id = row.text("id")?;
    let kind = Kind::parse(&row.text("kind")?)?;
    let message = row.text("message_id")?;
    let channel = row.text("channel_id")?;
    let saved = row.opt_text("content")?;
    let current = match kind {
        Kind::Channel => tx.query_one(
            "SELECT content FROM messages WHERE id = ?",
            params![&message],
        )?,
        Kind::Dm => tx.query_one(
            "SELECT content FROM dm_messages WHERE id = ?",
            params![&message],
        )?,
    };
    let status = match current {
        None => "deleted",
        Some(_) if readable(tx, user, kind, &channel).is_err() => "unavailable",
        Some(row) => {
            let now = row.opt_text("content")?.unwrap_or_default();
            if now == saved.clone().unwrap_or_default() {
                "ok"
            } else {
                "edited"
            }
        }
    };
    let mut attachments = Vec::new();
    for att in tx.query(
        "SELECT id, original_name, mimetype, size, width, height, duration, playable, \
         source_filename, source_thumbnail_filename FROM bs_bookmark_attachments \
         WHERE bookmark_id = ? ORDER BY position",
        params![&id],
    )? {
        let file = att.text("source_filename")?;
        let available = tx
            .query_one(
                "SELECT 1 AS one FROM attachments WHERE filename = ?",
                params![&file],
            )?
            .is_some();
        attachments.push(json!({
            "id": att.text("id")?,
            "originalName": att.text("original_name")?,
            "mimetype": att.text("mimetype")?,
            "size": att.int("size")?,
            "width": att.opt_int("width")?,
            "height": att.opt_int("height")?,
            "duration": att.opt_number("duration")?,
            "playable": att.opt_flag("playable")?,
            "state": if available { "available" } else { "gone" },
            "filename": available.then_some(file),
            "thumbnailFilename": if available { att.opt_text("source_thumbnail_filename")? } else { None },
        }));
    }
    Ok(json!({
        "id": id,
        "kind": kind.code(),
        "messageId": message,
        "channelId": channel,
        "spaceId": row.opt_text("space_id")?,
        "content": saved,
        "authorId": row.text("author_id")?,
        "authorName": row.text("author_name")?,
        "spaceName": row.opt_text("space_name")?,
        "channelName": row.opt_text("channel_name")?,
        "instanceOrigin": row.text("instance_origin")?,
        "messageCreatedAt": row.int("message_created_at")?,
        "createdAt": row.int("created_at")?,
        "attachments": attachments,
        "status": status,
    }))
}

/// `?cursor=&limit=` списка закладок.
#[derive(Debug, Default, serde::Deserialize)]
pub struct PageQuery {
    cursor: Option<String>,
    limit: Option<i64>,
}

/// `GET /api/bookmarks` — новые сверху, по страницам `(created_at, id)`.
pub async fn bookmarks(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
    let limit = query.limit.unwrap_or(BOOKMARKS_PAGE_DEFAULT);
    if !(1..=BOOKMARKS_PAGE_MAX).contains(&limit) {
        return Err(ApiError::bad_request("validation_failed"));
    }
    let cursor = match query.cursor.as_deref() {
        None => None,
        Some(raw) => {
            let (time, id) = raw
                .split_once(':')
                .and_then(|(time, id)| Some((time.parse::<i64>().ok()?, id.to_owned())))
                .filter(|(_, id)| !id.is_empty())
                .ok_or_else(|| ApiError::bad_request("validation_failed"))?;
            Some((time, id))
        }
    };
    state
        .run(move |tx, _| {
            let rows = match &cursor {
                None => tx.query(
                    &format!(
                        "SELECT {BOOKMARK_COLUMNS} FROM bs_bookmarks WHERE user_id = ? \
                         ORDER BY created_at DESC, id DESC LIMIT ?"
                    ),
                    params![&user.id, limit + 1],
                )?,
                Some((time, id)) => tx.query(
                    &format!(
                        "SELECT {BOOKMARK_COLUMNS} FROM bs_bookmarks WHERE user_id = ? \
                         AND (created_at < ? OR (created_at = ? AND id < ?)) \
                         ORDER BY created_at DESC, id DESC LIMIT ?"
                    ),
                    params![&user.id, *time, *time, id, limit + 1],
                )?,
            };
            let more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
            let mut items = Vec::new();
            for row in rows
                .iter()
                .take(usize::try_from(limit).unwrap_or(usize::MAX))
            {
                items.push(bookmark_json(tx, &user.id, row)?);
            }
            let next = more.then(|| items.last()).flatten().map(|last| {
                format!(
                    "{}:{}",
                    last["createdAt"],
                    last["id"].as_str().unwrap_or_default()
                )
            });
            Ok(Json(json!({"items": items, "nextCursor": next})))
        })
        .await
}

/// `?kind=&channelId=` ключей закладок одного чата.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeysQuery {
    kind: String,
    channel_id: String,
}

/// `GET /api/bookmarks/keys` — какие сообщения чата в закладках.
pub async fn bookmark_keys(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<KeysQuery>,
) -> ApiResult<Json<Value>> {
    let kind = Kind::parse(&query.kind)?;
    state
        .run(move |tx, _| {
            let ids = tx
                .query(
                    "SELECT message_id FROM bs_bookmarks WHERE user_id = ? AND kind = ? AND channel_id = ?",
                    params![&user.id, kind.code(), &query.channel_id],
                )?
                .iter()
                .map(|row| row.text("message_id"))
                .collect::<DbResult<Vec<_>>>()?;
            Ok(Json(json!(ids)))
        })
        .await
}

/// `PUT /api/bookmarks/:kind/:messageId` — 201 новая, 200 уже была.
pub async fn bookmark_add(
    State(state): State<AppState>,
    user: AuthUser,
    Path((kind, message)): Path<(String, String)>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let kind = Kind::parse(&kind)?;
    let app = state.clone();
    state
        .run(move |tx, out| {
            let src = source(tx, kind, &message)?;
            readable(tx, &user.id, kind, &src.channel)
                .map_err(|_| ApiError::not_found("message_not_found"))?;
            if let Some(row) = tx.query_one(
                &format!(
                    "SELECT {BOOKMARK_COLUMNS} FROM bs_bookmarks WHERE user_id = ? AND kind = ? AND message_id = ?"
                ),
                params![&user.id, kind.code(), &message],
            )? {
                return Ok((StatusCode::OK, Json(bookmark_json(tx, &user.id, &row)?)));
            }
            let id = app.ids.next();
            let (space_name, channel_name) = conversation_names(tx, &user.id, kind, &src)?;
            let author_name = name_of(tx, &src.author)?;
            tx.execute(
                &format!(
                    "INSERT INTO bs_bookmarks (user_id, {BOOKMARK_COLUMNS}) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                ),
                params![
                    &user.id,
                    &id,
                    kind.code(),
                    &message,
                    &src.channel,
                    &src.space,
                    &src.content,
                    &src.author,
                    &author_name,
                    &space_name,
                    &channel_name,
                    crate::federation::our_origin(&app.config),
                    src.created_at,
                    now_ms()
                ],
            )?;
            let column = match kind {
                Kind::Channel => "message_id",
                Kind::Dm => "dm_message_id",
            };
            let files = tx.query(
                &format!(
                    "SELECT original_name, mimetype, size, width, height, duration, playable, \
                     filename, thumbnail_filename FROM attachments WHERE {column} = ? ORDER BY created_at, id"
                ),
                params![&message],
            )?;
            for (position, file) in files.iter().enumerate() {
                tx.execute(
                    "INSERT INTO bs_bookmark_attachments (id, bookmark_id, position, original_name, \
                     mimetype, size, width, height, duration, playable, source_filename, \
                     source_thumbnail_filename) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        app.ids.next(),
                        &id,
                        i64::try_from(position).unwrap_or(i64::MAX),
                        file.text("original_name")?,
                        file.text("mimetype")?,
                        file.int("size")?,
                        file.opt_int("width")?,
                        file.opt_int("height")?,
                        file.opt_number("duration")?.and_then(|v| v.as_f64()),
                        file.opt_int("playable")?,
                        file.text("filename")?,
                        file.opt_text("thumbnail_filename")?
                    ],
                )?;
            }
            let row = tx
                .query_one(
                    &format!("SELECT {BOOKMARK_COLUMNS} FROM bs_bookmarks WHERE id = ?"),
                    params![&id],
                )?
                .ok_or_else(ApiError::internal)?;
            let bookmark = bookmark_json(tx, &user.id, &row)?;
            broadcast::to_user(
                tx,
                out,
                &user.id,
                &json!({"type": "bookmark_added", "bookmark": bookmark}),
            )?;
            Ok((StatusCode::CREATED, Json(bookmark)))
        })
        .await
}

/// `DELETE /api/bookmarks/:kind/:messageId` — успех и без закладки.
pub async fn bookmark_remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path((kind, message)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let kind = Kind::parse(&kind)?;
    state
        .run(move |tx, out| {
            let found = tx.query_one(
                "SELECT id, channel_id FROM bs_bookmarks WHERE user_id = ? AND kind = ? AND message_id = ?",
                params![&user.id, kind.code(), &message],
            )?;
            if let Some(row) = found {
                let id = row.text("id")?;
                tx.execute("DELETE FROM bs_bookmark_attachments WHERE bookmark_id = ?", params![&id])?;
                tx.execute("DELETE FROM bs_bookmarks WHERE id = ?", params![&id])?;
                broadcast::to_user(
                    tx,
                    out,
                    &user.id,
                    &json!({
                        "type": "bookmark_removed",
                        "kind": kind.code(),
                        "channelId": row.text("channel_id")?,
                        "messageId": &message,
                    }),
                )?;
            }
            Ok(Json(json!({"success": true})))
        })
        .await
}
