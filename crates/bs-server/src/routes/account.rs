//! Аккаунт пользователя: пароль, профиль, удаление, раскладка пространств
//! (`routes/users.ts`).

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rand::Rng;
use regex::Regex;
use serde_json::{Value, json};

use crate::auth::{self, AuthUser};
use crate::db::{DbResult, Tx};
use crate::error::{ApiError, ApiResult, error_text};
use crate::extract::{ClientIp, JsonBody};
use crate::hub::Outbox;
use crate::ratelimit::Limit;
use crate::routes::auth::AVATAR_COLORS;
use crate::routes::users::{USER_COLUMNS, sanitize_user, user_row};
use crate::state::{AppState, now_ms};
use crate::util::{field, js_trim, str_field, truthy, utf16_len};
use crate::{events, federation, files, params, ready, ws};

const PASSWORD_MIN_LENGTH: usize = 8;
const CHANGE_PASSWORD_LIMIT: Limit = Limit {
    name: "users.change-password",
    max: 5,
    window: Duration::from_mins(15),
};
const DELETE_LIMIT: Limit = Limit {
    name: "users.delete",
    max: 3,
    window: Duration::from_mins(15),
};
/// Проверка пароля — подбор по одному ответу «да/нет». Оригинал её не
/// ограничивает; здесь тот же порог, что у входа.
const VERIFY_LIMIT: Limit = Limit {
    name: "users.verify-password",
    max: 15,
    window: Duration::from_mins(2),
};

fn validation(field_name: &str, reason: &str) -> ApiError {
    ApiError::bad_request("validation_failed")
        .detail("field", field_name)
        .detail("reason", reason)
}

/// Пароль и признак «аккаунтом управляет этот инстанс».
async fn credentials(
    state: &AppState,
    user_id: String,
) -> ApiResult<(Option<String>, bool, String)> {
    state
        .run(move |tx, _| {
            let row = tx
                .query_one(
                    "SELECT username, password_hash, home_instance, federation_home_orphaned \
                     FROM users WHERE id = ?",
                    params![&user_id],
                )?
                .ok_or_else(|| ApiError::not_found("user_not_found"))?;
            let owns = row
                .opt_text("home_instance")?
                .is_none_or(|home| home.is_empty())
                || row.opt_flag("federation_home_orphaned")? == Some(true);
            Ok((row.opt_text("password_hash")?, owns, row.text("username")?))
        })
        .await
}

/// `POST /api/users/@me/verify-password`.
pub async fn verify_password(
    State(state): State<AppState>,
    user: AuthUser,
    ClientIp(ip): ClientIp,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    state.limiter.check(VERIFY_LIMIT, ip)?;
    let password = str_field(&body, "password")
        .filter(|password| !password.is_empty())
        .ok_or_else(|| ApiError::bad_request("password_required"))?
        .to_owned();
    let (hash, _, _) = credentials(&state, user.id).await?;
    let valid = auth::verify_password(&state, password, hash).await;
    Ok(Json(json!({"valid": valid})))
}

/// `POST /api/users/@me/change-password`.
pub async fn change_password(
    State(state): State<AppState>,
    user: AuthUser,
    ClientIp(ip): ClientIp,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    state.limiter.check(CHANGE_PASSWORD_LIMIT, ip)?;
    let new_password = str_field(&body, "newPassword")
        .filter(|password| utf16_len(password) >= PASSWORD_MIN_LENGTH)
        .ok_or_else(|| {
            ApiError::bad_request("password_too_short").detail("min", PASSWORD_MIN_LENGTH)
        })?
        .to_owned();
    let (hash, owns, username) = credentials(&state, user.id.clone()).await?;
    if owns {
        let current = str_field(&body, "currentPassword")
            .filter(|password| !password.is_empty())
            .ok_or_else(|| ApiError::bad_request("password_required"))?
            .to_owned();
        if !auth::verify_password(&state, current, hash).await {
            return Err(ApiError::forbidden("current_password_incorrect"));
        }
    }
    let new_hash = auth::hash_password(&state, new_password).await?;
    let user_id = user.id.clone();
    state
        .run(move |tx, _| {
            tx.execute(
                "UPDATE users SET password_hash = ?, password_changed_at = ? WHERE id = ?",
                params![new_hash, now_ms(), &user_id],
            )?;
            Ok(())
        })
        .await?;
    let token = auth::sign_token(&state, &user.id, &username)?;
    Ok(Json(json!({"token": token})))
}

/// Обезличивает аккаунт и убирает его следы (`tombstoneUser`). Возвращает
/// файлы, которые надо стереть.
///
/// # Errors
/// Ошибка базы.
pub fn tombstone_user(tx: &mut dyn Tx, user_id: &str) -> DbResult<Vec<String>> {
    let Some(user) = user_row(tx, user_id)? else {
        return Ok(Vec::new());
    };
    crate::twofactor::remove(tx, user_id)?;
    let mut stale = Vec::new();
    for column in ["avatar", "banner"] {
        if let Some(file) = user.opt_text(column)?.filter(|file| !file.is_empty()) {
            stale.push(file);
        }
    }
    let owned_groups: Vec<String> = tx
        .query(
            "SELECT id FROM dm_channels WHERE owner_id = ?",
            params![user_id],
        )?
        .iter()
        .map(|row| row.text("id"))
        .collect::<DbResult<_>>()?;
    let dm_channels: Vec<String> = tx
        .query(
            "SELECT dm_channel_id FROM dm_members WHERE user_id = ?",
            params![user_id],
        )?
        .iter()
        .map(|row| row.text("dm_channel_id"))
        .collect::<DbResult<_>>()?;
    for sql in [
        "DELETE FROM space_members WHERE user_id = ?",
        "DELETE FROM member_roles WHERE user_id = ?",
        "DELETE FROM read_states WHERE user_id = ?",
        "DELETE FROM reactions WHERE user_id = ?",
        "DELETE FROM dm_reactions WHERE user_id = ?",
        "DELETE FROM user_federation_credentials WHERE user_id = ?",
        "DELETE FROM bans WHERE user_id = ?",
        "DELETE FROM join_requests WHERE user_id = ?",
        "DELETE FROM voice_restrictions WHERE user_id = ?",
        "UPDATE bans SET banned_by = NULL WHERE banned_by = ?",
        "UPDATE voice_restrictions SET moderator_id = NULL WHERE moderator_id = ?",
        "UPDATE join_requests SET decided_by = NULL WHERE decided_by = ?",
        "DELETE FROM channel_overrides WHERE target_type = 'member' AND target_id = ?",
        "DELETE FROM category_overrides WHERE target_type = 'member' AND target_id = ?",
        "DELETE FROM dm_members WHERE user_id = ? AND dm_channel_id IN \
         (SELECT id FROM dm_channels WHERE owner_id IS NOT NULL)",
    ] {
        tx.execute(sql, params![user_id])?;
    }
    tx.execute(
        "DELETE FROM friends WHERE user_id = ? OR friend_id = ?",
        params![user_id, user_id],
    )?;
    tx.execute(
        "DELETE FROM friend_requests WHERE from_id = ? OR to_id = ?",
        params![user_id, user_id],
    )?;
    tx.execute(
        "DELETE FROM space_folder_members WHERE folder_id IN (SELECT id FROM space_folders WHERE user_id = ?)",
        params![user_id],
    )?;
    tx.execute(
        "DELETE FROM space_folders WHERE user_id = ?",
        params![user_id],
    )?;
    for group in &owned_groups {
        let next = tx.query_one(
            "SELECT m.user_id FROM dm_members m JOIN users u ON u.id = m.user_id \
             WHERE m.dm_channel_id = ? AND u.is_deleted = 0 LIMIT 1",
            params![group],
        )?;
        if let Some(next) = next {
            tx.execute(
                "UPDATE dm_channels SET owner_id = ? WHERE id = ?",
                params![next.text("user_id")?, group],
            )?;
        }
    }
    for channel in &dm_channels {
        let alive = tx.query_one(
            "SELECT 1 AS one FROM dm_members m JOIN users u ON u.id = m.user_id \
             WHERE m.dm_channel_id = ? AND u.is_deleted = 0 AND m.user_id <> ?",
            params![channel, user_id],
        )?;
        if alive.is_some() {
            continue;
        }
        for row in tx.query(
            "SELECT a.filename FROM attachments a JOIN dm_messages m ON m.id = a.dm_message_id \
             WHERE m.dm_channel_id = ?",
            params![channel],
        )? {
            stale.push(row.text("filename")?);
        }
        tx.execute(
            "DELETE FROM attachments WHERE dm_message_id IN (SELECT id FROM dm_messages WHERE dm_channel_id = ?)",
            params![channel],
        )?;
        tx.execute(
            "DELETE FROM dm_reactions WHERE dm_message_id IN (SELECT id FROM dm_messages WHERE dm_channel_id = ?)",
            params![channel],
        )?;
        tx.execute("DELETE FROM dm_channels WHERE id = ?", params![channel])?;
    }
    for row in tx.query(
        "SELECT a.filename FROM attachments a JOIN messages m ON m.id = a.message_id WHERE m.user_id = ?",
        params![user_id],
    )? {
        stale.push(row.text("filename")?);
    }
    tx.execute(
        "DELETE FROM attachments WHERE message_id IN (SELECT id FROM messages WHERE user_id = ?)",
        params![user_id],
    )?;
    tx.execute(
        "DELETE FROM embeds WHERE message_id IN (SELECT id FROM messages WHERE user_id = ?)",
        params![user_id],
    )?;
    tx.execute("DELETE FROM messages WHERE user_id = ?", params![user_id])?;
    tx.execute(
        "UPDATE users SET username = ?, password_hash = ?, display_name = NULL, avatar = NULL, \
         banner = NULL, bio = NULL, custom_status = NULL, accent_color = NULL, avatar_color = NULL, \
         replicated_instances = '[]', is_deleted = 1, status = 'offline', is_admin = 0, \
         federation_home_orphaned = 0 WHERE id = ?",
        params![
            format!("!deleted:{user_id}"),
            hex::encode(rand::rng().random::<[u8; 32]>()),
            user_id
        ],
    )?;
    Ok(stale)
}

/// `DELETE /api/users/@me`.
pub async fn delete_me(
    State(state): State<AppState>,
    user: AuthUser,
    ClientIp(ip): ClientIp,
    JsonBody(body): JsonBody,
) -> ApiResult<Response> {
    state.limiter.check(DELETE_LIMIT, ip)?;
    let confirm = str_field(&body, "username")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| ApiError::bad_request("username_confirmation_required"))?
        .to_owned();
    let (hash, owns, username) = credentials(&state, user.id.clone()).await?;
    if username != confirm {
        return Err(ApiError::bad_request("username_confirmation_mismatch"));
    }
    if owns {
        let password = str_field(&body, "password")
            .filter(|password| !password.is_empty())
            .ok_or_else(|| ApiError::bad_request("password_required"))?
            .to_owned();
        if !auth::verify_password(&state, password, hash).await {
            return Err(ApiError::forbidden("account_deletion_password_incorrect"));
        }
    }
    let app = state.clone();
    let outcome = state
        .run(move |tx, out| {
            let owned = tx.query(
                "SELECT id, name FROM spaces WHERE owner_id = ?",
                params![&user.id],
            )?;
            if !owned.is_empty() {
                let spaces = owned
                    .iter()
                    .map(|row| Ok(json!({"id": row.text("id")?, "name": row.text("name")?})))
                    .collect::<DbResult<Vec<_>>>()?;
                return Ok(Err(spaces));
            }
            let spaces: Vec<String> = tx
                .query(
                    "SELECT space_id FROM space_members WHERE user_id = ?",
                    params![&user.id],
                )?
                .iter()
                .map(|row| row.text("space_id"))
                .collect::<DbResult<_>>()?;
            let audience = ws::profile_targets(tx, &user.id)?;
            let stale = tombstone_user(tx, &user.id)?;
            for space in &spaces {
                out.to_space(
                    space,
                    &json!({"type": "member_left", "spaceId": space, "userId": user.id}),
                );
            }
            if let Some(row) = user_row(tx, &user.id)? {
                let event = json!({"type": "user_updated", "user": sanitize_user(&row, false)?});
                for target in &audience {
                    out.to_user(target, &event);
                }
            }
            crate::voice::user_removed(tx, &app.hub, out, &user.id)?;
            out.close_user(&user.id);
            Ok(Ok(stale))
        })
        .await?;
    match outcome {
        Err(spaces) => Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": error_text("account_deletion_blocked_owned_spaces", None),
                "code": "account_deletion_blocked_owned_spaces",
                "statusCode": 400,
                "ownedSpaces": spaces,
            })),
        )
            .into_response()),
        Ok(stale) => {
            for file in &stale {
                files::delete_upload_file(&state.config, file).await;
            }
            Ok(Json(json!({"success": true})).into_response())
        }
    }
}

static ACCENT: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::expect_used, reason = "выражение постоянное")]
    Regex::new(r"^#[0-9a-fA-F]{6}$").expect("regex")
});
static DOMAIN: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::expect_used, reason = "выражение постоянное")]
    Regex::new(
        r"^[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?(\.[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?)*$",
    )
    .expect("regex")
});

fn valid_asset(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(url) => {
            let trimmed = js_trim(url);
            trimmed.is_empty()
                || trimmed.starts_with("/api/uploads/")
                || trimmed.starts_with("https://")
                || trimmed.starts_with("http://")
                || (!trimmed.contains('/') && !trimmed.contains('\\') && !trimmed.contains(".."))
        }
        // Не строка: оригинал падает с ошибкой 500, здесь — отказ.
        _ => false,
    }
}

/// Обрезанная строка с пределом длины; пустая и не строка — `NULL`.
fn limited_text(value: &Value, max: usize, code: &'static str) -> ApiResult<crate::db::Value> {
    match value.as_str() {
        Some(text) => {
            let trimmed = js_trim(text);
            if utf16_len(trimmed) > max {
                return Err(ApiError::bad_request(code).detail("max", max));
            }
            Ok(Some(trimmed).filter(|text| !text.is_empty()).into())
        }
        None => Ok(crate::db::Value::Null),
    }
}

fn check_replicated(list: &Value) -> ApiResult<()> {
    let name = "replicatedInstances";
    let entries = list
        .as_array()
        .ok_or_else(|| validation(name, "must be an array"))?;
    if entries.len() > 20 {
        return Err(validation(name, "at most 20 entries"));
    }
    for entry in entries {
        let username = entry.get("username").and_then(Value::as_str);
        let Some(username) = username.filter(|name| !js_trim(name).is_empty()) else {
            return Err(validation(
                "replicatedInstances.username",
                "must be a non-empty string",
            ));
        };
        if utf16_len(username) > 255 {
            return Err(validation(
                "replicatedInstances.username",
                "at most 255 characters",
            ));
        }
        let origin = entry.get("origin").and_then(Value::as_str);
        let domain = entry.get("domain").and_then(Value::as_str);
        if origin.is_none() && domain.is_none() {
            return Err(validation(
                name,
                "each entry needs an origin or domain string",
            ));
        }
        if let Some(origin) = origin {
            if utf16_len(origin) > 512 {
                return Err(validation(
                    "replicatedInstances.origin",
                    "at most 512 characters",
                ));
            }
            if !origin.starts_with("https://") && !origin.starts_with("http://") {
                return Err(validation(
                    "replicatedInstances.origin",
                    "must start with https:// or http://",
                ));
            }
        }
        if let Some(domain) = domain {
            if utf16_len(domain) > 253 {
                return Err(validation(
                    "replicatedInstances.domain",
                    "at most 253 characters",
                ));
            }
            if !DOMAIN.is_match(domain) {
                return Err(validation(
                    "replicatedInstances.domain",
                    "contains invalid characters",
                ));
            }
        }
    }
    Ok(())
}

const DURABLE: [(&str, &str); 6] = [
    ("displayName", "display_name"),
    ("avatar", "avatar"),
    ("banner", "banner"),
    ("accentColor", "accent_color"),
    ("avatarColor", "avatar_color"),
    ("bio", "bio"),
];

/// `PATCH /api/users/@me`.
pub async fn update_me(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let app = state.clone();
    let (reply, stale, resize) = state
        .run(move |tx, out| {
            let before =
                user_row(tx, &user.id)?.ok_or_else(|| ApiError::not_found("user_not_found"))?;
            let replica = before
                .opt_text("home_instance")?
                .is_some_and(|home| !home.is_empty());
            let managed_elsewhere =
                replica && before.opt_flag("federation_home_orphaned")? != Some(true);
            if managed_elsewhere && DURABLE.iter().any(|(name, _)| field(&body, name).is_some()) {
                return Err(ApiError::forbidden("profile_managed_by_home"));
            }
            let mut sets: Vec<(&str, crate::db::Value)> = Vec::new();
            if let Some(value) = field(&body, "displayName") {
                sets.push((
                    "display_name",
                    limited_text(value, 32, "display_name_too_long")?,
                ));
            }
            let old_avatar = before.opt_text("avatar")?.filter(|file| !file.is_empty());
            let old_banner = before.opt_text("banner")?.filter(|file| !file.is_empty());
            let mut new_avatar: Option<Option<String>> = None;
            let mut new_banner: Option<Option<String>> = None;
            if let Some(value) = field(&body, "avatar") {
                if !valid_asset(value) {
                    return Err(ApiError::bad_request("avatar_url_invalid"));
                }
                let stored = value
                    .as_str()
                    .map(|url| url.strip_prefix("/api/uploads/").unwrap_or(url).to_owned());
                sets.push(("avatar", stored.clone().into()));
                new_avatar = Some(stored);
            }
            if let Some(value) = field(&body, "banner") {
                if !valid_asset(value) {
                    return Err(ApiError::bad_request("banner_url_invalid"));
                }
                let stored = value
                    .as_str()
                    .map(js_trim)
                    .filter(|url| !url.is_empty())
                    .map(|url| url.strip_prefix("/api/uploads/").unwrap_or(url).to_owned());
                sets.push(("banner", stored.clone().into()));
                new_banner = Some(stored);
            }
            if let Some(value) = field(&body, "accentColor") {
                match value
                    .as_str()
                    .map(js_trim)
                    .filter(|color| !color.is_empty())
                {
                    Some(color) if ACCENT.is_match(color) => {
                        sets.push(("accent_color", color.into()));
                    }
                    Some(_) => return Err(ApiError::bad_request("accent_color_invalid")),
                    None => sets.push(("accent_color", crate::db::Value::Null)),
                }
            }
            if let Some(value) = field(&body, "avatarColor") {
                match value
                    .as_str()
                    .map(js_trim)
                    .filter(|color| !color.is_empty())
                {
                    Some(color) if AVATAR_COLORS.contains(&color) => {
                        sets.push(("avatar_color", color.into()));
                    }
                    Some(_) => return Err(ApiError::bad_request("avatar_color_invalid")),
                    None => sets.push(("avatar_color", crate::db::Value::Null)),
                }
            }
            if let Some(value) = field(&body, "bio") {
                let stored = if truthy(Some(value)) {
                    limited_text(value, 190, "bio_too_long")?
                } else {
                    crate::db::Value::Null
                };
                sets.push(("bio", stored));
            }
            if let Some(value) = field(&body, "customStatus") {
                sets.push((
                    "custom_status",
                    limited_text(value, 128, "custom_status_too_long")?,
                ));
            }
            let mut chosen = None;
            if let Some(value) = field(&body, "status") {
                let status = value
                    .as_str()
                    .filter(|status| matches!(*status, "online" | "idle" | "dnd"))
                    .ok_or_else(|| ApiError::bad_request("status_invalid"))?;
                chosen = Some(status.to_owned());
            }
            let mut new_origins = Vec::new();
            if let Some(list) = field(&body, "replicatedInstances") {
                check_replicated(list)?;
                sets.push(("replicated_instances", list.to_string().into()));
                if !replica {
                    let known: HashSet<String> = before
                        .opt_text("replicated_instances")?
                        .and_then(|text| serde_json::from_str::<Vec<Value>>(&text).ok())
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|entry| entry["origin"].as_str().map(str::to_owned))
                        .collect();
                    for entry in list.as_array().into_iter().flatten() {
                        if let Some(origin) = entry["origin"]
                            .as_str()
                            .filter(|o| !o.is_empty() && !known.contains(*o))
                        {
                            new_origins.push(origin.to_owned());
                        }
                    }
                }
            }
            if let Some(home_user) = str_field(&body, "homeUserId").filter(|id| !id.is_empty())
                && replica
            {
                sets.push(("home_user_id", home_user.into()));
            }
            if let Some(value) = field(&body, "discoverable") {
                let flag = value
                    .as_bool()
                    .ok_or_else(|| validation("discoverable", "must be a boolean"))?;
                sets.push(("discoverable", flag.into()));
            }
            let mut show_activity = None;
            if let Some(value) = field(&body, "showActivity") {
                let flag = value
                    .as_bool()
                    .ok_or_else(|| validation("showActivity", "must be a boolean"))?;
                sets.push(("show_activity", flag.into()));
                show_activity = Some(flag);
            }
            if sets.is_empty() && chosen.is_none() {
                return Err(ApiError::bad_request("no_fields_to_update"));
            }
            let durable = DURABLE
                .iter()
                .any(|(_, column)| sets.iter().any(|(name, _)| name == column));
            let profile_changed = durable || sets.iter().any(|(name, _)| *name == "custom_status");
            if durable {
                sets.push(("profile_updated_at", now_ms().into()));
            }
            if !sets.is_empty() {
                let assignments: Vec<String> = sets
                    .iter()
                    .map(|(column, _)| format!("{column} = ?"))
                    .collect();
                let mut values: Vec<crate::db::Value> =
                    sets.iter().map(|(_, value)| value.clone()).collect();
                values.push(user.id.as_str().into());
                tx.execute(
                    &format!("UPDATE users SET {} WHERE id = ?", assignments.join(", ")),
                    &values,
                )?;
            }
            if !new_origins.is_empty() && federation::relay_enabled(tx)? {
                let payload = federation::profile_payload(&app.config, &before)?;
                federation::append_mutation_log_in(
                    tx,
                    &app.ids,
                    &user.id,
                    &user.id,
                    "profile",
                    "profile_update",
                    Some(&payload),
                )?;
                federation::queue_outbox(&user.id, "profile_update", &new_origins);
            }
            if let Some(status) = &chosen {
                events::apply_chosen_status(&app, tx, out, &user.id, status)?;
            }
            if let Some(show) = show_activity {
                app.hub.set_show_activity(&user.id, show);
                if !show {
                    app.hub.set_activities(&user.id, Vec::new());
                    let status = app.hub.status(&user.id);
                    let payload = ws::presence_update(
                        tx,
                        &user.id,
                        status.as_deref().unwrap_or("offline"),
                        Some(&[]),
                    )?;
                    let mut payload = payload;
                    if status.is_none() {
                        payload["status"] = Value::Null;
                    }
                    ws::to_profile_audience(tx, out, &user.id, &payload)?;
                    out.to_user(&user.id, &payload);
                }
            }
            let mut stale = Vec::new();
            let mut resize = Vec::new();
            if let Some(new) = &new_avatar {
                if let Some(old) =
                    old_avatar.filter(|old| Some(old) != new.as_ref() && !old.starts_with("http"))
                {
                    files::delete_attachment_by_filename(tx, &old)?;
                    stale.push(old);
                }
                if let Some(new) = new.as_deref().filter(|new| !new.is_empty()) {
                    files::delete_attachment_by_filename(tx, new)?;
                    if !new.starts_with("http") {
                        resize.push((new.to_owned(), files::ProfileImage::Avatar));
                    }
                }
            }
            if let Some(new) = &new_banner {
                if let Some(old) =
                    old_banner.filter(|old| Some(old) != new.as_ref() && !old.starts_with("http"))
                {
                    files::delete_attachment_by_filename(tx, &old)?;
                    stale.push(old);
                }
                if let Some(new) = new.as_deref() {
                    files::delete_attachment_by_filename(tx, new)?;
                    if !new.starts_with("http") {
                        resize.push((new.to_owned(), files::ProfileImage::Banner));
                    }
                }
            }
            let updated =
                user_row(tx, &user.id)?.ok_or_else(|| ApiError::not_found("user_not_found"))?;
            let sanitized = sanitize_user(&updated, true)?;
            if profile_changed {
                let event = json!({"type": "user_updated", "user": sanitized});
                ws::to_profile_audience(tx, out, &user.id, &event)?;
                out.to_user(&user.id, &event);
            }
            if durable && !replica && federation::relay_enabled(tx)? {
                let changed = DURABLE.iter().any(|(_, column)| {
                    sets.iter().any(|(name, _)| name == column)
                        && before.opt_text(column).ok() != updated.opt_text(column).ok()
                });
                if changed {
                    let payload = federation::profile_payload(&app.config, &updated)?;
                    federation::append_mutation_log_in(
                        tx,
                        &app.ids,
                        &user.id,
                        &user.id,
                        "profile",
                        "profile_update",
                        Some(&payload),
                    )?;
                    federation::broadcast_outbox(tx, &user.id, "profile_update")?;
                }
            }
            Ok((sanitized, stale, resize))
        })
        .await?;
    for file in &stale {
        files::delete_upload_file(&state.config, file).await;
    }
    for (file, kind) in resize {
        files::resize_profile_image(&state.config, Some(&file), kind).await;
    }
    Ok(Json(reply))
}

/// `GET /api/users/:id`.
pub async fn get_user(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let row = user_row(tx, &id)?.ok_or_else(|| ApiError::not_found("user_not_found"))?;
            Ok(Json(sanitize_user(&row, false)?))
        })
        .await
}

/// Параметр `homeUserId` запроса общих друзей и пространств.
#[derive(Debug, serde::Deserialize)]
pub struct MutualsQuery {
    #[serde(rename = "homeUserId")]
    home_user_id: Option<String>,
}

/// `GET /api/users/:id/mutuals`.
pub async fn mutuals(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<MutualsQuery>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let mut target = id.clone();
            if user_row(tx, &id)?.is_none()
                && let Some(home) = query.home_user_id.filter(|home| !home.is_empty())
                && let Some(row) = tx.query_one(
                    "SELECT id FROM users WHERE home_user_id = ? OR id = ?",
                    params![&home, &home],
                )?
            {
                target = row.text("id")?;
            }
            let friends = tx.query(
                &format!(
                    "SELECT {USER_COLUMNS} FROM users WHERE id IN ( \
                     SELECT CASE WHEN user_id = ? THEN friend_id ELSE user_id END FROM friends \
                     WHERE user_id = ? OR friend_id = ?) AND id IN ( \
                     SELECT CASE WHEN user_id = ? THEN friend_id ELSE user_id END FROM friends \
                     WHERE user_id = ? OR friend_id = ?)"
                ),
                params![&user.id, &user.id, &user.id, &target, &target, &target],
            )?;
            let spaces = tx.query(
                "SELECT s.id, s.name, s.icon, s.avatar_color FROM spaces s \
                 WHERE s.id IN (SELECT space_id FROM space_members WHERE user_id = ?) \
                 AND s.id IN (SELECT space_id FROM space_members WHERE user_id = ?)",
                params![&user.id, &target],
            )?;
            Ok(Json(json!({
                "mutualFriends": friends.iter().map(|row| sanitize_user(row, false)).collect::<DbResult<Vec<_>>>()?,
                "mutualSpaces": spaces.iter().map(|row| Ok(json!({
                    "id": row.text("id")?,
                    "name": row.text("name")?,
                    "icon": row.opt_text("icon")?,
                    "avatarColor": row.opt_text("avatar_color")?,
                }))).collect::<DbResult<Vec<_>>>()?,
            })))
        })
        .await
}

/// `GET /api/users/@me/federation-registry`.
pub async fn registry(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let rows = tx.query(
                "SELECT origin, label, username, remote_user_id, status, added_at, last_connected_at, \
                 disconnected_at, error_message FROM user_federation_registry WHERE user_id = ?",
                params![&user.id],
            )?;
            let updated = tx
                .query_one(
                    "SELECT federation_registry_updated_at FROM users WHERE id = ?",
                    params![&user.id],
                )?
                .map(|row| row.opt_int("federation_registry_updated_at"))
                .transpose()?
                .flatten()
                .unwrap_or(0);
            let registry = rows
                .iter()
                .map(|row| {
                    Ok(json!({
                        "origin": row.text("origin")?,
                        "label": row.opt_text("label")?,
                        "username": row.opt_text("username")?,
                        "remoteUserId": row.opt_text("remote_user_id")?,
                        "status": row.text("status")?,
                        "addedAt": row.int("added_at")?,
                        "lastConnectedAt": row.opt_int("last_connected_at")?,
                        "disconnectedAt": row.opt_int("disconnected_at")?,
                        "errorMessage": row.opt_text("error_message")?,
                    }))
                })
                .collect::<DbResult<Vec<_>>>()?;
            Ok(Json(json!({"registry": registry, "updatedAt": updated})))
        })
        .await
}

/// `PUT /api/users/@me/federation-registry`.
pub async fn put_registry(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    const STATUSES: [&str; 4] = ["connected", "disconnected", "unreachable", "auth_expired"];
    let entries = field(&body, "registry")
        .and_then(Value::as_array)
        .ok_or_else(|| validation("registry", "must be an array"))?
        .clone();
    let updated_at = field(&body, "updatedAt")
        .and_then(Value::as_f64)
        .filter(|at| *at > 0.0)
        .ok_or_else(|| validation("updatedAt", "must be a positive number"))?;
    if entries.len() > 100 {
        return Err(validation("registry", "at most 100 entries"));
    }
    let mut origins = HashSet::new();
    for entry in &entries {
        if !origins.insert(entry.get("origin").map(Value::to_string)) {
            return Err(validation("registry", "duplicate origins"));
        }
    }
    for entry in &entries {
        if entry
            .get("origin")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(validation("registry.origin", "must be a string"));
        }
        if !entry
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|s| STATUSES.contains(&s))
        {
            return Err(validation(
                "registry.status",
                "must be one of: connected, disconnected, unreachable, auth_expired",
            ));
        }
        if !entry.get("addedAt").is_some_and(Value::is_number) {
            return Err(validation("registry.addedAt", "must be a number"));
        }
    }
    state
        .run(move |tx, _| {
            let stored = tx
                .query_one(
                    "SELECT federation_registry_updated_at FROM users WHERE id = ?",
                    params![&user.id],
                )?
                .map(|row| row.opt_int("federation_registry_updated_at"))
                .transpose()?
                .flatten()
                .unwrap_or(0);
            #[allow(clippy::cast_precision_loss, reason = "отметки времени далеки от 2^53")]
            if updated_at <= stored as f64 {
                return Err(ApiError::conflict("registry_conflict"));
            }
            tx.execute("DELETE FROM user_federation_registry WHERE user_id = ?", params![&user.id])?;
            for entry in &entries {
                let text = |name: &str| entry.get(name).and_then(Value::as_str).unwrap_or_default().to_owned();
                tx.execute(
                    "INSERT INTO user_federation_registry (user_id, origin, label, username, \
                     remote_user_id, status, added_at, last_connected_at, disconnected_at, error_message) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        &user.id,
                        text("origin"),
                        text("label"),
                        text("username"),
                        text("remoteUserId"),
                        text("status"),
                        entry["addedAt"].as_f64().unwrap_or(0.0),
                        entry.get("lastConnectedAt").and_then(Value::as_f64),
                        entry.get("disconnectedAt").and_then(Value::as_f64),
                        entry.get("errorMessage").and_then(Value::as_str)
                    ],
                )?;
            }
            tx.execute(
                "UPDATE users SET federation_registry_updated_at = ? WHERE id = ?",
                params![updated_at, &user.id],
            )?;
            Ok(Json(json!({"ok": true, "updatedAt": body["updatedAt"]})))
        })
        .await
}

fn layout_snapshot(
    tx: &mut dyn Tx,
    user_id: &str,
    fallback_ts: i64,
) -> DbResult<(Value, Value, i64)> {
    let layout = tx.query_one(
        "SELECT layout, updated_at FROM user_space_layout WHERE user_id = ?",
        params![user_id],
    )?;
    let (items, updated_at) = match layout {
        Some(row) => (
            serde_json::from_str::<Value>(&row.text("layout")?).unwrap_or_else(|_| json!([])),
            row.opt_int("updated_at")?.unwrap_or(fallback_ts),
        ),
        None => (json!([]), fallback_ts),
    };
    Ok((items, json!(ready::folders(tx, user_id)?), updated_at))
}

/// `PUT /api/users/@me/space-layout`.
pub async fn put_space_layout(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let items = field(&body, "items")
        .and_then(Value::as_array)
        .ok_or_else(|| validation("items", "must be an array"))?
        .clone();
    let folders = field(&body, "folders")
        .filter(|folders| folders.is_object() || folders.is_array())
        .ok_or_else(|| validation("folders", "must be an object"))?;
    let folders: Vec<(String, Value)> = match folders {
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        _ => Vec::new(),
    };
    for item in &items {
        let kind = item.get("t").and_then(Value::as_str);
        if !matches!(kind, Some("s" | "f")) || item.get("id").and_then(Value::as_str).is_none() {
            return Err(validation(
                "items",
                "each item needs t (\"s\" or \"f\") and an id string",
            ));
        }
    }
    for (key, folder) in &folders {
        if !folder.get("spaceIds").is_some_and(Value::is_array) {
            return Err(validation(
                &format!("folders.{key}.spaceIds"),
                "must be an array",
            ));
        }
    }
    let incoming = field(&body, "updatedAt").and_then(Value::as_f64);
    let ids = state.ids.clone();
    state
        .run(move |tx, out: &mut Outbox| {
            let existing = tx.query_one(
                "SELECT updated_at FROM user_space_layout WHERE user_id = ?",
                params![&user.id],
            )?;
            #[allow(clippy::cast_precision_loss, reason = "отметки времени далеки от 2^53")]
            if let (Some(incoming), Some(row)) = (incoming, &existing)
                && incoming < row.opt_int("updated_at")?.unwrap_or(0) as f64
            {
                let (items, folders, updated_at) = layout_snapshot(tx, &user.id, 0)?;
                return Ok(Json(json!({"items": items, "folders": folders, "updatedAt": updated_at})));
            }
            #[allow(clippy::cast_possible_truncation, reason = "отметка времени клиента")]
            let effective = incoming.map_or_else(now_ms, |at| at as i64);
            let known: HashSet<String> = tx
                .query("SELECT id FROM space_folders WHERE user_id = ?", params![&user.id])?
                .iter()
                .map(|row| row.text("id"))
                .collect::<DbResult<_>>()?;
            let mut keep = HashSet::new();
            let mut minted = std::collections::HashMap::new();
            for (key, folder) in &folders {
                let name = folder.get("name").and_then(Value::as_str);
                let color = folder.get("color").and_then(Value::as_str);
                let id = if key.starts_with("new:") {
                    let id = ids.next();
                    minted.insert(key.clone(), id.clone());
                    id
                } else {
                    key.clone()
                };
                keep.insert(id.clone());
                if known.contains(&id) {
                    tx.execute(
                        "UPDATE space_folders SET name = ?, color = ? WHERE id = ? AND user_id = ?",
                        params![name, color, &id, &user.id],
                    )?;
                } else {
                    // Идентификатор задаёт клиент; чужую папку им не занять.
                    let inserted = tx.execute(
                        "INSERT INTO space_folders (id, user_id, name, color, position, created_at) \
                         VALUES (?, ?, ?, ?, 0, ?) ON CONFLICT DO NOTHING",
                        params![&id, &user.id, name, color, now_ms()],
                    )?;
                    if inserted == 0 {
                        return Err(validation("folders", "folder id is not available"));
                    }
                }
                tx.execute("DELETE FROM space_folder_members WHERE folder_id = ?", params![&id])?;
                let spaces = folder["spaceIds"].as_array().into_iter().flatten();
                for (position, space) in spaces.enumerate() {
                    let Some(space) = space.as_str().filter(|space| !space.is_empty()) else { continue };
                    tx.execute(
                        "INSERT INTO space_folder_members (folder_id, space_id, position) VALUES (?, ?, ?)",
                        params![&id, space, i64::try_from(position).unwrap_or(0)],
                    )?;
                }
            }
            for id in known.difference(&keep) {
                tx.execute("DELETE FROM space_folder_members WHERE folder_id = ?", params![id])?;
                tx.execute(
                    "DELETE FROM space_folders WHERE id = ? AND user_id = ?",
                    params![id, &user.id],
                )?;
            }
            let final_items: Vec<Value> = items
                .iter()
                .map(|item| match item["id"].as_str().and_then(|id| minted.get(id)) {
                    Some(id) if item["t"] == "f" => json!({"t": "f", "id": id}),
                    _ => item.clone(),
                })
                .collect();
            let layout = Value::Array(final_items).to_string();
            if existing.is_some() {
                tx.execute(
                    "UPDATE user_space_layout SET layout = ?, updated_at = ? WHERE user_id = ?",
                    params![layout, effective, &user.id],
                )?;
            } else {
                tx.execute(
                    "INSERT INTO user_space_layout (user_id, layout, updated_at) VALUES (?, ?, ?)",
                    params![&user.id, layout, effective],
                )?;
            }
            let (items, folders, updated_at) = layout_snapshot(tx, &user.id, effective)?;
            out.to_user(
                &user.id,
                &json!({
                    "type": "space_layout_updated",
                    "layout": items,
                    "folders": folders,
                    "updatedAt": updated_at,
                }),
            );
            Ok(Json(json!({"items": items, "folders": folders, "updatedAt": updated_at})))
        })
        .await
}
