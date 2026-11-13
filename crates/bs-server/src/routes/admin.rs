//! Управление инстансом: хранилище и пользователи (`routes/admin.ts`).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};

use crate::auth::{self, AuthUser};
use crate::db::{DbResult, Row};
use crate::error::{ApiError, error_text};
use crate::routes::account::tombstone_user;
use crate::routes::invites::as_admin;
use crate::routes::search::parse_date;
use crate::routes::users::{USER_COLUMNS, sanitize_user, user_row};
use crate::state::{AppState, now_ms};
use crate::util::{field, js_trim};
use crate::{files, params, storage, voice, ws};

type Outcome = Result<Response, ApiError>;

fn reply(value: Value) -> Response {
    Json(value).into_response()
}

/// `Number(value)` для поля тела запроса.
#[must_use]
pub fn js_number(value: &Value) -> f64 {
    match value {
        Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        Value::String(text) if js_trim(text).is_empty() => 0.0,
        Value::String(text) => js_trim(text).parse().unwrap_or(f64::NAN),
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        Value::Null => 0.0,
        _ => f64::NAN,
    }
}

fn parse_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

fn dry_run(body: &Value) -> bool {
    crate::util::truthy(field(body, "dryRun"))
}

/// `GET /api/admin/storage/stats`.
pub async fn storage_stats(State(state): State<AppState>, user: AuthUser) -> Outcome {
    let config = state.config.clone();
    let done = as_admin(&state, &user, move |tx, _| {
        Ok::<_, ApiError>(storage::stats(tx, &config)?)
    })
    .await?;
    Ok(done.map_or_else(|refusal| refusal, reply))
}

/// `GET /api/admin/storage/orphans`.
pub async fn storage_orphans(State(state): State<AppState>, user: AuthUser) -> Outcome {
    let config = state.config.clone();
    let done = as_admin(&state, &user, move |tx, _| {
        Ok::<_, ApiError>(storage::orphans(tx, &config)?)
    })
    .await?;
    Ok(done.map_or_else(
        |refusal| refusal,
        |orphans| reply(json!({"orphans": orphans})),
    ))
}

/// `POST /api/admin/storage/cleanup`.
pub async fn storage_cleanup(
    State(state): State<AppState>,
    user: AuthUser,
    body: axum::body::Bytes,
) -> Outcome {
    let dry = dry_run(&parse_body(&body));
    let config = state.config.clone();
    let done = as_admin(&state, &user, move |tx, _| {
        Ok::<_, ApiError>(storage::cleanup(tx, &config, dry)?)
    })
    .await?;
    Ok(done.map_or_else(|refusal| refusal, |result| reply(result.to_json(dry))))
}

/// `POST /api/admin/storage/cleanup-media`.
pub async fn storage_cleanup_media(
    State(state): State<AppState>,
    user: AuthUser,
    body: axum::body::Bytes,
) -> Outcome {
    let body = parse_body(&body);
    let dry = dry_run(&body);
    let days = field(&body, "maxAgeDays").map_or(f64::NAN, js_number);
    let config = state.config.clone();
    let done = as_admin(&state, &user, move |tx, _| {
        if days.is_nan() || days < 1.0 {
            return Err(ApiError::bad_request("cleanup_age_invalid").detail("field", "maxAgeDays"));
        }
        Ok(storage::cleanup_old_media(tx, &config, days.min(1e9), dry)?)
    })
    .await?;
    Ok(done.map_or_else(|refusal| refusal, |result| reply(result.to_json(dry))))
}

/// `POST /api/admin/storage/cleanup-tus`.
pub async fn storage_cleanup_uploads(
    State(state): State<AppState>,
    user: AuthUser,
    body: axum::body::Bytes,
) -> Outcome {
    let body = parse_body(&body);
    let dry = dry_run(&body);
    let hours = field(&body, "maxAgeHours").map_or(1.0, js_number);
    let config = state.config.clone();
    let done = as_admin(&state, &user, move |_, _| {
        if !hours.is_finite() || hours <= 0.0 {
            return Err(ApiError::bad_request("cleanup_age_invalid").detail("field", "maxAgeHours"));
        }
        Ok(storage::cleanup_stale_uploads(
            &config,
            hours * 3_600_000.0,
            dry,
        ))
    })
    .await?;
    Ok(done.map_or_else(|refusal| refusal, |result| reply(result.to_json(dry))))
}

/// Пользователь в списке администратора (`toAdminUser`).
fn admin_user(row: &Row) -> DbResult<Value> {
    Ok(json!({
        "id": row.text("id")?,
        "username": row.text("username")?,
        "displayName": row.opt_text("display_name")?,
        "avatar": row.opt_text("avatar")?,
        "avatarColor": row.opt_text("avatar_color")?,
        "status": row.opt_text("status")?.unwrap_or_else(|| "offline".to_owned()),
        "isAdmin": row.opt_flag("is_admin")? == Some(true),
        "isDeleted": row.opt_flag("is_deleted")? == Some(true),
        "homeInstance": row.opt_text("home_instance")?,
        "createdAt": row.int("created_at")?,
    }))
}

/// Параметры списка пользователей.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsersQuery {
    q: Option<String>,
    page: Option<String>,
    page_size: Option<String>,
    show_deleted: Option<String>,
    home_instance: Option<String>,
    role: Option<String>,
    joined_after: Option<String>,
    joined_before: Option<String>,
    sort: Option<String>,
}

fn int_or(text: Option<&str>, fallback: i64) -> i64 {
    let digits: String = text
        .unwrap_or_default()
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits
        .parse::<i64>()
        .ok()
        .filter(|value| *value != 0)
        .unwrap_or(fallback)
}

/// `GET /api/admin/users`.
pub async fn users(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<UsersQuery>,
) -> Outcome {
    let done = as_admin(&state, &user, move |tx, _| {
        let page = int_or(query.page.as_deref(), 1).max(1);
        let page_size = int_or(query.page_size.as_deref(), 50).clamp(1, 100);
        let mut filter = vec!["1 = 1".to_owned()];
        let mut values: Vec<crate::db::Value> = Vec::new();
        if query.show_deleted.as_deref() != Some("true") {
            filter.push("is_deleted = 0".to_owned());
        }
        let q = query.q.as_deref().map(js_trim).unwrap_or_default();
        if !q.is_empty() {
            filter.push("(username LIKE ? OR display_name LIKE ?)".to_owned());
            values.push(format!("%{q}%").into());
            values.push(format!("%{q}%").into());
        }
        match query
            .home_instance
            .as_deref()
            .filter(|home| !home.is_empty())
        {
            Some("local") => filter.push("home_instance IS NULL".to_owned()),
            Some(home) => {
                filter.push("home_instance = ?".to_owned());
                values.push(home.into());
            }
            None => {}
        }
        match query.role.as_deref() {
            Some("admin") => filter.push("is_admin = 1".to_owned()),
            Some("non-admin") => filter.push("is_admin = 0".to_owned()),
            _ => {}
        }
        if let Some(at) = query
            .joined_after
            .as_deref()
            .filter(|text| !text.is_empty())
            .and_then(parse_date)
        {
            filter.push("created_at >= ?".to_owned());
            values.push(at.into());
        }
        if let Some(at) = query
            .joined_before
            .as_deref()
            .filter(|text| !text.is_empty())
            .and_then(|day| parse_date(&format!("{day}T23:59:59.999Z")))
        {
            filter.push("created_at <= ?".to_owned());
            values.push(at.into());
        }
        let order = match query.sort.as_deref() {
            Some("oldest") => "created_at ASC",
            Some("az") => "username ASC",
            Some("za") => "username DESC",
            _ => "created_at DESC",
        };
        let filter = filter.join(" AND ");
        let total = tx
            .query_one(
                &format!("SELECT count(*) AS n FROM users WHERE {filter}"),
                &values,
            )?
            .map_or(Ok(0), |row| row.int("n"))?;
        values.push(page_size.into());
        values.push(((page - 1) * page_size).into());
        let rows = tx.query(
            &format!(
                "SELECT {USER_COLUMNS} FROM users WHERE {filter} ORDER BY {order} LIMIT ? OFFSET ?"
            ),
            &values,
        )?;
        let users = rows.iter().map(admin_user).collect::<DbResult<Vec<_>>>()?;
        Ok::<_, ApiError>(
            json!({"users": users, "total": total, "page": page, "pageSize": page_size}),
        )
    })
    .await?;
    Ok(done.map_or_else(|refusal| refusal, reply))
}

/// `GET /api/admin/users/instances`.
pub async fn user_instances(State(state): State<AppState>, user: AuthUser) -> Outcome {
    let done = as_admin(&state, &user, move |tx, _| {
        let rows = tx.query(
            "SELECT DISTINCT home_instance FROM users WHERE home_instance IS NOT NULL",
            params![],
        )?;
        let mut instances = Vec::new();
        for row in rows {
            if let Some(home) = row
                .opt_text("home_instance")?
                .filter(|home| !home.is_empty())
            {
                instances.push(home);
            }
        }
        Ok::<_, ApiError>(json!({"instances": instances}))
    })
    .await?;
    Ok(done.map_or_else(|refusal| refusal, reply))
}

/// `PATCH /api/admin/users/:id/role`.
pub async fn set_role(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Outcome {
    let body = parse_body(&body);
    let hub = state.hub.clone();
    let done = as_admin(&state, &user, move |tx, out| {
        let Some(make_admin) = field(&body, "isAdmin").and_then(Value::as_bool) else {
            return Err(ApiError::bad_request("field_not_boolean").detail("field", "isAdmin"));
        };
        let target = user_row(tx, &id)?.ok_or_else(|| ApiError::not_found("user_not_found"))?;
        if target.opt_flag("is_deleted")? == Some(true) {
            return Err(ApiError::bad_request("deleted_user_role_change"));
        }
        if make_admin
            && target
                .opt_text("home_instance")?
                .is_some_and(|home| !home.is_empty())
        {
            return Err(ApiError::forbidden("federated_user_cannot_be_admin"));
        }
        let was_admin = target.opt_flag("is_admin")? == Some(true);
        if !make_admin && was_admin {
            let admins = tx
                .query_one(
                    "SELECT count(*) AS n FROM users WHERE is_admin = 1 AND is_deleted = 0",
                    params![],
                )?
                .map_or(Ok(0), |row| row.int("n"))?;
            if admins <= 1 {
                return Err(ApiError::bad_request("last_admin_cannot_be_demoted"));
            }
        }
        tx.execute(
            "UPDATE users SET is_admin = ? WHERE id = ?",
            params![i64::from(make_admin), &id],
        )?;
        let updated = user_row(tx, &id)?.ok_or_else(ApiError::internal)?;
        out.to_user(
            &id,
            &json!({"type": "user_updated", "user": sanitize_user(&updated, false)?}),
        );
        if was_admin != make_admin {
            // Администратор инстанса видит и может больше: его клиент
            // перечитывает пространства, а голосовые права перепроверяются.
            let spaces = tx.query(
                "SELECT space_id FROM space_members WHERE user_id = ?",
                params![&id],
            )?;
            for row in spaces {
                let space = row.text("space_id")?;
                if hub.is_online(&id) {
                    out.to_user(
                        &id,
                        &json!({"type": "space_access_changed", "spaceId": space}),
                    );
                    voice::push_space_state(tx, &hub, out, &id, &space)?;
                }
                voice::check_permissions(tx, &hub, out, &space)?;
            }
        }
        Ok(admin_user(&updated)?)
    })
    .await?;
    Ok(done.map_or_else(|refusal| refusal, reply))
}

/// `POST /api/admin/users/:id/reset-password`: временный пароль.
pub async fn reset_password(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Outcome {
    let target = id.clone();
    let checked = as_admin(&state, &user, move |tx, _| {
        let row = tx
            .query_one(
                "SELECT is_deleted, home_instance FROM users WHERE id = ?",
                params![&target],
            )?
            .ok_or_else(|| ApiError::not_found("user_not_found"))?;
        if row.opt_flag("is_deleted")? == Some(true) {
            return Err(ApiError::bad_request("deleted_user_password_reset"));
        }
        if row
            .opt_text("home_instance")?
            .is_some_and(|home| !home.is_empty())
        {
            return Err(ApiError::bad_request(
                "federated_user_password_managed_by_home",
            ));
        }
        Ok(())
    })
    .await?;
    if let Err(refusal) = checked {
        return Ok(refusal);
    }
    let temporary = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 12]>());
    let hash = auth::hash_password(&state, temporary.clone()).await?;
    state
        .run(move |tx, out| {
            tx.execute(
                "UPDATE users SET password_hash = ?, password_changed_at = ? WHERE id = ?",
                params![&hash, now_ms(), &id],
            )?;
            // Сброс пароля администратором — путь восстановления доступа:
            // вместе с паролем снимается и второй фактор.
            crate::twofactor::remove(tx, &id)?;
            out.close_user(&id);
            Ok(())
        })
        .await?;
    Ok(reply(json!({"temporaryPassword": temporary})))
}

/// `DELETE /api/admin/users/:id`.
pub async fn delete_user(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Outcome {
    let admin = user.id.clone();
    let hub = state.hub.clone();
    let done = as_admin(&state, &user, move |tx, out| {
        if id == admin {
            return Err(ApiError::bad_request("admin_cannot_delete_self"));
        }
        let target = user_row(tx, &id)?.ok_or_else(|| ApiError::not_found("user_not_found"))?;
        if target.opt_flag("is_deleted")? == Some(true) {
            return Err(ApiError::bad_request("user_already_deleted"));
        }
        let owned = tx.query(
            "SELECT id, name FROM spaces WHERE owner_id = ?",
            params![&id],
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
                params![&id],
            )?
            .iter()
            .map(|row| row.text("space_id"))
            .collect::<DbResult<_>>()?;
        let audience = ws::profile_targets(tx, &id)?;
        let stale = tombstone_user(tx, &id)?;
        for space in &spaces {
            out.to_space(
                space,
                &json!({"type": "member_left", "spaceId": space, "userId": id}),
            );
        }
        if let Some(row) = user_row(tx, &id)? {
            let event = json!({"type": "user_updated", "user": sanitize_user(&row, false)?});
            for target in &audience {
                out.to_user(target, &event);
            }
        }
        voice::user_removed(tx, &hub, out, &id)?;
        out.close_user(&id);
        Ok(Ok(stale))
    })
    .await?;
    match done {
        Err(refusal) => Ok(refusal),
        Ok(Err(spaces)) => Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": error_text("user_owns_spaces", None),
                "code": "user_owns_spaces",
                "statusCode": 400,
                "ownedSpaces": spaces,
            })),
        )
            .into_response()),
        Ok(Ok(stale)) => {
            for file in &stale {
                files::delete_upload_file(&state.config, file).await;
            }
            Ok(reply(json!({"success": true})))
        }
    }
}
