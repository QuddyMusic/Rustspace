//! Вебхуки каналов и сообщения с карточками (Rustspace; у оригинала их нет).
//!
//! - `GET /api/rustspace/channels/:id/webhooks` — вебхуки канала;
//! - `POST /api/rustspace/channels/:id/webhooks` `{name}` — создать;
//! - `DELETE /api/rustspace/webhooks/:id` — удалить;
//! - `POST /api/webhooks/:id/:token` — написать в канал (как у Discord);
//! - `POST /api/rustspace/channels/:id/cards` `{content?, embeds}` —
//!   сообщение с карточками от своего имени.
//!
//! Вебхуками управляют те, кто может управлять каналами.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bs_permissions::Permissions;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::Tx;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ClientIp, JsonBody};
use crate::messages::channel_space;
use crate::ratelimit::Limit;
use crate::state::{AppState, now_ms};
use crate::util::str_field;
use crate::webhooks::{cards, content, hook_json, new_token, post, post_dm, puppet};
use crate::{params, perms};

/// Вебхук — не больше 5 сообщений за 2 секунды с адреса (как у Discord).
const HOOK_LIMIT: Limit = Limit {
    name: "webhooks.execute",
    max: 5,
    window: Duration::from_secs(2),
};

/// Вебхуков в канале.
const MAX_HOOKS: i64 = 15;

/// Пространство канала, если пользователь может управлять его каналами.
fn managed_space(tx: &mut dyn Tx, user: &str, channel: &str) -> ApiResult<String> {
    let space =
        channel_space(tx, channel)?.ok_or_else(|| ApiError::not_found("channel_not_found"))?;
    if !perms::has_channel_permission(tx, user, &space, channel, Permissions::MANAGE_CHANNELS)? {
        return Err(perms::missing("MANAGE_CHANNELS"));
    }
    Ok(space)
}

/// Имя вебхука или автора: 1–80 знаков.
fn hook_name(value: Option<&str>) -> Option<String> {
    let name = value?.trim();
    (!name.is_empty() && name.chars().count() <= 80).then(|| name.to_owned())
}

/// `GET /api/rustspace/channels/:id/webhooks`.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(channel): Path<String>,
) -> ApiResult<Json<Value>> {
    let origin = crate::federation::our_origin(&state.config);
    let hooks = state
        .run(move |tx, _| {
            managed_space(tx, &user.id, &channel)?;
            let rows = tx.query(
                "SELECT id, channel_id, name, avatar, token, created_at FROM bs_webhooks \
                 WHERE channel_id = ? ORDER BY created_at",
                params![&channel],
            )?;
            Ok(rows
                .iter()
                .map(|row| hook_json(row, &origin))
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await?;
    Ok(Json(json!(hooks)))
}

/// `POST /api/rustspace/channels/:id/webhooks`.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Path(channel): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = hook_name(str_field(&body, "name"))
        .ok_or_else(|| ApiError::bad_request("webhook_name_invalid"))?;
    let origin = crate::federation::our_origin(&state.config);
    let id = state.ids.next();
    let hook = state
        .run(move |tx, _| {
            managed_space(tx, &user.id, &channel)?;
            let count = tx
                .query_one(
                    "SELECT COUNT(*) AS n FROM bs_webhooks WHERE channel_id = ?",
                    params![&channel],
                )?
                .map_or(Ok(0), |row| row.int("n"))?;
            if count >= MAX_HOOKS {
                return Err(ApiError::bad_request("webhook_limit").detail("max", MAX_HOOKS));
            }
            tx.execute(
                "INSERT INTO bs_webhooks (id, channel_id, name, avatar, token, created_by, created_at) \
                 VALUES (?, ?, ?, NULL, ?, ?, ?)",
                params![&id, &channel, &name, new_token(), &user.id, now_ms()],
            )?;
            let row = tx
                .query_one(
                    "SELECT id, channel_id, name, avatar, token, created_at FROM bs_webhooks WHERE id = ?",
                    params![&id],
                )?
                .ok_or_else(ApiError::internal)?;
            Ok(hook_json(&row, &origin)?)
        })
        .await?;
    Ok((StatusCode::CREATED, Json(hook)))
}

/// `DELETE /api/rustspace/webhooks/:id`.
pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(hook): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let row = tx
                .query_one(
                    "SELECT channel_id FROM bs_webhooks WHERE id = ?",
                    params![&hook],
                )?
                .ok_or_else(|| ApiError::not_found("webhook_not_found"))?;
            managed_space(tx, &user.id, &row.text("channel_id")?)?;
            tx.execute("DELETE FROM bs_webhooks WHERE id = ?", params![&hook])?;
            Ok(())
        })
        .await?;
    Ok(Json(json!({"success": true})))
}

/// `?wait=true`: вернуть созданное сообщение, как у Discord.
#[derive(Debug, Default, serde::Deserialize)]
pub struct Wait {
    wait: Option<String>,
}

/// `POST /api/webhooks/:id/:token` — без входа, по секрету в адресе.
pub async fn execute(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Path((hook, token)): Path<(String, String)>,
    Query(query): Query<Wait>,
    JsonBody(body): JsonBody,
) -> ApiResult<Response> {
    state.limiter.check(HOOK_LIMIT, ip)?;
    let content = content(&body)?;
    let cards = cards(&body)?;
    if content.is_none() && cards.is_empty() {
        return Err(ApiError::bad_request("message_empty"));
    }
    let username = hook_name(str_field(&body, "username"));
    let avatar = str_field(&body, "avatar_url")
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
        .map(str::to_owned);
    let app = state.clone();
    let text = content.clone();
    let (message, channel) = state
        .run(move |tx, out| {
            let row = tx
                .query_one(
                    "SELECT channel_id, name, avatar, token FROM bs_webhooks WHERE id = ?",
                    params![&hook],
                )?
                .filter(|row| row.text("token").is_ok_and(|saved| saved == token))
                .ok_or_else(|| ApiError::not_found("webhook_not_found"))?;
            let channel = row.text("channel_id")?;
            let space = channel_space(tx, &channel)?
                .ok_or_else(|| ApiError::not_found("channel_not_found"))?;
            let name = username.unwrap_or(row.text("name")?);
            let avatar = avatar.or(row.opt_text("avatar")?);
            let author = puppet(tx, &app, &hook, &name, avatar.as_deref())?;
            let message = post(
                tx,
                out,
                &app,
                &space,
                &channel,
                &author,
                text.as_deref(),
                &cards,
            )?;
            Ok((message, channel))
        })
        .await?;
    let id = message["id"].as_str().unwrap_or_default().to_owned();
    tracing::debug!(channel, "вебхук: сообщение");
    crate::bridges::notify(&state, &id);
    if query.wait.as_deref() == Some("true") {
        Ok(Json(message).into_response())
    } else {
        Ok(StatusCode::NO_CONTENT.into_response())
    }
}

/// `POST /api/rustspace/channels/:id/cards` — сообщение с карточками.
pub async fn send_cards(
    State(state): State<AppState>,
    user: AuthUser,
    ClientIp(ip): ClientIp,
    Path(channel): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<(StatusCode, Json<Value>)> {
    state.limiter.check(HOOK_LIMIT, ip)?;
    let content = content(&body)?;
    let cards = cards(&body)?;
    if cards.is_empty() {
        return Err(ApiError::bad_request("message_empty"));
    }
    let app = state.clone();
    let message = state
        .run(move |tx, out| {
            let space = channel_space(tx, &channel)?
                .ok_or_else(|| ApiError::not_found("channel_not_found"))?;
            if !perms::has_channel_permission(
                tx,
                &user.id,
                &space,
                &channel,
                Permissions::SEND_MESSAGES,
            )? {
                return Err(perms::missing("SEND_MESSAGES"));
            }
            if crate::mentions::has_mass(content.as_deref())
                && !perms::has_channel_permission(
                    tx,
                    &user.id,
                    &space,
                    &channel,
                    Permissions::MENTION_EVERYONE,
                )?
            {
                return Err(perms::missing("MENTION_EVERYONE"));
            }
            post(
                tx,
                out,
                &app,
                &space,
                &channel,
                &user.id,
                content.as_deref(),
                &cards,
            )
        })
        .await?;
    let id = message["id"].as_str().unwrap_or_default().to_owned();
    crate::bridges::notify(&state, &id);
    Ok((StatusCode::CREATED, Json(message)))
}

/// `POST /api/rustspace/dm/:id/cards` `{content?, embeds}` — сообщение с
/// карточками в личной переписке (в том числе в «Избранном»).
pub async fn send_dm_cards(
    State(state): State<AppState>,
    user: AuthUser,
    ClientIp(ip): ClientIp,
    Path(channel): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<(StatusCode, Json<Value>)> {
    state.limiter.check(HOOK_LIMIT, ip)?;
    let content = content(&body)?;
    let cards = cards(&body)?;
    if cards.is_empty() {
        return Err(ApiError::bad_request("message_empty"));
    }
    let app = state.clone();
    let message = state
        .run(move |tx, out| {
            post_dm(
                tx,
                out,
                &app,
                &channel,
                &user.id,
                content.as_deref(),
                &cards,
            )
        })
        .await?;
    Ok((StatusCode::CREATED, Json(message)))
}
