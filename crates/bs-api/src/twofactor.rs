//! Двухфакторная аутентификация (`/api/auth/2fa`) — расширение `bs-server`.
//!
//! Оригинальный сервер этих маршрутов не знает и отвечает `404`: клиент
//! показывает, что защита на этом инстансе недоступна.

use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::error::ApiError;

/// Состояние защиты аккаунта.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TwoFactorStatus {
    /// Защита включена.
    pub enabled: bool,
    /// Настройка начата, но не подтверждена кодом.
    #[serde(default)]
    pub pending: bool,
    /// Сколько запасных кодов осталось.
    #[serde(default)]
    pub recovery_codes_left: u32,
}

/// Секрет для приложения-аутентификатора.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TwoFactorSetup {
    /// Секрет в base32 — для ручного ввода.
    pub secret: String,
    /// Ссылка `otpauth://` — для QR-кода.
    pub otpauth_url: String,
}

impl std::fmt::Debug for TwoFactorSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TwoFactorSetup(<redacted>)")
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Codes {
    recovery_codes: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Confirm<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery_code: Option<&'a str>,
}

impl<'a> Confirm<'a> {
    fn new(password: Option<&'a str>, factor: &'a str) -> Self {
        let factor = factor.trim();
        let is_totp = factor.len() == 6 && factor.bytes().all(|b| b.is_ascii_digit());
        Self {
            password,
            code: is_totp.then_some(factor),
            recovery_code: (!is_totp && !factor.is_empty()).then_some(factor),
        }
    }
}

impl ApiClient {
    /// `GET /api/auth/2fa`.
    ///
    /// # Errors
    /// Сетевые ошибки; `404` — сервер не поддерживает защиту.
    pub async fn two_factor_status(&self) -> Result<TwoFactorStatus, ApiError> {
        self.get_json("/api/auth/2fa").await
    }

    /// `POST /api/auth/2fa/setup`: новый секрет (защита ещё не включена).
    ///
    /// # Errors
    /// Сетевые ошибки и отказ сервера (неверный пароль, уже включено).
    pub async fn two_factor_setup(&self, password: &str) -> Result<TwoFactorSetup, ApiError> {
        self.post_json(
            "/api/auth/2fa/setup",
            &Confirm {
                password: Some(password),
                code: None,
                recovery_code: None,
            },
        )
        .await
    }

    /// `POST /api/auth/2fa/enable`: подтверждает настройку кодом и
    /// возвращает запасные коды.
    ///
    /// # Errors
    /// Сетевые ошибки и отказ сервера (неверный код).
    pub async fn two_factor_enable(&self, code: &str) -> Result<Vec<String>, ApiError> {
        let codes: Codes = self
            .post_json("/api/auth/2fa/enable", &Confirm::new(None, code))
            .await?;
        Ok(codes.recovery_codes)
    }

    /// `POST /api/auth/2fa/disable`: снимает защиту (пароль и код).
    ///
    /// # Errors
    /// Сетевые ошибки и отказ сервера.
    pub async fn two_factor_disable(&self, password: &str, factor: &str) -> Result<(), ApiError> {
        let _: serde_json::Value = self
            .post_json(
                "/api/auth/2fa/disable",
                &Confirm::new(Some(password), factor),
            )
            .await?;
        Ok(())
    }

    /// `POST /api/auth/2fa/recovery-codes`: новый набор запасных кодов.
    ///
    /// # Errors
    /// Сетевые ошибки и отказ сервера.
    pub async fn two_factor_recovery_codes(
        &self,
        password: &str,
        code: &str,
    ) -> Result<Vec<String>, ApiError> {
        let codes: Codes = self
            .post_json(
                "/api/auth/2fa/recovery-codes",
                &Confirm::new(Some(password), code),
            )
            .await?;
        Ok(codes.recovery_codes)
    }
}

/// Открытый ключ сквозного шифрования из справочника сервера.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct E2eeKey {
    /// Чей ключ.
    pub user_id: String,
    /// Ключ в base64url; `None` — не опубликован.
    pub public_key: Option<String>,
}

impl ApiClient {
    /// `PUT /api/e2ee/key`: публикует свой открытый ключ в справочнике
    /// сервера (расширение `bs-server`; оригинал отвечает `404`).
    ///
    /// # Errors
    /// Сетевые ошибки и отказ сервера.
    pub async fn publish_e2ee_key(&self, public_key: &str) -> Result<E2eeKey, ApiError> {
        self.put_json(
            "/api/e2ee/key",
            &serde_json::json!({ "publicKey": public_key }),
        )
        .await
    }

    /// `GET /api/e2ee/keys/:userId`: открытый ключ пользователя.
    ///
    /// # Errors
    /// Сетевые ошибки; `404` — ключа нет или сервер не ведёт справочник.
    pub async fn e2ee_key_of(&self, user_id: &str) -> Result<E2eeKey, ApiError> {
        let id: String = user_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        self.get_json(&format!("/api/e2ee/keys/{id}")).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::LoginRequest;

    #[test]
    fn second_factor_goes_into_the_right_field() {
        let totp =
            serde_json::to_value(LoginRequest::new("u", "p", Some(" 123456 "))).unwrap_or_default();
        assert_eq!(
            totp,
            serde_json::json!({"username": "u", "password": "p", "totp": "123456"})
        );
        let recovery = serde_json::to_value(LoginRequest::new("u", "p", Some("abcd-efgh-jkmn")))
            .unwrap_or_default();
        assert_eq!(
            recovery,
            serde_json::json!({"username": "u", "password": "p", "recoveryCode": "abcd-efgh-jkmn"})
        );
        let plain =
            serde_json::to_value(LoginRequest::new("u", "p", Some("  "))).unwrap_or_default();
        assert_eq!(plain, serde_json::json!({"username": "u", "password": "p"}));
        let confirm =
            serde_json::to_value(Confirm::new(Some("p"), "abcd-efgh-jkmn")).unwrap_or_default();
        assert_eq!(
            confirm,
            serde_json::json!({"password": "p", "recoveryCode": "abcd-efgh-jkmn"})
        );
    }

    #[test]
    fn status_parses_with_and_without_optional_fields() {
        let full: TwoFactorStatus = serde_json::from_str(
            r#"{"enabled":true,"pending":false,"enabledAt":1,"recoveryCodesLeft":9}"#,
        )
        .unwrap_or_default();
        assert_eq!(
            full,
            TwoFactorStatus {
                enabled: true,
                pending: false,
                recovery_codes_left: 9
            }
        );
        let short: TwoFactorStatus =
            serde_json::from_str(r#"{"enabled":false}"#).unwrap_or_default();
        assert_eq!(short, TwoFactorStatus::default());
    }
}
