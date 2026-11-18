//! Боты (расширение Rustspace): `/api/rustspace/bots…` — свои боты и их
//! токены, `/api/rustspace/spaces/:id/bots` — добавить бота в пространство.
//! См. [`crate::bots`].

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::bots;
use crate::error::{ApiError, ApiResult};
use crate::extract::JsonBody;
use crate::state::AppState;

fn text(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// Поле, которое можно стереть: нет ключа — не трогать, `null` — стереть.
fn nullable(body: &Value, key: &str) -> Option<Option<String>> {
    body.get(key).map(|value| value.as_str().map(str::to_owned))
}

/// `GET /api/rustspace/bots` — свои боты.
pub async fn list(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| Ok(Json(Value::Array(bots::list(tx, &user.id)?))))
        .await
}

/// `POST /api/rustspace/bots` `{username, displayName?, description?}` →
/// `{bot, token}`; токен показывается только здесь.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let username =
        text(&body, "username").ok_or_else(|| ApiError::bad_request("validation_failed"))?;
    let display = text(&body, "displayName");
    let description = text(&body, "description");
    let app = state.clone();
    state
        .run(move |tx, _| {
            let (bot, token) = bots::create(
                tx,
                &app,
                &user.id,
                &username,
                display.as_deref(),
                description.as_deref(),
            )?;
            Ok((
                StatusCode::CREATED,
                Json(json!({"bot": bot, "token": token})),
            ))
        })
        .await
}

/// `PATCH /api/rustspace/bots/:id` `{displayName?, description?}`.
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let display = nullable(&body, "displayName");
    let description = nullable(&body, "description");
    state
        .run(move |tx, _| Ok(Json(bots::update(tx, &user.id, &id, display, description)?)))
        .await
}

/// `POST /api/rustspace/bots/:id/token` → `{token}` — новый токен.
pub async fn regenerate(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| Ok(Json(json!({"token": bots::regenerate(tx, &user.id, &id)?}))))
        .await
}

/// `DELETE /api/rustspace/bots/:id`.
pub async fn delete(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, out| {
            bots::delete(tx, out, &user.id, &id)?;
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `PUT /api/rustspace/spaces/:space/bots/:bot` — добавить своего бота.
/// Убрать — обычным `DELETE /api/spaces/:space/members/:bot`.
pub async fn add_to_space(
    State(state): State<AppState>,
    user: AuthUser,
    Path((space, bot)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, out| {
            bots::add_to_space(tx, out, &user.id, &space, &bot)?;
            Ok(Json(json!({"success": true})))
        })
        .await
}
