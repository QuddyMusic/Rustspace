//! Пользователи: вид пользователя в ответах и собственный профиль.

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Row, Tx};
use crate::error::{ApiError, ApiResult};
use crate::params;
use crate::state::AppState;

/// Столбцы, из которых собирается вид пользователя.
pub const USER_COLUMNS: &str = "id, username, display_name, avatar, banner, accent_color, \
    avatar_color, bio, status, custom_status, is_admin, is_deleted, discoverable, \
    profile_updated_at, created_at, home_instance, home_user_id, replicated_instances, \
    show_activity, federation_home_orphaned, detached_home_instance";

/// Пользователь в ответе API (`sanitizeUser` оригинала): без хеша пароля,
/// удалённый — обезличенным; `is_self` добавляет поля собственного профиля.
///
/// # Errors
/// В строке нет нужных столбцов (запрос должен брать [`USER_COLUMNS`]).
pub fn sanitize_user(row: &Row, is_self: bool) -> DbResult<Value> {
    if row.opt_flag("is_deleted")? == Some(true) {
        let mut user = json!({
            "id": row.text("id")?,
            "username": "Deleted User",
            "displayName": null,
            "avatar": null,
            "banner": null,
            "accentColor": null,
            "avatarColor": null,
            "bio": null,
            "status": "offline",
            "customStatus": null,
            "isAdmin": false,
            "isDeleted": true,
            "discoverable": false,
            "profileUpdatedAt": 0,
            "createdAt": row.int("created_at")?,
            "homeInstance": null,
            "homeUserId": null,
            "replicatedInstances": [],
        });
        if is_self {
            user["showActivity"] = json!(false);
        }
        return Ok(user);
    }
    let replicated = row
        .opt_text("replicated_instances")?
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_array)
        .unwrap_or_else(|| json!([]));
    let mut user = json!({
        "id": row.text("id")?,
        "username": row.text("username")?,
        "displayName": row.opt_text("display_name")?,
        "avatar": row.opt_text("avatar")?,
        "banner": row.opt_text("banner")?,
        "accentColor": row.opt_text("accent_color")?,
        "avatarColor": row.opt_text("avatar_color")?,
        "bio": row.opt_text("bio")?,
        "status": row.opt_text("status")?.unwrap_or_else(|| "offline".to_owned()),
        "customStatus": row.opt_text("custom_status")?,
        "isAdmin": row.opt_flag("is_admin")? == Some(true),
        "discoverable": row.opt_flag("discoverable")? != Some(false),
        "profileUpdatedAt": row.opt_int("profile_updated_at")?.unwrap_or(0),
        "createdAt": row.int("created_at")?,
        "homeInstance": row.opt_text("home_instance")?,
        "homeUserId": row.opt_text("home_user_id")?,
        "replicatedInstances": replicated,
    });
    if is_self {
        user["showActivity"] = json!(row.opt_flag("show_activity")? != Some(false));
        user["federationHomeOrphaned"] =
            json!(row.opt_flag("federation_home_orphaned")? == Some(true));
        user["detachedHomeInstance"] = json!(row.opt_text("detached_home_instance")?);
    }
    Ok(user)
}

/// [`USER_COLUMNS`] с префиксом таблицы — для запросов с соединениями.
#[must_use]
pub fn user_columns(alias: &str) -> String {
    USER_COLUMNS
        .split(',')
        .map(|column| format!("{alias}.{}", column.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Строка пользователя по идентификатору.
///
/// # Errors
/// Ошибка базы.
pub fn user_row(tx: &mut dyn Tx, id: &str) -> DbResult<Option<Row>> {
    tx.query_one(
        &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"),
        params![id],
    )
}

/// `GET /api/users/@me`.
pub async fn me(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    state
        .db
        .transaction(move |tx| {
            let row = user_row(tx, &user.id)?
                .filter(|row| row.opt_flag("is_deleted").ok().flatten() != Some(true))
                .ok_or_else(|| ApiError::unauthorized("account_deleted"))?;
            Ok(Json(sanitize_user(&row, true)?))
        })
        .await
}
