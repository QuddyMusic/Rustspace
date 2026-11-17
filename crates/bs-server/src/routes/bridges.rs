//! Связи каналов с внешними чатами (мосты Rustspace; у оригинала их нет).
//!
//! - `GET /api/rustspace/channels/:id/bridges` — какие сети доступны на
//!   сервере и с какими чатами связан канал;
//! - `POST /api/rustspace/channels/:id/bridges` `{kind, externalId}` — связать;
//! - `DELETE /api/rustspace/bridges/:linkId` — развязать.
//!
//! Нужно право «Управлять каналами» в пространстве канала.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use bs_permissions::Permissions;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::bridges::{Kind, available_codes, links_json};
use crate::db::Tx;
use crate::error::{ApiError, ApiResult};
use crate::extract::JsonBody;
use crate::messages::channel_space;
use crate::state::{AppState, now_ms};
use crate::util::str_field;
use crate::{params, perms};

/// Пространство канала, если пользователь может управлять его каналами.
fn managed_space(tx: &mut dyn Tx, user: &str, channel: &str) -> ApiResult<String> {
    let space =
        channel_space(tx, channel)?.ok_or_else(|| ApiError::not_found("channel_not_found"))?;
    if !perms::has_channel_permission(tx, user, &space, channel, Permissions::MANAGE_CHANNELS)? {
        return Err(perms::missing("MANAGE_CHANNELS"));
    }
    Ok(space)
}

/// `GET /api/rustspace/channels/:id/bridges`.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(channel): Path<String>,
) -> ApiResult<Json<Value>> {
    let available = available_codes(&state.config);
    let links = state
        .run(move |tx, _| {
            managed_space(tx, &user.id, &channel)?;
            Ok(links_json(tx, &channel)?)
        })
        .await?;
    Ok(Json(json!({"available": available, "links": links})))
}

/// `POST /api/rustspace/channels/:id/bridges`.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Path(channel): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let kind = str_field(&body, "kind")
        .and_then(Kind::parse)
        .ok_or_else(|| ApiError::bad_request("bridge_kind_invalid"))?;
    if !state.config.bridges.available().contains(&kind) {
        return Err(ApiError::bad_request("bridge_not_configured"));
    }
    let external = str_field(&body, "externalId")
        .map(str::trim)
        .filter(|id| kind.valid_chat(id))
        .ok_or_else(|| ApiError::bad_request("bridge_chat_invalid"))?
        .to_owned();
    let id = state.ids.next();
    let link = state
        .run(move |tx, _| {
            managed_space(tx, &user.id, &channel)?;
            if tx
                .query_one(
                    "SELECT id FROM bs_bridge_links WHERE channel_id = ? AND kind = ? AND external_id = ?",
                    params![&channel, kind.code(), &external],
                )?
                .is_some()
            {
                return Err(ApiError::conflict("bridge_exists"));
            }
            let created = now_ms();
            tx.execute(
                "INSERT INTO bs_bridge_links (id, channel_id, kind, external_id, created_by, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![&id, &channel, kind.code(), &external, &user.id, created],
            )?;
            Ok(json!({"id": id, "kind": kind.code(), "externalId": external, "createdAt": created}))
        })
        .await?;
    Ok((StatusCode::CREATED, Json(link)))
}

/// `DELETE /api/rustspace/bridges/:linkId`.
pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(link): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let row = tx
                .query_one(
                    "SELECT channel_id FROM bs_bridge_links WHERE id = ?",
                    params![&link],
                )?
                .ok_or_else(|| ApiError::not_found("bridge_not_found"))?;
            managed_space(tx, &user.id, &row.text("channel_id")?)?;
            tx.execute("DELETE FROM bs_bridge_links WHERE id = ?", params![&link])?;
            Ok(())
        })
        .await?;
    Ok(Json(json!({"success": true})))
}
