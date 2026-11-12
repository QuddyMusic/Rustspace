//! Каталог открытых пространств и заявки на вступление (`routes/explore.ts`).

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use bs_permissions::Permissions;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Row, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ClientIp, JsonBody};
use crate::ratelimit::Limit;
use crate::ready::{self, CHANNEL_COLUMNS};
use crate::routes::spaces::{admit, space_json, space_row};
use crate::routes::users::{sanitize_user, user_columns};
use crate::state::{AppState, now_ms};
use crate::util::{js_trim, str_field, utf16_prefix};
use crate::{params, perms};

const REQUEST_LIMIT: Limit = Limit {
    name: "spaces.request-join",
    max: 5,
    window: Duration::from_mins(1),
};
const REQUEST_COLUMNS: &str =
    "id, space_id, user_id, message, status, decided_by, created_at, decided_at";

/// `parseInt(text, 10) || fallback`.
fn int_or(text: Option<&str>, fallback: i64) -> i64 {
    let text = text.map(str::trim_start).unwrap_or_default();
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
    match digits.parse::<i64>() {
        Ok(0) | Err(_) => fallback,
        Ok(value) => sign * value,
    }
}

fn user_view(tx: &mut dyn Tx, user_id: &str) -> DbResult<Option<Value>> {
    tx.query_one(
        &format!("SELECT {} FROM users u WHERE u.id = ?", user_columns("u")),
        params![user_id],
    )?
    .map(|row| sanitize_user(&row, false))
    .transpose()
}

/// Заявка на проводе (`rowToJoinRequest`).
fn request_json(row: &Row, user: Option<Value>) -> DbResult<Value> {
    let mut request = json!({
        "id": row.text("id")?,
        "spaceId": row.text("space_id")?,
        "userId": row.text("user_id")?,
        "message": row.opt_text("message")?,
        "status": row.text("status")?,
        "decidedBy": row.opt_text("decided_by")?,
        "createdAt": row.int("created_at")?,
        "decidedAt": row.opt_int("decided_at")?,
    });
    if let Some(user) = user {
        request["user"] = user;
    }
    Ok(request)
}

fn request_row(tx: &mut dyn Tx, id: &str) -> DbResult<Option<Row>> {
    tx.query_one(
        &format!("SELECT {REQUEST_COLUMNS} FROM join_requests WHERE id = ?"),
        params![id],
    )
}

/// Кто решает по заявкам: владелец и участники с правом управлять пространством.
fn managers(tx: &mut dyn Tx, space_id: &str, owner_id: &str) -> DbResult<Vec<String>> {
    let rows = tx.query(
        "SELECT user_id FROM space_members WHERE space_id = ?",
        params![space_id],
    )?;
    let mut out = Vec::new();
    for row in rows {
        let user_id = row.text("user_id")?;
        if user_id == owner_id
            || perms::has_permission(tx, &user_id, space_id, Permissions::MANAGE_SPACE)?
        {
            out.push(user_id);
        }
    }
    Ok(out)
}

/// Пространство целиком для вступившего (`buildFullSpace`).
fn full_space(tx: &mut dyn Tx, space: &Row, viewer: &str) -> DbResult<Value> {
    let space_id = space.text("id")?;
    let roles = ready::space_roles(tx, &space_id)?;
    let members = ready::space_members(tx, &space_id, &roles)?;
    let space_perms = perms::compute(tx, viewer, &space_id, None)?;
    let mut categories = Vec::new();
    for row in tx.query(
        "SELECT id, space_id, name, position, created_at FROM channel_categories WHERE space_id = ? ORDER BY position",
        params![&space_id],
    )? {
        categories.push(ready::category_base(&row)?);
    }
    let mut channels = Vec::new();
    for row in tx.query(
        &format!("SELECT {CHANNEL_COLUMNS} FROM channels WHERE space_id = ?"),
        params![&space_id],
    )? {
        let channel_perms = perms::compute(tx, viewer, &space_id, Some(&row.text("id")?))?;
        if perms::can_view(channel_perms) {
            let mut channel = ready::channel_base(&row)?;
            channel["myPermissions"] = json!(perms::to_wire(channel_perms));
            channels.push(channel);
        }
    }
    let with_bits = ready::reads_permission_data(space_perms);
    let mut view = space_json(space)?;
    view["channels"] = json!(channels);
    view["categories"] = json!(categories);
    view["members"] = json!(members);
    view["roles"] = json!(
        roles
            .iter()
            .map(|row| ready::role_view(row, with_bits))
            .collect::<DbResult<Vec<_>>>()?
    );
    view["myPermissions"] = json!(perms::to_wire(space_perms));
    Ok(view)
}

fn require_space(tx: &mut dyn Tx, id: &str) -> ApiResult<Row> {
    space_row(tx, id)?.ok_or_else(|| ApiError::not_found("space_not_found"))
}

fn require_manager(tx: &mut dyn Tx, space_id: &str, user_id: &str) -> ApiResult<()> {
    if perms::is_owner(tx, space_id, user_id)?
        || perms::has_permission(tx, user_id, space_id, Permissions::MANAGE_SPACE)?
    {
        Ok(())
    } else {
        Err(perms::missing("MANAGE_SPACE"))
    }
}

/// Параметры каталога.
#[derive(Debug, Default, serde::Deserialize)]
pub struct ExploreQuery {
    q: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/spaces/explore`.
pub async fn explore(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<ExploreQuery>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let enabled = tx
                .query_one("SELECT discovery_enabled FROM instance_settings WHERE id = 1", params![])?
                .is_some_and(|row| row.opt_flag("discovery_enabled").ok().flatten() != Some(false));
            if !enabled {
                return Ok(Json(json!({"spaces": [], "total": 0, "discoveryEnabled": false})));
            }
            let q = query.q.as_deref().map(js_trim).unwrap_or_default();
            let limit = int_or(query.limit.as_deref(), 50).clamp(1, 100);
            let offset = int_or(query.offset.as_deref(), 0).max(0);
            let listed = "s.visibility IN ('public', 'request')";
            let total_all = tx
                .query_one(&format!("SELECT count(*) AS n FROM spaces s WHERE {listed}"), params![])?
                .map_or(Ok(0), |row| row.int("n"))?;
            // Поиск без учёта регистра, как `LIKE ... COLLATE NOCASE`.
            let pattern = format!("%{}%", q.to_lowercase());
            let matching = if q.is_empty() {
                String::new()
            } else {
                " AND (lower(s.name) LIKE ? OR lower(s.description) LIKE ?)".to_owned()
            };
            let mut values: Vec<crate::db::Value> = Vec::new();
            if !q.is_empty() {
                values.push(pattern.clone().into());
                values.push(pattern.into());
            }
            let total = tx
                .query_one(&format!("SELECT count(*) AS n FROM spaces s WHERE {listed}{matching}"), &values)?
                .map_or(Ok(0), |row| row.int("n"))?;
            let mut listing: Vec<crate::db::Value> = vec![user.id.clone().into()];
            listing.extend(values);
            listing.push(limit.into());
            listing.push(offset.into());
            let rows = tx.query(
                &format!(
                    "SELECT s.id, s.name, s.icon, s.banner, s.avatar_color, s.description, s.visibility, s.created_at, \
                     (SELECT count(*) FROM space_members sm WHERE sm.space_id = s.id) AS member_count, \
                     (SELECT count(*) FROM space_members mine WHERE mine.space_id = s.id AND mine.user_id = ?) AS joined \
                     FROM spaces s WHERE {listed}{matching} \
                     ORDER BY member_count DESC, s.created_at DESC LIMIT ? OFFSET ?"
                ),
                &listing,
            )?;
            let mut spaces = Vec::with_capacity(rows.len());
            for row in rows {
                spaces.push(json!({
                    "id": row.text("id")?,
                    "name": row.text("name")?,
                    "icon": row.opt_text("icon")?,
                    "banner": row.opt_text("banner")?,
                    "avatarColor": row.opt_text("avatar_color")?,
                    "description": row.opt_text("description")?,
                    "visibility": row.text("visibility")?,
                    "memberCount": row.int("member_count")?,
                    "createdAt": row.int("created_at")?,
                    "joined": row.int("joined")? > 0,
                }));
            }
            Ok(Json(json!({"spaces": spaces, "total": total, "totalAll": total_all, "discoveryEnabled": true})))
        })
        .await
}

fn join_refusal(tx: &mut dyn Tx, space_id: &str, user_id: &str) -> ApiResult<()> {
    if perms::is_banned(tx, space_id, user_id)? {
        return Err(ApiError::forbidden("user_banned"));
    }
    if perms::is_member(tx, space_id, user_id)? {
        return Err(ApiError::conflict("already_member"));
    }
    Ok(())
}

/// `POST /api/spaces/:id/public-join`.
pub async fn public_join(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, out| {
            let space = require_space(tx, &id)?;
            if space.opt_text("visibility")?.as_deref() != Some("public") {
                return Err(ApiError::forbidden("space_not_public"));
            }
            join_refusal(tx, &id, &user.id)?;
            admit(tx, out, &id, &user.id)?;
            Ok(Json(full_space(tx, &space, &user.id)?))
        })
        .await
}

/// `POST /api/spaces/:id/request-join`.
pub async fn request_join(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    user: AuthUser,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult<(StatusCode, Json<Value>)> {
    state.limiter.check(REQUEST_LIMIT, ip)?;
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let ids = state.ids.clone();
    state
        .run(move |tx, out| {
            let space = require_space(tx, &id)?;
            if space.opt_text("visibility")?.as_deref() != Some("request") {
                return Err(ApiError::forbidden("space_not_requestable"));
            }
            join_refusal(tx, &id, &user.id)?;
            let pending = tx.query_one(
                "SELECT id FROM join_requests WHERE space_id = ? AND user_id = ? AND status = 'pending'",
                params![&id, &user.id],
            )?;
            if pending.is_some() {
                return Err(ApiError::conflict("join_request_pending"));
            }
            let request_id = ids.next();
            let message = str_field(&body, "message").map(|text| utf16_prefix(js_trim(text), 500).to_owned());
            tx.execute(
                "INSERT INTO join_requests (id, space_id, user_id, message, status, created_at) VALUES (?, ?, ?, ?, 'pending', ?)",
                params![&request_id, &id, &user.id, &message, now_ms()],
            )?;
            let row = request_row(tx, &request_id)?.ok_or_else(ApiError::internal)?;
            let request = request_json(&row, user_view(tx, &user.id)?)?;
            let event = json!({"type": "join_request_received", "request": request});
            for manager in managers(tx, &id, &space.text("owner_id")?)? {
                out.to_user(&manager, &event);
            }
            Ok((StatusCode::CREATED, Json(request)))
        })
        .await
}

/// Отбор заявок по состоянию.
#[derive(Debug, Default, serde::Deserialize)]
pub struct StatusQuery {
    status: Option<String>,
}

/// `GET /api/spaces/:id/join-requests`.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<StatusQuery>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            require_space(tx, &id)?;
            require_manager(tx, &id, &user.id)?;
            let status = query.status.unwrap_or_else(|| "pending".to_owned());
            if !matches!(status.as_str(), "pending" | "accepted" | "declined") {
                return Err(ApiError::bad_request("validation_failed"));
            }
            let rows = tx.query(
                &format!(
                    "SELECT {REQUEST_COLUMNS} FROM join_requests WHERE space_id = ? AND status = ?"
                ),
                params![&id, &status],
            )?;
            let mut requests = Vec::with_capacity(rows.len());
            for row in rows {
                let author = user_view(tx, &row.text("user_id")?)?;
                requests.push(request_json(&row, author)?);
            }
            Ok(Json(json!({"requests": requests})))
        })
        .await
}

/// `PATCH /api/spaces/:id/join-requests/:requestId`.
pub async fn decide(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, request_id)): Path<(String, String)>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let accept = match str_field(&body, "action") {
        Some("accept") => true,
        Some("decline") => false,
        _ => return Err(ApiError::bad_request("validation_failed")),
    };
    state
        .run(move |tx, out| {
            let space = require_space(tx, &id)?;
            require_manager(tx, &id, &user.id)?;
            let request = request_row(tx, &request_id)?
                .filter(|row| row.text("space_id").is_ok_and(|space_id| space_id == id))
                .ok_or_else(|| ApiError::not_found("join_request_not_found"))?;
            if request.text("status")? != "pending" {
                return Err(ApiError::bad_request("join_request_decided"));
            }
            let applicant = request.text("user_id")?;
            let now = now_ms();
            if accept {
                // Оригинал здесь впускает и того, кого успели забанить после
                // подачи заявки, а на уже вступившем падает с ошибкой базы.
                if perms::is_banned(tx, &id, &applicant)? {
                    return Err(ApiError::forbidden("user_banned"));
                }
                if !perms::is_member(tx, &id, &applicant)? {
                    admit(tx, out, &id, &applicant)?;
                }
            }
            tx.execute(
                "UPDATE join_requests SET status = ?, decided_by = ?, decided_at = ? WHERE id = ?",
                params![if accept { "accepted" } else { "declined" }, &user.id, now, &request_id],
            )?;
            let row = request_row(tx, &request_id)?.ok_or_else(ApiError::internal)?;
            if accept {
                let updated = request_json(&row, user_view(tx, &applicant)?)?;
                let space_view = full_space(tx, &space, &applicant)?;
                out.to_user(&applicant, &json!({"type": "join_request_accepted", "request": updated, "space": space_view}));
                Ok(Json(updated))
            } else {
                let updated = request_json(&row, None)?;
                out.to_user(&applicant, &json!({"type": "join_request_declined", "request": updated}));
                Ok(Json(updated))
            }
        })
        .await
}

/// `GET /api/users/@me/join-requests`.
pub async fn mine(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<StatusQuery>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let status = query.status.filter(|status| !status.is_empty());
            let rows = match &status {
                Some(status) => tx.query(
                    &format!("SELECT {REQUEST_COLUMNS} FROM join_requests WHERE user_id = ? AND status = ?"),
                    params![&user.id, status],
                )?,
                None => tx.query(&format!("SELECT {REQUEST_COLUMNS} FROM join_requests WHERE user_id = ?"), params![&user.id])?,
            };
            let requests = rows.iter().map(|row| request_json(row, None)).collect::<DbResult<Vec<_>>>()?;
            Ok(Json(json!({"requests": requests})))
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::int_or;

    #[test]
    fn parse_int_with_fallback() {
        assert_eq!(int_or(Some("12abc"), 50), 12);
        assert_eq!(int_or(Some(" -3"), 50), -3);
        assert_eq!(int_or(Some("0"), 50), 50);
        assert_eq!(int_or(Some("x"), 50), 50);
        assert_eq!(int_or(None, 7), 7);
    }
}
