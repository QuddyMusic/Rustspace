//! Регистрация и вход (`routes/auth.ts` оригинала).

use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rand::seq::IndexedRandom as _;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::instance::registration_open;
use super::users::{USER_COLUMNS, sanitize_user, user_row};
use crate::auth::{hash_password, sign_token, verify_password};
use crate::db::{DbError, DbResult, Row, Tx};
use crate::error::{ApiError, ApiResult, error_text};
use crate::extract::{ClientIp, JsonBody};
use crate::params;
use crate::ratelimit::Limit;
use crate::state::{AppState, now_ms};

const USERNAME_MIN: usize = 3;
const USERNAME_MAX: usize = 32;
const FEDERATED_USERNAME_MAX: usize = 100;
const PASSWORD_MIN_LENGTH: usize = 8;
/// Цвета аватара (`AVATAR_COLORS`).
pub const AVATAR_COLORS: [&str; 7] = ["mint", "sky", "lavender", "coral", "rose", "teal", "amber"];

const REGISTER_LIMIT: Limit = Limit {
    name: "auth.register",
    max: 10,
    window: Duration::from_mins(2),
};
const CHECK_LIMIT: Limit = Limit {
    name: "auth.check",
    max: 30,
    window: Duration::from_mins(1),
};
const LOGIN_LIMIT: Limit = Limit {
    name: "auth.login",
    max: 15,
    window: Duration::from_mins(2),
};

/// Непустая строка из тела запроса (`!x || typeof x !== 'string'`).
fn text<'a>(body: &'a Value, key: &str) -> Option<&'a str> {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// Длина строки, как её считает JavaScript (`.length`, единицы UTF-16).
fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn is_local_name(name: &str) -> bool {
    name.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn is_domain(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn length_error() -> ApiError {
    ApiError::bad_request("username_length_invalid")
        .detail("min", USERNAME_MIN)
        .detail("max", USERNAME_MAX)
}

/// Проверяет имя для регистрации; `home` — домен домашнего инстанса у
/// реплицированного аккаунта (тогда имя — `локальное@домен`).
fn check_username(name: &str, home: Option<&str>) -> ApiResult<()> {
    if home.is_none() {
        if !(USERNAME_MIN..=USERNAME_MAX).contains(&js_len(name)) {
            return Err(length_error());
        }
        if !is_local_name(name) {
            return Err(ApiError::bad_request("username_characters_invalid"));
        }
        return Ok(());
    }
    let Some((local, domain)) = name.split_once('@') else {
        return Err(ApiError::bad_request("replicated_username_format_required"));
    };
    if !(USERNAME_MIN..=USERNAME_MAX).contains(&local.len()) || !is_local_name(local) {
        return Err(ApiError::bad_request("username_local_part_invalid")
            .detail("min", USERNAME_MIN)
            .detail("max", USERNAME_MAX));
    }
    if js_len(domain) > 253 || !is_domain(domain) {
        return Err(ApiError::bad_request("username_domain_part_invalid"));
    }
    if js_len(name) > FEDERATED_USERNAME_MAX {
        return Err(
            ApiError::bad_request("username_too_long").detail("max", FEDERATED_USERNAME_MAX)
        );
    }
    Ok(())
}

/// Строка приглашения по токену (22 символа base64url).
fn invite_by_token(tx: &mut dyn Tx, token: &str) -> DbResult<Option<Row>> {
    let well_formed = token.len() == 22
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !well_formed {
        return Ok(None);
    }
    tx.query_one(
        "SELECT id, name, max_uses, used_count, expires_at, revoked_at FROM invite_links WHERE token = ?",
        params![token],
    )
}

/// Состояние приглашения (`inviteStatus`).
fn invite_status(row: &Row) -> DbResult<&'static str> {
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

/// Что нужно, чтобы создать пользователя.
struct NewUser {
    id: String,
    username: String,
    display_name: Option<String>,
    password_hash: String,
    home_instance: Option<String>,
    home_user_id: Option<String>,
    avatar_color: String,
    invite: Option<String>,
}

/// Создаёт пользователя одной транзакцией: проверка имени, первый
/// пользователь становится администратором, приглашение гасится вместе со
/// вставкой (в оригинале проверка имени и вставка не атомарны).
fn create_user(
    state: &AppState,
    tx: &mut dyn Tx,
    new: &NewUser,
    redeem_id: &str,
) -> ApiResult<Row> {
    let exists = tx
        .query_one(
            "SELECT id FROM users WHERE username = ?",
            params![&new.username],
        )?
        .is_some();
    if exists {
        return Err(ApiError::conflict("username_taken"));
    }
    let invite = match &new.invite {
        Some(token) if !registration_open(state, tx)? => {
            let row = invite_by_token(tx, token)?
                .filter(|row| invite_status(row).ok() == Some("active"))
                .ok_or_else(|| ApiError::forbidden("invite_invalid"))?;
            Some(row)
        }
        _ => None,
    };
    let first = new.home_instance.is_none()
        && tx
            .query_one("SELECT id FROM users LIMIT 1", params![])?
            .is_none();
    let now = now_ms();
    tx.execute(
        "INSERT INTO users (id, username, display_name, password_hash, is_admin, home_instance, \
         home_user_id, avatar_color, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            &new.id,
            &new.username,
            new.display_name.as_ref(),
            &new.password_hash,
            first,
            new.home_instance.as_ref(),
            new.home_user_id.as_ref(),
            &new.avatar_color,
            now
        ],
    )
    .map_err(|err| {
        // Гонка двух регистраций одного имени упирается в уникальный индекс.
        if err.is_unique_violation() {
            ApiError::conflict("username_taken")
        } else {
            err.into()
        }
    })?;
    if let Some(invite) = invite {
        let invite_id = invite.text("id")?;
        tx.execute(
            "UPDATE invite_links SET used_count = used_count + 1 WHERE id = ?",
            params![&invite_id],
        )?;
        tx.execute(
            "INSERT INTO invite_redemptions (id, invite_id, user_id, registrant_username, redeemed_at) \
             VALUES (?, ?, ?, ?, ?)",
            params![redeem_id, &invite_id, &new.id, &new.username, now],
        )?;
    }
    user_row(tx, &new.id)?
        .ok_or_else(|| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "user_create_failed"))
}

/// `POST /api/auth/register`.
pub async fn register(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    JsonBody(body): JsonBody,
) -> ApiResult<Response> {
    state.limiter.check(REGISTER_LIMIT, ip)?;
    let username =
        text(&body, "username").ok_or_else(|| ApiError::bad_request("username_required"))?;
    let password =
        text(&body, "password").ok_or_else(|| ApiError::bad_request("password_required"))?;
    let username = username.trim().to_lowercase();
    let home_instance = match body.get("homeInstance") {
        None | Some(Value::Null | Value::Bool(false)) => None,
        Some(Value::String(home)) if home.is_empty() => None,
        Some(Value::String(home)) if js_len(home) <= 253 && is_domain(home) => Some(home.clone()),
        Some(_) => return Err(ApiError::bad_request("home_instance_invalid")),
    };
    check_username(&username, home_instance.as_deref())?;
    if js_len(password) < PASSWORD_MIN_LENGTH {
        return Err(ApiError::bad_request("password_too_short").detail("min", PASSWORD_MIN_LENGTH));
    }
    let invite = body
        .get("inviteToken")
        .and_then(Value::as_str)
        .map(str::to_owned);

    // Дешёвые отказы — до дорогого bcrypt.
    let (gate_state, gate_home, gate_invite) =
        (state.clone(), home_instance.clone(), invite.clone());
    state
        .db
        .transaction(move |tx| {
            if gate_home.is_some() {
                let open = tx
                    .query_one(
                        "SELECT federated_registration_open FROM instance_settings WHERE id = 1",
                        params![],
                    )?
                    .map(|row| row.opt_flag("federated_registration_open"))
                    .transpose()?
                    .flatten()
                    == Some(true);
                if !open {
                    return Err(ApiError::forbidden("federated_registration_closed"));
                }
            } else if !registration_open(&gate_state, tx)? {
                let token = gate_invite.ok_or_else(|| ApiError::forbidden("invite_required"))?;
                invite_by_token(tx, &token)?
                    .filter(|row| invite_status(row).ok() == Some("active"))
                    .ok_or_else(|| ApiError::forbidden("invite_invalid"))?;
            }
            Ok(())
        })
        .await?;

    let avatar_color = body
        .get("avatarColor")
        .and_then(Value::as_str)
        .filter(|color| AVATAR_COLORS.contains(color))
        .or_else(|| AVATAR_COLORS.choose(&mut rand::rng()).copied())
        .unwrap_or("mint")
        .to_owned();
    let new = NewUser {
        id: state.ids.next(),
        password_hash: hash_password(&state, password.to_owned()).await?,
        display_name: body
            .get("displayName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
        home_user_id: home_instance
            .as_ref()
            .and_then(|_| text(&body, "homeUserId"))
            .map(str::to_owned),
        invite: home_instance.is_none().then_some(invite).flatten(),
        home_instance,
        avatar_color,
        username,
    };
    let redeem_id = state.ids.next();
    let tx_state = state.clone();
    let (id, name, user) = state
        .db
        .transaction(move |tx| {
            let row = create_user(&tx_state, tx, &new, &redeem_id)?;
            Ok::<_, ApiError>((
                row.text("id")?,
                row.text("username")?,
                sanitize_user(&row, true)?,
            ))
        })
        .await?;
    let token = sign_token(&state, &id, &name)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "token": token, "user": user })),
    )
        .into_response())
}

/// Параметры `check-username`.
#[derive(Debug, Deserialize)]
pub struct UsernameQuery {
    username: Option<String>,
}

fn unavailable(
    status: StatusCode,
    error: &ApiError,
    details: Option<Map<String, Value>>,
) -> Response {
    let mut body = json!({
        "available": false,
        "reason": error_text(error.code(), details.as_ref()),
        "code": error.code(),
    });
    if let Some(details) = details {
        body["details"] = Value::Object(details);
    }
    (status, Json(body)).into_response()
}

/// `GET /api/auth/check-username`.
pub async fn check_username_route(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Query(query): Query<UsernameQuery>,
) -> ApiResult<Response> {
    state.limiter.check(CHECK_LIMIT, ip)?;
    let Some(raw) = query.username.filter(|name| !name.is_empty()) else {
        return Ok(unavailable(
            StatusCode::BAD_REQUEST,
            &ApiError::bad_request("username_required"),
            None,
        ));
    };
    let name = raw.trim().to_lowercase();
    if !(USERNAME_MIN..=USERNAME_MAX).contains(&js_len(&name)) {
        let mut details = Map::new();
        details.insert("min".into(), USERNAME_MIN.into());
        details.insert("max".into(), USERNAME_MAX.into());
        return Ok(unavailable(StatusCode::OK, &length_error(), Some(details)));
    }
    if !is_local_name(&name) {
        return Ok(unavailable(
            StatusCode::OK,
            &ApiError::bad_request("username_characters_invalid"),
            None,
        ));
    }
    let db = state.db.clone();
    db.transaction(move |tx| {
        if !registration_open(&state, tx)? {
            return Ok(unavailable(
                StatusCode::FORBIDDEN,
                &ApiError::forbidden("registration_closed"),
                None,
            ));
        }
        let taken = tx
            .query_one("SELECT id FROM users WHERE username = ?", params![&name])?
            .is_some();
        Ok(Json(json!({ "available": !taken })).into_response())
    })
    .await
}

/// Параметры `check-invite`.
#[derive(Debug, Deserialize)]
pub struct InviteQuery {
    token: Option<String>,
}

/// `GET /api/auth/check-invite`.
pub async fn check_invite(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Query(query): Query<InviteQuery>,
) -> ApiResult<Json<Value>> {
    state.limiter.check(CHECK_LIMIT, ip)?;
    let invalid = || Json(json!({ "valid": false, "reason": "invalid" }));
    let Some(token) = query.token.filter(|token| !token.is_empty()) else {
        return Ok(invalid());
    };
    state
        .db
        .transaction(move |tx| {
            let Some(row) = invite_by_token(tx, &token)? else {
                return Ok(invalid());
            };
            Ok(match invite_status(&row)? {
                "active" => Json(json!({ "valid": true, "name": row.text("name")? })),
                status @ ("expired" | "exhausted") => {
                    Json(json!({ "valid": false, "reason": status }))
                }
                _ => invalid(),
            })
        })
        .await
}

/// `POST /api/auth/login`.
pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    state.limiter.check(LOGIN_LIMIT, ip)?;
    let username =
        text(&body, "username").ok_or_else(|| ApiError::bad_request("username_required"))?;
    let password =
        text(&body, "password").ok_or_else(|| ApiError::bad_request("password_required"))?;
    let name = username.trim().to_lowercase();
    let found = state
        .db
        .transaction(move |tx| {
            let row = tx.query_one(
                &format!("SELECT {USER_COLUMNS}, password_hash FROM users WHERE username = ?"),
                params![&name],
            )?;
            Ok::<_, DbError>(row)
        })
        .await?;
    // bcrypt считается всегда — и для несуществующего имени, и для
    // удалённого аккаунта: время ответа не выдаёт, есть ли такой пользователь.
    let hash = found
        .as_ref()
        .map(|row| row.text("password_hash"))
        .transpose()?;
    let valid = verify_password(&state, password.to_owned(), hash).await;
    let invalid = || ApiError::unauthorized("invalid_credentials");
    let row = found.ok_or_else(invalid)?;
    if row.opt_flag("is_deleted")? == Some(true) {
        return Err(ApiError::unauthorized("account_deleted"));
    }
    if !valid {
        // Реплицированный аккаунт со сменённым дома паролем оригинал
        // перепроверяет у домашнего инстанса; это часть федерации.
        return Err(invalid());
    }
    // Второй фактор спрашивается только после верного пароля: по ответу
    // нельзя узнать, включена ли защита, не зная пароля.
    let user_id = row.text("id")?;
    let (owner, secret, factor_body) = (
        user_id.clone(),
        state.config.jwt_secret.clone(),
        body.clone(),
    );
    let protection = state
        .run(move |tx, _| Ok(crate::twofactor::load(tx, &owner)?))
        .await?;
    if let Some(protection) = protection.filter(|state| state.enabled_at.is_some()) {
        crate::routes::twofactor::count_attempt(&state, &user_id)?;
        let owner = user_id.clone();
        let outcome = state
            .run(move |tx, _| {
                crate::routes::twofactor::check_second_factor(
                    tx,
                    &secret,
                    &owner,
                    &protection,
                    &factor_body,
                )
            })
            .await;
        crate::routes::twofactor::note_outcome(&state, &user_id, outcome)?;
    }
    let token = sign_token(&state, &user_id, &row.text("username")?)?;
    Ok(Json(
        json!({ "token": token, "user": sanitize_user(&row, true)? }),
    ))
}
