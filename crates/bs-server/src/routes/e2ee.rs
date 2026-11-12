//! Справочник открытых ключей сквозного шифрования личных сообщений
//! (`/api/e2ee`). Расширение этого сервера.
//!
//! Сервер хранит только открытые ключи и не участвует в шифровании:
//! зашифрованное сообщение для него — обычный текст `bs-e2ee:1:…`. Справочник
//! нужен, чтобы написать собеседнику зашифрованно, пока он не в сети; без
//! него клиенты обмениваются ключами прямо в переписке.

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::JsonBody;
use crate::params;
use crate::state::{AppState, now_ms};
use crate::util::str_field;

/// Открытый ключ X25519 в base64url без дополнения: 32 байта — 43 знака.
fn is_public_key(text: &str) -> bool {
    text.len() == 43
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        // Последний знак несёт четыре значащих бита: остальные два — нули.
        && text.as_bytes().last().is_some_and(|last| b"AEIMQUYcgkosw048".contains(last))
}

fn key_json(tx: &mut dyn Tx, user_id: &str) -> DbResult<Option<Value>> {
    tx.query_one("SELECT public_key, updated_at FROM bs_e2ee_keys WHERE user_id = ?", params![user_id])?
        .map(|row| Ok(json!({"userId": user_id, "publicKey": row.text("public_key")?, "updatedAt": row.int("updated_at")?})))
        .transpose()
}

/// `GET /api/e2ee/key`: свой опубликованный ключ.
pub async fn mine(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            Ok(Json(key_json(tx, &user.id)?.unwrap_or_else(
                || json!({"userId": user.id, "publicKey": null, "updatedAt": null}),
            )))
        })
        .await
}

/// `PUT /api/e2ee/key`: публикует или заменяет свой открытый ключ.
pub async fn publish(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let key = str_field(&body, "publicKey")
        .filter(|key| is_public_key(key))
        .ok_or_else(|| ApiError::bad_request("e2ee_key_invalid"))?
        .to_owned();
    state
        .run(move |tx, _| {
            let now = now_ms();
            tx.execute(
                "INSERT INTO bs_e2ee_keys (user_id, public_key, updated_at) VALUES (?, ?, ?) \
                 ON CONFLICT (user_id) DO UPDATE SET public_key = ?, updated_at = ?",
                params![&user.id, &key, now, &key, now],
            )?;
            Ok(Json(
                json!({"userId": user.id, "publicKey": key, "updatedAt": now}),
            ))
        })
        .await
}

/// `DELETE /api/e2ee/key`: убирает свой ключ из справочника.
pub async fn withdraw(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            tx.execute(
                "DELETE FROM bs_e2ee_keys WHERE user_id = ?",
                params![&user.id],
            )?;
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `GET /api/e2ee/keys/:userId`: открытый ключ другого пользователя.
pub async fn of_user(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(user_id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            key_json(tx, &user_id)?
                .map(Json)
                .ok_or_else(|| ApiError::not_found("e2ee_key_not_found"))
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::is_public_key;

    #[test]
    fn only_canonical_32_byte_keys_are_accepted() {
        assert!(is_public_key("hSDwCYkwp1R0i33ctD73Wg2_Og0mOBr066SpjqqbTmo"));
        assert!(
            !is_public_key("hSDwCYkwp1R0i33ctD73Wg2_Og0mOBr066SpjqqbTmp"),
            "лишние биты в последнем знаке"
        );
        assert!(!is_public_key("hSDwCYkwp1R0i33ctD73Wg2_Og0mOBr066SpjqqbTm"));
        assert!(!is_public_key(
            "hSDwCYkwp1R0i33ctD73Wg2/Og0mOBr066SpjqqbTmo"
        ));
        assert!(!is_public_key(""));
    }
}
