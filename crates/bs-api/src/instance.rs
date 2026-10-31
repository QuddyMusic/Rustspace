//! Подключение к другому инстансу: проба, регистрация федеративного
//! аккаунта, проверка пароля и запрос пиринга.
//!
//! Порт `connectToRemote` / `peerHomeWithRemote` из `instanceStore.ts`.

use bs_types::User;
use serde::{Deserialize, Serialize};

use crate::auth::AuthResponse;
use crate::client::ApiClient;
use crate::error::ApiError;

/// Ответ `GET /api/instance/info` (без авторизации).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceInfo {
    /// Название инстанса.
    pub name: String,
    /// Версия сервера.
    #[serde(default)]
    pub version: String,
    /// Открыта ли обычная регистрация.
    #[serde(default)]
    pub registration_open: bool,
    /// Открыта ли регистрация федеративных аккаунтов; при `false` войти
    /// может только тот, у кого аккаунт там уже есть.
    #[serde(default = "default_true")]
    pub federated_registration_open: bool,
    /// Ссылка на исходный код (AGPL § 13).
    #[serde(default)]
    pub source_code_url: Option<String>,
    /// Короткий хеш сборки.
    #[serde(default)]
    pub commit: Option<String>,
}

const fn default_true() -> bool {
    true
}

/// Тело `POST /api/auth/register` для федеративного аккаунта.
///
/// `Debug` не печатает пароль.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    /// Имя вида `alice@home.example`.
    pub username: String,
    /// Секрет, выданный домашним инстансом.
    pub password: String,
    /// Отображаемое имя.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Хост домашнего инстанса.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_instance: Option<String>,
    /// Идентификатор пользователя на домашнем инстансе.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_user_id: Option<String>,
}

impl std::fmt::Debug for RegisterRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisterRequest")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("home_instance", &self.home_instance)
            .finish_non_exhaustive()
    }
}

/// Результат проверки пароля.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct VerifyPasswordResponse {
    /// Совпал ли пароль.
    pub valid: bool,
}

/// Ответ `POST /api/federation/peer/ensure`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerEnsureResponse {
    /// `active`, `pending`, `awaiting_approval`, `admin_required`,
    /// `rejected`, `unreachable` или `revoked`.
    pub peering_status: String,
}

/// Причина запроса пиринга.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerEnsureBody<'a> {
    remote_origin: &'a str,
    reason: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifyBody<'a> {
    password: &'a str,
}

/// Запись для `replicatedInstances` при синхронизации списка инстансов.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplicatedEntry {
    /// Origin инстанса.
    pub origin: String,
    /// Имя пользователя на нём.
    pub username: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplicatedBody<'a> {
    replicated_instances: &'a [ReplicatedEntry],
}

impl ApiClient {
    /// `GET /api/instance/info` — публичные сведения об инстансе.
    ///
    /// # Errors
    /// Сетевые ошибки и некорректный ответ.
    pub async fn instance_info(&self) -> Result<InstanceInfo, ApiError> {
        self.get_json("/api/instance/info").await
    }

    /// `POST /api/auth/register` — создаёт федеративный аккаунт.
    ///
    /// # Errors
    /// `409` — имя занято, `403` — регистрация закрыта.
    pub async fn register(&self, request: &RegisterRequest) -> Result<AuthResponse, ApiError> {
        self.post_json("/api/auth/register", request).await
    }

    /// `POST /api/users/@me/verify-password` — проверяет пароль аккаунта.
    ///
    /// # Errors
    /// Сетевые ошибки; неверный пароль — это `valid == false`, не ошибка.
    pub async fn verify_password(&self, password: &str) -> Result<bool, ApiError> {
        let response: VerifyPasswordResponse = self
            .post_json("/api/users/@me/verify-password", &VerifyBody { password })
            .await?;
        Ok(response.valid)
    }

    /// `POST /api/federation/peer/ensure` — просит **этот** (домашний)
    /// инстанс установить пиринг с `remote_origin`.
    ///
    /// # Errors
    /// Сетевые ошибки и отказ сервера (лимит, нет прав).
    pub async fn ensure_peered(&self, remote_origin: &str) -> Result<PeerEnsureResponse, ApiError> {
        self.post_json(
            "/api/federation/peer/ensure",
            &PeerEnsureBody {
                remote_origin,
                reason: "instance_connect",
            },
        )
        .await
    }

    /// `PATCH /api/users/@me` со списком связанных инстансов.
    ///
    /// # Errors
    /// Сетевые ошибки и отказ сервера.
    pub async fn set_replicated_instances(
        &self,
        entries: &[ReplicatedEntry],
    ) -> Result<User, ApiError> {
        self.patch_json(
            "/api/users/@me",
            &ReplicatedBody {
                replicated_instances: entries,
            },
        )
        .await
    }
}
