//! Сообщения каналов по REST (`routes/messages.ts`).

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bs_permissions::Permissions;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ClientIp, JsonBody};
use crate::messages::{
    MAX_MESSAGE_LENGTH, MESSAGE_COLUMNS, Page, Shape, channel_space, message_json, message_row,
    parse_page, reply_target_in_channel, select_page,
};
use crate::ratelimit::Limit;
use crate::state::{AppState, now_ms};
use crate::util::{field, js_trim, str_field, truthy, utf16_len};
use crate::{broadcast, embeds, files, mentions, params, perms};

const SEND_LIMIT: Limit = Limit {
    name: "messages.send",
    max: 5,
    window: Duration::from_secs(5),
};

fn has(
    tx: &mut dyn Tx,
    user: &str,
    space: &str,
    channel: &str,
    flag: Permissions,
) -> DbResult<bool> {
    perms::has_channel_permission(tx, user, space, channel, flag)
}

/// Проверяет вложения, которые автор хочет прикрепить к новому сообщению.
///
/// # Errors
/// Вложение не найдено, уже прикреплено или загружено другим пользователем.
pub fn check_attachments(tx: &mut dyn Tx, user_id: &str, ids: &[String]) -> ApiResult<()> {
    for id in ids {
        let row = tx.query_one(
            "SELECT message_id, dm_message_id, uploader_id FROM attachments WHERE id = ?",
            params![id],
        )?;
        let Some(row) = row else {
            return Err(ApiError::bad_request("attachment_invalid"));
        };
        if row.opt_text("message_id")?.is_some() || row.opt_text("dm_message_id")?.is_some() {
            return Err(ApiError::bad_request("attachment_invalid"));
        }
        // Вложение без известного автора оригинал отдаёт любому. Здесь оно
        // не достаётся никому: чужой идентификатор не даёт доступа к файлу.
        if row.opt_text("uploader_id")?.as_deref() != Some(user_id) {
            return Err(ApiError::bad_request("attachment_not_owned"));
        }
    }
    Ok(())
}

/// Идентификаторы вложений из тела запроса.
#[must_use]
pub fn attachment_ids(body: &Value) -> Vec<String> {
    field(body, "attachments")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// `GET /api/channels/:id/messages`.
pub async fn history(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
) -> ApiResult<Response> {
    let page = parse_page(&query).map_err(ApiError::bad_request)?;
    let after = matches!(page, Page::After(..));
    let list = state
        .run(move |tx, _| {
            let space =
                channel_space(tx, &id)?.ok_or_else(|| ApiError::not_found("channel_not_found"))?;
            let needed = Permissions::VIEW_CHANNEL | Permissions::READ_MESSAGE_HISTORY;
            if !has(tx, &user.id, &space, &id, needed)? {
                return Err(perms::missing("VIEW_CHANNEL or READ_MESSAGE_HISTORY"));
            }
            let rows = select_page(tx, MESSAGE_COLUMNS, "messages", "channel_id", &id, &page)?;
            let mut list = Vec::with_capacity(rows.len());
            for row in &rows {
                if let Some(message) = message_json(tx, row, Shape::Rest, true, true)? {
                    list.push(message);
                }
            }
            Ok(list)
        })
        .await?;
    let mut response = Json(json!(list)).into_response();
    if after {
        response.headers_mut().insert(
            HeaderName::from_static("x-backspace-paging"),
            HeaderValue::from_static("after"),
        );
    }
    Ok(response)
}

/// `POST /api/channels/:id/messages`.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<(StatusCode, Json<Value>)> {
    state.limiter.check(SEND_LIMIT, ip)?;
    let ids = state.ids.clone();
    let hub = state.hub.clone();
    let channel = id.clone();
    let (message, space, content) = state
        .run(move |tx, out| {
            let space =
                channel_space(tx, &id)?.ok_or_else(|| ApiError::not_found("channel_not_found"))?;
            if !has(tx, &user.id, &space, &id, Permissions::SEND_MESSAGES)? {
                return Err(perms::missing("SEND_MESSAGES"));
            }
            let attachments = attachment_ids(&body);
            if !attachments.is_empty()
                && !has(tx, &user.id, &space, &id, Permissions::ATTACH_FILES)?
            {
                return Err(perms::missing("ATTACH_FILES"));
            }
            let raw = str_field(&body, "content");
            let content = raw
                .map(js_trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned);
            if content.is_none() && attachments.is_empty() {
                return Err(ApiError::bad_request("message_empty"));
            }
            if raw.is_some_and(|text| utf16_len(text) > MAX_MESSAGE_LENGTH) {
                return Err(
                    ApiError::bad_request("content_too_long").detail("max", MAX_MESSAGE_LENGTH)
                );
            }
            if mentions::has_mass(raw)
                && !has(tx, &user.id, &space, &id, Permissions::MENTION_EVERYONE)?
            {
                return Err(perms::missing("MENTION_EVERYONE"));
            }
            let reply_to = field(&body, "replyToId").filter(|value| truthy(Some(value)));
            let reply_to = match reply_to {
                Some(target) => {
                    let target = target.as_str().unwrap_or_default();
                    if !reply_target_in_channel(tx, &id, target)? {
                        return Err(ApiError::bad_request("reply_target_invalid"));
                    }
                    Some(target.to_owned())
                }
                None => None,
            };
            check_attachments(tx, &user.id, &attachments)?;
            let message_id = ids.next();
            tx.execute(
                "INSERT INTO messages (id, channel_id, user_id, reply_to_id, content, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![&message_id, &id, &user.id, &reply_to, &content, now_ms()],
            )?;
            for attachment in &attachments {
                tx.execute(
                    "UPDATE attachments SET message_id = ? WHERE id = ?",
                    params![&message_id, attachment],
                )?;
            }
            let row = message_row(tx, &message_id)?.ok_or_else(ApiError::internal)?;
            let message = message_json(tx, &row, Shape::Rest, false, false)?
                .ok_or_else(ApiError::internal)?;
            broadcast::to_channel(
                tx,
                &hub,
                out,
                &space,
                &id,
                &json!({"type": "message_created", "message": message}),
                None,
            )?;
            Ok((message, space, content))
        })
        .await?;
    let message_id = message["id"].as_str().unwrap_or_default().to_owned();
    embeds::schedule(
        &state,
        message_id,
        content,
        embeds::Place::Channel {
            space_id: space,
            channel_id: channel,
        },
    );
    Ok((StatusCode::CREATED, Json(message)))
}

/// Может ли автор всё ещё править своё сообщение. Оригинал проверяет только
/// авторство, поэтому исключённый или забаненный продолжает переписывать
/// старые сообщения в пространстве, куда ему закрыт доступ. Здесь правка
/// требует, чтобы автор по-прежнему видел канал.
///
/// # Errors
/// Ошибка базы.
pub fn author_still_sees(tx: &mut dyn Tx, user_id: &str, channel_id: &str) -> DbResult<bool> {
    let Some(space) = channel_space(tx, channel_id)? else {
        return Ok(false);
    };
    has(tx, user_id, &space, channel_id, Permissions::VIEW_CHANNEL)
}

/// `PATCH /api/messages/:id`.
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let raw = str_field(&body, "content")
        .filter(|text| !js_trim(text).is_empty())
        .ok_or_else(|| ApiError::bad_request("content_required"))?
        .to_owned();
    if utf16_len(&raw) > MAX_MESSAGE_LENGTH {
        return Err(ApiError::bad_request("content_too_long").detail("max", MAX_MESSAGE_LENGTH));
    }
    let hub = state.hub.clone();
    let content = js_trim(&raw).to_owned();
    let trimmed = content.clone();
    let message_id = id.clone();
    let (message, place) = state
        .run(move |tx, out| {
            let message =
                message_row(tx, &id)?.ok_or_else(|| ApiError::not_found("message_not_found"))?;
            if message.text("type")? == "system" {
                return Err(ApiError::forbidden("system_message_immutable"));
            }
            if message.text("user_id")? != user.id {
                return Err(ApiError::forbidden("message_edit_not_author"));
            }
            let channel = message.text("channel_id")?;
            let space = channel_space(tx, &channel)?;
            if !author_still_sees(tx, &user.id, &channel)? {
                return Err(perms::missing("VIEW_CHANNEL"));
            }
            if mentions::adds_mass(message.opt_text("content")?.as_deref(), Some(&raw)) {
                let allowed = match &space {
                    Some(space) => {
                        has(tx, &user.id, space, &channel, Permissions::MENTION_EVERYONE)?
                    }
                    None => false,
                };
                if !allowed {
                    return Err(perms::missing("MENTION_EVERYONE"));
                }
            }
            tx.execute(
                "UPDATE messages SET content = ?, edited_at = ? WHERE id = ?",
                params![&trimmed, now_ms(), &id],
            )?;
            tx.execute("DELETE FROM embeds WHERE message_id = ?", params![&id])?;
            let updated = message_row(tx, &id)?.ok_or_else(ApiError::internal)?;
            let data = message_json(tx, &updated, Shape::Rest, false, true)?
                .ok_or_else(ApiError::internal)?;
            let mut place = None;
            if let Some(space) = space {
                broadcast::to_channel(
                    tx,
                    &hub,
                    out,
                    &space,
                    &channel,
                    &json!({"type": "message_updated", "message": data}),
                    None,
                )?;
                place = Some(embeds::Place::Channel {
                    space_id: space,
                    channel_id: channel,
                });
            }
            Ok((data, place))
        })
        .await?;
    if let Some(place) = place {
        embeds::schedule(&state, message_id, Some(content), place);
    }
    Ok(Json(message))
}

/// Удаляет сообщение канала вместе с вложениями и возвращает имена файлов,
/// которые надо стереть с диска.
///
/// # Errors
/// Ошибка базы.
pub fn delete_message(tx: &mut dyn Tx, message_id: &str) -> DbResult<Vec<String>> {
    let stale = tx
        .query(
            "SELECT filename FROM attachments WHERE message_id = ?",
            params![message_id],
        )?
        .iter()
        .map(|row| row.text("filename"))
        .collect::<DbResult<Vec<_>>>()?;
    tx.execute(
        "DELETE FROM attachments WHERE message_id = ?",
        params![message_id],
    )?;
    tx.execute("DELETE FROM messages WHERE id = ?", params![message_id])?;
    Ok(stale)
}

/// `DELETE /api/messages/:id`.
pub async fn delete(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let hub = state.hub.clone();
    let config = state.config.clone();
    let stale = state
        .run(move |tx, out| {
            let message =
                message_row(tx, &id)?.ok_or_else(|| ApiError::not_found("message_not_found"))?;
            let channel = message.text("channel_id")?;
            let space = channel_space(tx, &channel)?
                .ok_or_else(|| ApiError::not_found("channel_not_found"))?;
            let author = message.text("user_id")? == user.id;
            if !author && !has(tx, &user.id, &space, &channel, Permissions::MANAGE_MESSAGES)? {
                return Err(ApiError::forbidden("message_delete_forbidden"));
            }
            let stale = delete_message(tx, &id)?;
            broadcast::to_channel(
                tx,
                &hub,
                out,
                &space,
                &channel,
                &json!({"type": "message_deleted", "messageId": id, "channelId": channel}),
                None,
            )?;
            Ok(stale)
        })
        .await?;
    for file in &stale {
        files::delete_upload_file(&config, file).await;
    }
    Ok(Json(json!({"success": true})))
}
