//! Приватность пользователя (Rustspace; у оригинала её нет).
//!
//! Сейчас одна настройка: при пересылке сообщений не давать ссылку на свой
//! профиль (пересланное подписывается именем без упоминания).
//!
//! - `GET /api/rustspace/privacy` → `{hideForwardAuthor}` — свои настройки;
//! - `PUT /api/rustspace/privacy` `{hideForwardAuthor}`;
//! - `GET /api/rustspace/users/:id/privacy` → `{hideForwardAuthor}` — то, что
//!   нужно знать пересылающему.

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::JsonBody;
use crate::params;
use crate::state::AppState;

fn hide_forward_author(tx: &mut dyn Tx, user: &str) -> DbResult<bool> {
    Ok(tx
        .query_one(
            "SELECT hide_forward_author FROM bs_user_privacy WHERE user_id = ?",
            params![user],
        )?
        .and_then(|row| row.int("hide_forward_author").ok())
        .is_some_and(|value| value != 0))
}

/// `GET /api/rustspace/privacy`.
pub async fn get_own(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    let hide = state
        .run(move |tx, _| Ok(hide_forward_author(tx, &user.id)?))
        .await?;
    Ok(Json(json!({ "hideForwardAuthor": hide })))
}

/// `PUT /api/rustspace/privacy`.
pub async fn put_own(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let hide = body
        .get("hideForwardAuthor")
        .and_then(Value::as_bool)
        .ok_or_else(|| ApiError::bad_request("validation_failed"))?;
    state
        .run(move |tx, _| {
            tx.execute(
                "INSERT INTO bs_user_privacy (user_id, hide_forward_author) VALUES (?, ?) \
                 ON CONFLICT (user_id) DO UPDATE SET hide_forward_author = excluded.hide_forward_author",
                params![user.id, i64::from(hide)],
            )?;
            Ok(())
        })
        .await?;
    Ok(Json(json!({ "hideForwardAuthor": hide })))
}

/// `GET /api/rustspace/users/:id/privacy`.
pub async fn get_user(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let hide = state
        .run(move |tx, _| Ok(hide_forward_author(tx, &id)?))
        .await?;
    Ok(Json(json!({ "hideForwardAuthor": hide })))
}
