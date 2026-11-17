//! Двухфакторная аутентификация: настройка и управление (`/api/auth/2fa`).
//!
//! Расширение этого сервера. Вход с кодом — в `routes::auth::login`: поля
//! `totp` или `recoveryCode` рядом с именем и паролем.

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::auth::{self, AuthUser};
use crate::db::Tx;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ClientIp, JsonBody};
use crate::ratelimit::Limit;
use crate::state::{AppState, now_ms};
use crate::util::str_field;
use crate::{params, twofactor};

const SETUP_LIMIT: Limit = Limit {
    name: "auth.2fa",
    max: 15,
    window: Duration::from_mins(2),
};
/// Сколько неверных кодов прощается аккаунту за четверть часа.
const CODE_FAILURES: u32 = 5;
const CODE_WINDOW: Duration = Duration::from_mins(15);

/// Отказывает, если по аккаунту накопилось слишком много неверных кодов.
///
/// # Errors
/// Попыток слишком много.
pub fn count_attempt(state: &AppState, user_id: &str) -> ApiResult<()> {
    if state
        .hub
        .exhausted(&format!("2fa:{user_id}"), CODE_FAILURES, CODE_WINDOW)
    {
        Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "two_factor_rate_limited",
        ))
    } else {
        Ok(())
    }
}

/// Засчитывает неверный код, если проверка закончилась именно им.
///
/// # Errors
/// Та же ошибка, что пришла.
pub fn note_outcome<T>(state: &AppState, user_id: &str, outcome: ApiResult<T>) -> ApiResult<T> {
    if outcome
        .as_ref()
        .is_err_and(|error| error.code() == "two_factor_invalid")
    {
        let _ = state
            .hub
            .allow(&format!("2fa:{user_id}"), CODE_FAILURES, CODE_WINDOW);
    }
    outcome
}

/// Проверяет второй фактор из тела запроса: код по времени (`totp`) или
/// запасной (`recoveryCode`).
///
/// # Errors
/// Код не передан (`two_factor_required`) или не подошёл (`two_factor_invalid`).
pub fn check_second_factor(
    tx: &mut dyn Tx,
    server_secret: &str,
    user_id: &str,
    protection: &twofactor::State,
    body: &Value,
) -> ApiResult<()> {
    let filled = |name: &str| {
        str_field(body, name)
            .map(str::trim)
            .filter(|text| !text.is_empty())
    };
    let passed = match (filled("totp"), filled("recoveryCode")) {
        (Some(code), _) => twofactor::check_code(tx, server_secret, user_id, protection, code)?,
        (None, Some(code)) => twofactor::use_recovery_code(tx, user_id, code)?,
        (None, None) => {
            return Err(ApiError::unauthorized("two_factor_required")
                .detail("methods", json!(["totp", "recoveryCode"])));
        }
    };
    if passed {
        Ok(())
    } else {
        Err(ApiError::unauthorized("two_factor_invalid"))
    }
}

/// Имя, пароль и право аккаунта на собственную защиту: у реплики чужого
/// аккаунта вход проверяет домашний инстанс.
async fn own_account(state: &AppState, user_id: String) -> ApiResult<(String, Option<String>)> {
    state
        .run(move |tx, _| {
            let row = tx
                .query_one(
                    "SELECT username, password_hash, home_instance, federation_home_orphaned FROM users WHERE id = ?",
                    params![&user_id],
                )?
                .ok_or_else(|| ApiError::not_found("user_not_found"))?;
            let local = row.opt_text("home_instance")?.is_none_or(|home| home.is_empty())
                || row.opt_flag("federation_home_orphaned")? == Some(true);
            if !local {
                return Err(ApiError::bad_request("federated_user_password_managed_by_home"));
            }
            Ok((row.text("username")?, row.opt_text("password_hash")?))
        })
        .await
}

async fn require_password(state: &AppState, body: &Value, hash: Option<String>) -> ApiResult<()> {
    let password = str_field(body, "password")
        .filter(|password| !password.is_empty())
        .ok_or_else(|| ApiError::bad_request("password_required"))?
        .to_owned();
    if auth::verify_password(state, password, hash).await {
        Ok(())
    } else {
        Err(ApiError::forbidden("current_password_incorrect"))
    }
}

/// `GET /api/auth/2fa`.
pub async fn status(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let protection = twofactor::load(tx, &user.id)?;
            let enabled = protection.as_ref().is_some_and(|state| state.enabled_at.is_some());
            Ok(Json(json!({
                "enabled": enabled,
                "pending": protection.is_some() && !enabled,
                "enabledAt": protection.and_then(|state| state.enabled_at),
                "recoveryCodesLeft": if enabled { twofactor::recovery_codes_left(tx, &user.id)? } else { 0 },
            })))
        })
        .await
}

/// `POST /api/auth/2fa/setup`: новый секрет; защита включится после
/// подтверждения кодом.
pub async fn setup(
    State(state): State<AppState>,
    user: AuthUser,
    ClientIp(ip): ClientIp,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    state.limiter.check(SETUP_LIMIT, ip)?;
    let (username, hash) = own_account(&state, user.id.clone()).await?;
    require_password(&state, &body, hash).await?;
    let secret = twofactor::new_secret();
    let sealed =
        twofactor::seal(&state.config.jwt_secret, &secret).ok_or_else(ApiError::internal)?;
    let issuer = state
        .run(move |tx, _| {
            if twofactor::is_enabled(tx, &user.id)? {
                return Err(ApiError::conflict("two_factor_already_enabled"));
            }
            twofactor::begin(tx, &user.id, &sealed)?;
            let row = tx.query_one(
                "SELECT instance_name FROM instance_settings WHERE id = 1",
                params![],
            )?;
            Ok(row
                .and_then(|row| row.opt_text("instance_name").ok().flatten())
                .filter(|name| !name.is_empty()))
        })
        .await?
        .unwrap_or_else(|| "Rustspace".to_owned());
    Ok(Json(json!({
        "secret": twofactor::base32_encode(&secret),
        "otpauthUrl": twofactor::otpauth_url(&issuer, &username, &secret),
    })))
}

/// `POST /api/auth/2fa/enable`: подтверждение кодом из приложения.
pub async fn enable(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    count_attempt(&state, &user.id)?;
    let code = str_field(&body, "code").unwrap_or_default().to_owned();
    let secret = state.config.jwt_secret.clone();
    let owner = user.id.clone();
    let outcome = state
        .run(move |tx, _| {
            let protection = twofactor::load(tx, &user.id)?.ok_or_else(|| ApiError::bad_request("two_factor_setup_missing"))?;
            if protection.enabled_at.is_some() {
                return Err(ApiError::conflict("two_factor_already_enabled"));
            }
            if !twofactor::check_code(tx, &secret, &user.id, &protection, &code)? {
                return Err(ApiError::bad_request("two_factor_invalid"));
            }
            tx.execute("UPDATE bs_totp SET enabled_at = ? WHERE user_id = ?", params![now_ms(), &user.id])?;
            Ok(Json(json!({"enabled": true, "recoveryCodes": twofactor::issue_recovery_codes(tx, &user.id)?})))
        })
        .await;
    note_outcome(&state, &owner, outcome)
}

/// Общая часть действий над включённой защитой: пароль и второй фактор.
async fn confirmed(
    state: &AppState,
    user: &AuthUser,
    ip: std::net::IpAddr,
    body: &Value,
) -> ApiResult<()> {
    state.limiter.check(SETUP_LIMIT, ip)?;
    let (_, hash) = own_account(state, user.id.clone()).await?;
    require_password(state, body, hash).await?;
    count_attempt(state, &user.id)
}

/// `POST /api/auth/2fa/disable`.
pub async fn disable(
    State(state): State<AppState>,
    user: AuthUser,
    ClientIp(ip): ClientIp,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    confirmed(&state, &user, ip, &body).await?;
    let secret = state.config.jwt_secret.clone();
    let owner = user.id.clone();
    let outcome = state
        .run(move |tx, _| {
            let protection = twofactor::load(tx, &user.id)?
                .filter(|state| state.enabled_at.is_some())
                .ok_or_else(|| ApiError::bad_request("two_factor_not_enabled"))?;
            check_second_factor(
                tx,
                &secret,
                &user.id,
                &protection,
                &json!({"totp": body["code"], "recoveryCode": body["recoveryCode"]}),
            )?;
            twofactor::remove(tx, &user.id)?;
            Ok(Json(json!({"enabled": false})))
        })
        .await;
    note_outcome(&state, &owner, outcome)
}

/// `POST /api/auth/2fa/recovery-codes`: новый набор запасных кодов.
pub async fn recovery_codes(
    State(state): State<AppState>,
    user: AuthUser,
    ClientIp(ip): ClientIp,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    confirmed(&state, &user, ip, &body).await?;
    let secret = state.config.jwt_secret.clone();
    let owner = user.id.clone();
    let outcome = state
        .run(move |tx, _| {
            let protection = twofactor::load(tx, &user.id)?
                .filter(|state| state.enabled_at.is_some())
                .ok_or_else(|| ApiError::bad_request("two_factor_not_enabled"))?;
            let code = str_field(&body, "code").unwrap_or_default();
            if !twofactor::check_code(tx, &secret, &user.id, &protection, code)? {
                return Err(ApiError::unauthorized("two_factor_invalid"));
            }
            Ok(Json(
                json!({"recoveryCodes": twofactor::issue_recovery_codes(tx, &user.id)?}),
            ))
        })
        .await;
    note_outcome(&state, &owner, outcome)
}
