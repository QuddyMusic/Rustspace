//! Серверный реестр подключённых инстансов и удаление федеративной учётки.
//!
//! Порт `getFederationRegistry` / `putFederationRegistry` /
//! `deleteFederationIdentity` из `client.ts`. Реестр — источник списка
//! «Подключения»: домашний инстанс хранит его между запусками и устройствами.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::error::ApiError;

/// Состояние подключения в реестре.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryStatus {
    /// Сессия работает.
    Connected,
    /// Пользователь отключил инстанс сам: автоподключения нет.
    Disconnected,
    /// Инстанс не отвечал.
    Unreachable,
    /// Нужен вход заново.
    AuthExpired,
}

/// Запись реестра.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryEntry {
    /// Адрес инстанса (`https://host`).
    pub origin: String,
    /// Название для показа.
    #[serde(default)]
    pub label: String,
    /// Имя пользователя на инстансе.
    #[serde(default)]
    pub username: String,
    /// Идентификатор пользователя на инстансе.
    #[serde(default)]
    pub remote_user_id: String,
    /// Состояние.
    pub status: RegistryStatus,
    /// Когда добавлен, мс с начала эпохи.
    #[serde(default)]
    pub added_at: i64,
    /// Когда подключались в последний раз.
    #[serde(default)]
    pub last_connected_at: Option<i64>,
    /// Когда отключили вручную.
    #[serde(default)]
    pub disconnected_at: Option<i64>,
    /// Код причины (`unreachable`, `session_expired`, …) или текст старого клиента.
    #[serde(default)]
    pub error_message: Option<String>,
}

/// Ответ `GET /api/users/@me/federation-registry`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryResponse {
    /// Записи.
    #[serde(default)]
    pub registry: Vec<RegistryEntry>,
    /// Метка последней записи (LWW).
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RegistryBody<'a> {
    registry: &'a [RegistryEntry],
    updated_at: i64,
}

/// Что делать с учёткой на удалённом инстансе.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityDeleteMode {
    /// Тихо уйти: учётка и данные остаются.
    Leave,
    /// Удалить пользователя, сообщения остаются.
    Soft,
    /// Удалить всё.
    Full,
}

#[derive(Serialize)]
struct DeleteBody<'a> {
    origins: &'a [String],
    mode: IdentityDeleteMode,
}

/// Итог удаления на одном инстансе.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDeleteResult {
    /// Удалено ли.
    pub success: bool,
    /// Код или текст ошибки.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct DeleteResponse {
    #[serde(default)]
    results: HashMap<String, IdentityDeleteResult>,
}

impl ApiClient {
    /// `GET /api/users/@me/federation-registry`.
    ///
    /// # Errors
    /// Сетевые ошибки и отказ сервера.
    pub async fn federation_registry(&self) -> Result<RegistryResponse, ApiError> {
        self.get_json("/api/users/@me/federation-registry").await
    }

    /// `PUT /api/users/@me/federation-registry` — заменяет реестр (LWW).
    ///
    /// # Errors
    /// `409 registry_conflict`, если на сервере метка новее.
    pub async fn put_federation_registry(
        &self,
        entries: &[RegistryEntry],
        updated_at: i64,
    ) -> Result<(), ApiError> {
        let _: serde_json::Value = self
            .put_json(
                "/api/users/@me/federation-registry",
                &RegistryBody {
                    registry: entries,
                    updated_at,
                },
            )
            .await?;
        Ok(())
    }

    /// `POST /api/users/@me/federation-identity/delete`.
    ///
    /// # Errors
    /// Сетевые ошибки и отказ сервера; ошибки по отдельным инстансам — в
    /// результатах.
    pub async fn delete_federation_identity(
        &self,
        origins: &[String],
        mode: IdentityDeleteMode,
    ) -> Result<HashMap<String, IdentityDeleteResult>, ApiError> {
        let response: DeleteResponse = self
            .post_json(
                "/api/users/@me/federation-identity/delete",
                &DeleteBody { origins, mode },
            )
            .await?;
        Ok(response.results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_reads_server_shape() {
        let raw = r#"{"origin":"https://a.example","label":"A","username":"q@h","remoteUserId":"7",
            "status":"auth_expired","addedAt":5,"lastConnectedAt":null,"disconnectedAt":null,
            "errorMessage":"session_expired"}"#;
        let entry: RegistryEntry = serde_json::from_str(raw).expect("parse");
        assert_eq!(entry.status, RegistryStatus::AuthExpired);
        assert_eq!(entry.remote_user_id, "7");
    }

    #[test]
    fn body_uses_camel_case_and_snake_status() {
        let entry = RegistryEntry {
            origin: "https://a.example".into(),
            label: String::new(),
            username: String::new(),
            remote_user_id: String::new(),
            status: RegistryStatus::Disconnected,
            added_at: 1,
            last_connected_at: None,
            disconnected_at: Some(2),
            error_message: None,
        };
        let body = serde_json::to_value(RegistryBody {
            registry: std::slice::from_ref(&entry),
            updated_at: 9,
        })
        .expect("ser");
        assert_eq!(body["updatedAt"], 9);
        assert_eq!(body["registry"][0]["status"], "disconnected");
        assert_eq!(body["registry"][0]["disconnectedAt"], 2);
    }
}
