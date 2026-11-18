//! Пароли, токены и проверка запроса — как `utils/auth.ts` оригинала.
//!
//! Формат совместим в обе стороны: bcrypt-хеши оригинала проверяются этим
//! сервером и наоборот; JWT подписан HS256 с полями `userId`, `username`,
//! `iat`, `exp`, так что токены, выданные оригиналом с тем же
//! `JWT_SECRET`, продолжают работать.

use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::db::Tx;
use crate::error::{ApiError, ApiResult};
use crate::params;
use crate::state::AppState;

/// Поля токена.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Claims {
    #[serde(rename = "userId")]
    user_id: String,
    username: String,
    iat: u64,
    exp: u64,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Выдаёт токен пользователю.
///
/// # Errors
/// Подпись не удалась (на практике — никогда).
pub fn sign_token(state: &AppState, user_id: &str, username: &str) -> ApiResult<String> {
    let now = now_secs();
    let claims = Claims {
        user_id: user_id.to_owned(),
        username: username.to_owned(),
        iat: now,
        exp: now + state.config.jwt_expires_secs,
    };
    jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(state.config.jwt_secret.as_bytes()),
    )
    .map_err(|err| {
        tracing::error!(%err, "токен не подписан");
        ApiError::internal()
    })
}

fn decode_token(state: &AppState, token: &str) -> Option<Claims> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 0;
    validation.set_required_spec_claims(&["exp"]);
    jsonwebtoken::decode::<Claims>(
        token,
        &DecodingKey::from_secret(state.config.jwt_secret.as_bytes()),
        &validation,
    )
    .ok()
    .map(|data| data.claims)
}

/// Хеширует пароль (bcrypt). Блокирует поток: вызывать через
/// [`hash_password`].
fn hash_blocking(password: &str, cost: u32) -> ApiResult<String> {
    bcrypt::hash(password, cost).map_err(|err| {
        tracing::warn!(%err, "пароль не хешируется");
        ApiError::bad_request("validation_failed")
    })
}

/// Хеширует пароль, не занимая исполнитель.
///
/// # Errors
/// Пароль нельзя хешировать (например, содержит нулевой байт).
pub async fn hash_password(state: &AppState, password: String) -> ApiResult<String> {
    let cost = state.config.password_hash_cost;
    tokio::task::spawn_blocking(move || {
        let password = zeroize::Zeroizing::new(password);
        hash_blocking(&password, cost)
    })
    .await
    .map_err(|_| ApiError::internal())?
}

/// Проверяет пароль по хешу. Если хеша нет (такого пользователя нет),
/// всё равно считает bcrypt — по времени ответа нельзя узнать, существует
/// ли имя.
pub async fn verify_password(state: &AppState, password: String, hash: Option<String>) -> bool {
    let dummy = state.dummy_hash.clone();
    tokio::task::spawn_blocking(move || {
        let password = zeroize::Zeroizing::new(password);
        hash.map_or_else(
            || {
                let _ = bcrypt::verify(password.as_str(), &dummy);
                false
            },
            |hash| bcrypt::verify(password.as_str(), &hash).unwrap_or(false),
        )
    })
    .await
    .unwrap_or(false)
}

/// Хеш для холостой проверки (той же стоимости, что настоящие).
///
/// # Panics
/// Не паникует: стоимость проверена конфигурацией.
#[must_use]
pub fn dummy_hash(cost: u32) -> String {
    bcrypt::hash("backspace-timing-equalizer", cost).unwrap_or_default()
}

/// Пользователь, от имени которого идёт запрос.
#[derive(Debug, Clone)]
pub struct AuthUser {
    /// Идентификатор.
    pub id: String,
    /// Имя из токена.
    pub username: String,
    /// Домашний инстанс (для реплицированных аккаунтов).
    pub home_instance: Option<String>,
    /// Вошёл бот (токеном бота, см. [`crate::bots`]).
    pub bot: bool,
}

/// Проверяет токен и состояние аккаунта (`verifyJwtAndUser`).
///
/// # Errors
/// Токен недействителен, аккаунт удалён или пароль с тех пор сменили.
pub fn verify_token(state: &AppState, tx: &mut dyn Tx, token: &str) -> ApiResult<AuthUser> {
    if crate::bots::looks_like_token(token) {
        return crate::bots::verify(tx, token);
    }
    let invalid = || ApiError::unauthorized("unauthorized").text("Invalid or expired token");
    let claims = decode_token(state, token).ok_or_else(invalid)?;
    let row = tx.query_one(
        "SELECT is_deleted, password_changed_at, home_instance FROM users WHERE id = ?",
        params![&claims.user_id],
    )?;
    let deleted = || ApiError::unauthorized("account_deleted");
    let row = row.ok_or_else(deleted)?;
    if row.opt_flag("is_deleted")? == Some(true) {
        return Err(deleted());
    }
    if let Some(changed) = row.opt_int("password_changed_at")?.filter(|ms| *ms > 0)
        && i64::try_from(claims.iat).unwrap_or(i64::MAX) < changed / 1000
    {
        return Err(ApiError::unauthorized("unauthorized")
            .text("Token has been revoked — please log in again"));
    }
    Ok(AuthUser {
        id: claims.user_id,
        username: claims.username,
        home_instance: row.opt_text("home_instance")?,
        bot: false,
    })
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                value
                    .strip_prefix("Bearer ")
                    .or_else(|| value.strip_prefix("Bot "))
            })
            .ok_or_else(|| {
                ApiError::unauthorized("unauthorized")
                    .text("Missing or invalid authorization header")
            })?
            .to_owned();
        let state = state.clone();
        let db = state.db.clone();
        let path = parts.uri.path().to_owned();
        let method = parts.method.clone();
        let user = db
            .transaction(move |tx| verify_token(&state, tx, &token))
            .await?;
        if user.bot && !crate::bots::allowed(&method, &path) {
            return Err(ApiError::forbidden("bots_not_allowed"));
        }
        Ok(user)
    }
}
