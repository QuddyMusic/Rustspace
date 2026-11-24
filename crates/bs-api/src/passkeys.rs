//! Ключи доступа (passkey) Rustspace: билеты для браузера и список ключей.

use serde::Deserialize;

use crate::client::ApiClient;
use crate::error::ApiError;

/// Билет: страница для браузера и код, который должен совпасть.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PasskeyTicket {
    /// Билет (опрашивается до готовности).
    pub ticket: String,
    /// Код из шести цифр — тот же на странице.
    pub code: String,
    /// Страница инстанса для браузера.
    pub url: String,
}

/// Состояние билета.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasskeyStatus {
    /// Ждём браузер.
    Pending,
    /// Ключ добавлен или вход выполнен (`token` — при входе).
    Done {
        /// Токен сессии (только при входе).
        token: Option<String>,
    },
    /// Не получилось.
    Failed(String),
}

/// Ключ доступа аккаунта.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Passkey {
    /// Id ключа.
    pub id: String,
    /// Где создан (платформа браузера).
    #[serde(default)]
    pub name: String,
    /// Когда добавлен (мс).
    pub created_at: i64,
    /// Когда последний раз входили (мс).
    #[serde(default)]
    pub last_used_at: Option<i64>,
}

impl ApiClient {
    /// Билет на вход по ключу (`POST /api/rustspace/passkeys/login`).
    ///
    /// # Errors
    /// Сервер без ключей доступа (404/409) или ошибка сети.
    pub async fn passkey_login_ticket(&self) -> Result<PasskeyTicket, ApiError> {
        self.post_json("/api/rustspace/passkeys/login", &serde_json::json!({}))
            .await
    }

    /// Билет на добавление ключа к своему аккаунту.
    ///
    /// # Errors
    /// Сервер без ключей доступа или ошибка сети.
    pub async fn passkey_register_ticket(&self) -> Result<PasskeyTicket, ApiError> {
        self.post_json("/api/rustspace/passkeys/register", &serde_json::json!({}))
            .await
    }

    /// Состояние билета.
    ///
    /// # Errors
    /// Билет устарел (404) или ошибка сети.
    pub async fn passkey_status(&self, ticket: &str) -> Result<PasskeyStatus, ApiError> {
        #[derive(Deserialize)]
        struct Raw {
            status: String,
            #[serde(default)]
            token: Option<String>,
            #[serde(default)]
            error: Option<String>,
        }
        let raw: Raw = self
            .get_json(&format!("/api/rustspace/passkeys/status/{ticket}"))
            .await?;
        Ok(match raw.status.as_str() {
            "done" => PasskeyStatus::Done { token: raw.token },
            "failed" => PasskeyStatus::Failed(raw.error.unwrap_or_default()),
            _ => PasskeyStatus::Pending,
        })
    }

    /// Свои ключи доступа.
    ///
    /// # Errors
    /// Сервер без ключей доступа или ошибка сети.
    pub async fn passkeys(&self) -> Result<Vec<Passkey>, ApiError> {
        self.get_json("/api/rustspace/passkeys").await
    }

    /// Удаляет свой ключ доступа.
    ///
    /// # Errors
    /// Ошибка сети.
    pub async fn delete_passkey(&self, id: &str) -> Result<(), ApiError> {
        let _: serde_json::Value = self
            .delete_json(&format!("/api/rustspace/passkeys/{id}"))
            .await?;
        Ok(())
    }
}
