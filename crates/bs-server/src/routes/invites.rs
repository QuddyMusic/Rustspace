//! Приглашения на регистрацию — управление администратором
//! (`routes/invites.ts`, `utils/inviteService.ts`).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rand::Rng;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::config::Config;
use crate::db::{DbResult, Row, Tx};
use crate::error::ApiError;
use crate::extract::JsonBody;
use crate::params;
use crate::state::{AppState, now_ms};
use crate::util::{field, js_trim, utf16_len};

const COLUMNS: &str =
    "id, token, name, created_by, created_at, max_uses, used_count, expires_at, revoked_at";

/// Отказ с текстом, как отвечает оригинал: `{error, statusCode}` без кода.
#[derive(Debug)]
pub struct Plain(StatusCode, String);

impl IntoResponse for Plain {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({"error": self.1, "statusCode": self.0.as_u16()})),
        )
            .into_response()
    }
}

/// Ошибка маршрутов приглашений.
#[derive(Debug)]
pub enum Refusal {
    /// Отказ с текстом.
    Plain(Plain),
    /// Обычная ошибка API.
    Api(ApiError),
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        match self {
            Self::Plain(plain) => plain.into_response(),
            Self::Api(error) => error.into_response(),
        }
    }
}

impl From<ApiError> for Refusal {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl From<crate::db::DbError> for Refusal {
    fn from(error: crate::db::DbError) -> Self {
        Self::Api(error.into())
    }
}

fn invalid(message: impl Into<String>) -> Refusal {
    Refusal::Plain(Plain(StatusCode::BAD_REQUEST, message.into()))
}

fn conflict(message: &str) -> Refusal {
    Refusal::Plain(Plain(StatusCode::CONFLICT, message.to_owned()))
}

fn not_found() -> Refusal {
    Refusal::Plain(Plain(StatusCode::NOT_FOUND, "Invite not found".to_owned()))
}

/// Ответ тому, кто не администратор инстанса (`requireAdmin`).
#[must_use]
pub fn admin_only() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": "Only instance admins can perform this action",
            "code": "forbidden",
            "statusCode": 403,
        })),
    )
        .into_response()
}

/// Администратор ли пользователь.
///
/// # Errors
/// Ошибка базы.
pub fn is_admin(tx: &mut dyn Tx, user_id: &str) -> DbResult<bool> {
    Ok(tx
        .query_one("SELECT is_admin FROM users WHERE id = ?", params![user_id])?
        .map(|row| row.opt_flag("is_admin"))
        .transpose()?
        .flatten()
        == Some(true))
}

/// Выполняет `work` для администратора; остальным — отказ `requireAdmin`.
///
/// # Errors
/// Ошибка из `work` или базы.
pub async fn as_admin<T, E, F>(
    state: &AppState,
    user: &AuthUser,
    work: F,
) -> Result<Result<T, Response>, E>
where
    T: Send + 'static,
    E: From<ApiError> + Send + 'static,
    F: FnOnce(&mut dyn Tx, &mut crate::hub::Outbox) -> Result<T, E> + Send + 'static,
{
    let user_id = user.id.clone();
    let outcome = state
        .run(move |tx, out| {
            if !is_admin(tx, &user_id)? {
                return Ok(None);
            }
            Ok(Some(work(tx, out)))
        })
        .await
        .map_err(E::from)?;
    match outcome {
        None => Ok(Err(admin_only())),
        Some(Ok(value)) => Ok(Ok(value)),
        Some(Err(error)) => Err(error),
    }
}

fn status_of(row: &Row) -> DbResult<&'static str> {
    if row.opt_int("revoked_at")?.is_some() {
        return Ok("revoked");
    }
    if row.opt_int("expires_at")?.is_some_and(|at| at < now_ms()) {
        return Ok("expired");
    }
    if row
        .opt_int("max_uses")?
        .is_some_and(|max| row.int("used_count").unwrap_or(0) >= max)
    {
        return Ok("exhausted");
    }
    Ok("active")
}

fn new_token() -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::rng().random::<[u8; 16]>())
}

fn invite_url(config: &Config, token: &str) -> String {
    match config.domain.as_deref().filter(|domain| !domain.is_empty()) {
        Some(domain) => format!("https://{domain}/register?invite={token}"),
        None => format!("http://localhost:{}/register?invite={token}", config.port),
    }
}

fn creator_name(tx: &mut dyn Tx, creator: Option<&str>) -> DbResult<Option<String>> {
    let Some(creator) = creator else {
        return Ok(None);
    };
    let Some(row) = tx.query_one(
        "SELECT username, is_deleted FROM users WHERE id = ?",
        params![creator],
    )?
    else {
        return Ok(None);
    };
    Ok(Some(if row.opt_flag("is_deleted")? == Some(true) {
        "Deleted User".to_owned()
    } else {
        row.text("username")?
    }))
}

fn summary(tx: &mut dyn Tx, config: &Config, row: &Row) -> DbResult<Value> {
    let id = row.text("id")?;
    let token = row.text("token")?;
    let creator = row.opt_text("created_by")?;
    let last = tx
        .query_one(
            "SELECT MAX(redeemed_at) AS last FROM invite_redemptions WHERE invite_id = ?",
            params![&id],
        )?
        .map(|row| row.opt_int("last"))
        .transpose()?
        .flatten();
    Ok(json!({
        "id": id,
        "name": row.text("name")?,
        "status": status_of(row)?,
        "maxUses": row.opt_int("max_uses")?,
        "usedCount": row.int("used_count")?,
        "expiresAt": row.opt_int("expires_at")?,
        "revokedAt": row.opt_int("revoked_at")?,
        "createdByUsername": creator_name(tx, creator.as_deref())?,
        "createdBy": creator,
        "createdAt": row.int("created_at")?,
        "lastRedeemedAt": last,
        "url": invite_url(config, &token),
        "token": token,
    }))
}

fn invite_row(tx: &mut dyn Tx, id: &str) -> Result<Row, Refusal> {
    tx.query_one(
        &format!("SELECT {COLUMNS} FROM invite_links WHERE id = ?"),
        params![id],
    )?
    .ok_or_else(not_found)
}

fn valid_name(value: Option<&Value>) -> Result<String, Refusal> {
    let trimmed = value
        .and_then(Value::as_str)
        .map(js_trim)
        .unwrap_or_default();
    if trimmed.is_empty() || utf16_len(trimmed) > 64 {
        return Err(invalid("Name must be 1-64 characters"));
    }
    Ok(trimmed.to_owned())
}

/// Целое из JSON: `Number.isInteger`.
fn integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        value
            .as_f64()
            .filter(|n| n.fract() == 0.0 && n.abs() < 9.0e15)
            .map(|n| {
                #[allow(clippy::cast_possible_truncation, reason = "проверено выше")]
                let whole = n as i64;
                whole
            })
    })
}

fn valid_max_uses(value: &Value) -> Result<Option<i64>, Refusal> {
    if value.is_null() {
        return Ok(None);
    }
    integer(value)
        .filter(|max| *max >= 1)
        .map(Some)
        .ok_or_else(|| invalid("maxUses must be a positive integer or null"))
}

fn valid_expires(value: &Value, allow_past: bool) -> Result<Option<i64>, Refusal> {
    if value.is_null() {
        return Ok(None);
    }
    let at =
        integer(value).ok_or_else(|| invalid("expiresAt must be an integer epoch ms or null"))?;
    if !allow_past && at <= now_ms() {
        return Err(invalid("expiresAt must be in the future"));
    }
    Ok(Some(at))
}

fn respond<T: IntoResponse>(outcome: Result<Result<T, Response>, Refusal>) -> Response {
    match outcome {
        Ok(Ok(value)) => value.into_response(),
        Ok(Err(refused)) => refused,
        Err(refusal) => refusal.into_response(),
    }
}

/// `POST /api/admin/invites`.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> Response {
    let app = state.clone();
    let creator = user.id.clone();
    respond(
        as_admin(&state, &user, move |tx, _| {
            let name = valid_name(field(&body, "name"))?;
            // Отсутствующее поле оригинал пропускает дальше как `undefined`;
            // здесь оно значит «без ограничения».
            let max_uses = valid_max_uses(field(&body, "maxUses").unwrap_or(&Value::Null))?;
            let expires = valid_expires(field(&body, "expiresAt").unwrap_or(&Value::Null), false)?;
            let id = app.ids.next();
            tx.execute(
                "INSERT INTO invite_links (id, token, name, created_by, created_at, max_uses, used_count, \
                 expires_at, revoked_at) VALUES (?, ?, ?, ?, ?, ?, 0, ?, NULL)",
                params![&id, new_token(), name, &creator, now_ms(), max_uses, expires],
            )?;
            let row = invite_row(tx, &id)?;
            Ok::<_, Refusal>((StatusCode::CREATED, Json(summary(tx, &app.config, &row)?)))
        })
        .await,
    )
}

/// Параметр списка приглашений.
#[derive(Debug, serde::Deserialize)]
pub struct ListQuery {
    status: Option<String>,
}

/// `GET /api/admin/invites`.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<ListQuery>,
) -> Response {
    let app = state.clone();
    let archived = query.status.as_deref() == Some("archived");
    respond(
        as_admin(&state, &user, move |tx, _| {
            let rows = tx.query(
                &format!("SELECT {COLUMNS} FROM invite_links ORDER BY created_at DESC"),
                params![],
            )?;
            let mut invites = Vec::new();
            for row in &rows {
                if (status_of(row)? == "active") != archived {
                    invites.push(summary(tx, &app.config, row)?);
                }
            }
            Ok::<_, Refusal>(Json(json!({"invites": invites})))
        })
        .await,
    )
}

/// Изменения лимита и срока, общие для правки и восстановления.
fn limit_updates(
    body: &Value,
    row: &Row,
    sets: &mut Vec<(&'static str, crate::db::Value)>,
) -> Result<(), Refusal> {
    if let Some(value) = field(body, "maxUses") {
        let max = valid_max_uses(value)?;
        let used = row.int("used_count")?;
        if let Some(max) = max.filter(|max| *max < used) {
            return Err(invalid(format!(
                "maxUses ({max}) cannot be less than current usedCount ({used})"
            )));
        }
        sets.push(("max_uses", max.into()));
    }
    if let Some(value) = field(body, "expiresAt") {
        sets.push(("expires_at", valid_expires(value, true)?.into()));
    }
    Ok(())
}

fn apply(tx: &mut dyn Tx, id: &str, sets: Vec<(&'static str, crate::db::Value)>) -> DbResult<()> {
    if sets.is_empty() {
        return Ok(());
    }
    let assignments: Vec<String> = sets
        .iter()
        .map(|(column, _)| format!("{column} = ?"))
        .collect();
    let mut values: Vec<crate::db::Value> = sets.into_iter().map(|(_, value)| value).collect();
    values.push(id.into());
    tx.execute(
        &format!(
            "UPDATE invite_links SET {} WHERE id = ?",
            assignments.join(", ")
        ),
        &values,
    )?;
    Ok(())
}

/// `PATCH /api/admin/invites/:id`.
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> Response {
    let app = state.clone();
    respond(
        as_admin(&state, &user, move |tx, _| {
            let row = invite_row(tx, &id)?;
            if row.opt_int("revoked_at")?.is_some() {
                return Err(conflict("Invite is revoked. Reinstate first to modify."));
            }
            let mut sets = Vec::new();
            if let Some(name) = field(&body, "name") {
                sets.push(("name", valid_name(Some(name))?.into()));
            }
            limit_updates(&body, &row, &mut sets)?;
            apply(tx, &id, sets)?;
            let updated = invite_row(tx, &id)?;
            Ok::<_, Refusal>(Json(summary(tx, &app.config, &updated)?))
        })
        .await,
    )
}

/// `POST /api/admin/invites/:id/revoke`.
pub async fn revoke(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Response {
    let app = state.clone();
    respond(
        as_admin(&state, &user, move |tx, _| {
            let row = invite_row(tx, &id)?;
            if row.opt_int("revoked_at")?.is_some() {
                return Err(conflict("Invite is already revoked"));
            }
            tx.execute(
                "UPDATE invite_links SET revoked_at = ? WHERE id = ?",
                params![now_ms(), &id],
            )?;
            let updated = invite_row(tx, &id)?;
            Ok::<_, Refusal>(Json(json!({"invite": summary(tx, &app.config, &updated)?})))
        })
        .await,
    )
}

/// `POST /api/admin/invites/:id/reinstate`.
pub async fn reinstate(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let app = state.clone();
    // Тело необязательно: без него приглашение просто возвращается в строй.
    let body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    respond(
        as_admin(&state, &user, move |tx, _| {
            let row = invite_row(tx, &id)?;
            let status = status_of(&row)?;
            if status == "active" {
                return Err(conflict("Invite is already active"));
            }
            let mut sets: Vec<(&'static str, crate::db::Value)> = Vec::new();
            let rotated = status == "revoked";
            if rotated {
                sets.push(("revoked_at", crate::db::Value::Null));
                sets.push(("token", new_token().into()));
            }
            limit_updates(&body, &row, &mut sets)?;
            apply(tx, &id, sets)?;
            let updated = invite_row(tx, &id)?;
            if status_of(&updated)? != "active" {
                return Err(invalid(
                    "Reinstate would leave invite in non-active state. Bump maxUses and/or expiresAt.",
                ));
            }
            Ok::<_, Refusal>(Json(json!({
                "invite": summary(tx, &app.config, &updated)?,
                "tokenRotated": rotated,
            })))
        })
        .await,
    )
}

/// `DELETE /api/admin/invites/:id`.
pub async fn delete(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Response {
    respond(
        as_admin(&state, &user, move |tx, _| {
            invite_row(tx, &id)?;
            tx.execute("DELETE FROM invite_links WHERE id = ?", params![&id])?;
            Ok::<_, Refusal>(Json(json!({"success": true})))
        })
        .await,
    )
}

/// `GET /api/admin/invites/:id/redemptions`.
pub async fn redemptions(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Response {
    respond(
        as_admin(&state, &user, move |tx, _| {
            let rows = tx.query(
                "SELECT r.id, r.user_id, r.registrant_username, r.redeemed_at, u.username AS current_name, \
                 u.is_deleted AS current_deleted FROM invite_redemptions r \
                 LEFT JOIN users u ON u.id = r.user_id WHERE r.invite_id = ? ORDER BY r.redeemed_at DESC",
                params![&id],
            )?;
            let list = rows
                .iter()
                .map(|row| {
                    let deleted = row.opt_flag("current_deleted")? == Some(true);
                    let user_id = row.opt_text("user_id")?;
                    let current = match (&user_id, row.opt_text("current_name")?) {
                        (Some(_), Some(_)) if deleted => Some("Deleted User".to_owned()),
                        (Some(_), name) => name,
                        (None, _) => None,
                    };
                    Ok(json!({
                        "id": row.text("id")?,
                        "userId": user_id,
                        "registrantUsername": row.opt_text("registrant_username")?,
                        "currentUsername": current,
                        "isDeleted": deleted,
                        "redeemedAt": row.int("redeemed_at")?,
                    }))
                })
                .collect::<DbResult<Vec<_>>>()?;
            Ok::<_, Refusal>(Json(json!({"redemptions": list})))
        })
        .await,
    )
}
