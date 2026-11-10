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
#[allow(
    clippy::struct_excessive_bools,
    reason = "повторяет JSON сервера: флаги независимы"
)]
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
    /// Постоянный идентификатор инстанса (эпоха федерации).
    #[serde(default)]
    pub instance_id: Option<String>,
    /// Настроен ли у инстанса каталог пространств (хаб).
    #[serde(default)]
    pub directory_configured: bool,
    /// Можно ли просматривать каталог с этого инстанса.
    #[serde(default)]
    pub directory_available: bool,
    /// Включил ли администратор публикацию в каталоге.
    #[serde(default)]
    pub directory_enabled: bool,
    /// Показывать ли карточку поддержки проекта.
    #[serde(default = "default_true")]
    pub support_card_enabled: bool,
}

const fn default_true() -> bool {
    true
}

/// Тело `POST /api/auth/register`.
///
/// Без `home_instance` это обычная регистрация на этом инстансе; с ним —
/// федеративный аккаунт, выданный домашним инстансом. `Debug` не печатает
/// пароль и токен приглашения.
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    /// Имя: `alice` для обычной регистрации, `alice@home.example` для
    /// федеративной.
    pub username: String,
    /// Пароль (для федеративной — секрет, выданный домашним инстансом).
    pub password: String,
    /// Отображаемое имя.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Цвет аватара из палитры (`mint`, `sky`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_color: Option<String>,
    /// Токен ссылки-приглашения, когда открытая регистрация выключена.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_token: Option<String>,
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
            .field(
                "invite_token",
                &self.invite_token.as_ref().map(|_| "<redacted>"),
            )
            .finish_non_exhaustive()
    }
}

/// Результат `GET /api/auth/check-username`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsernameCheck {
    /// Имя свободно.
    Available,
    /// Имя уже занято.
    Taken,
    /// Имя не проходит правила сервера; `code` — машинный код причины.
    Invalid {
        /// Например, `username_length_invalid`.
        code: String,
    },
    /// Регистрация на инстансе закрыта.
    RegistrationClosed,
}

/// Результат `GET /api/auth/check-invite`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteCheck {
    /// Приглашение действует; `name` — его название.
    Valid {
        /// Название ссылки, которое задал администратор.
        name: String,
    },
    /// Срок действия истёк.
    Expired,
    /// Использования закончились.
    Exhausted,
    /// Ссылки нет, она отозвана или токен испорчен.
    Invalid,
}

#[derive(Deserialize)]
struct UsernameCheckBody {
    available: bool,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Deserialize)]
struct InviteCheckBody {
    valid: bool,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    reason: Option<String>,
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

    /// `GET /api/auth/check-username` — свободно ли имя для регистрации.
    ///
    /// Сервер отвечает `403`, если регистрация закрыта, и `400` на пустое
    /// имя: оба случая возвращаются как значения, а не ошибки.
    ///
    /// # Errors
    /// Сетевые ошибки и неожиданные ответы.
    pub async fn check_username(&self, username: &str) -> Result<UsernameCheck, ApiError> {
        let path = {
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            ser.append_pair("username", username);
            format!("/api/auth/check-username?{}", ser.finish())
        };
        match self.get_json::<UsernameCheckBody>(&path).await {
            Ok(body) if body.available => Ok(UsernameCheck::Available),
            Ok(body) => Ok(body
                .code
                .map_or(UsernameCheck::Taken, |code| UsernameCheck::Invalid { code })),
            Err(err) if err.status() == Some(reqwest::StatusCode::FORBIDDEN) => {
                Ok(UsernameCheck::RegistrationClosed)
            }
            Err(err) if err.status() == Some(reqwest::StatusCode::BAD_REQUEST) => {
                Ok(UsernameCheck::Invalid {
                    code: "username_required".to_owned(),
                })
            }
            Err(err) => Err(err),
        }
    }

    /// `GET /api/auth/check-invite` — действует ли токен приглашения.
    ///
    /// # Errors
    /// Сетевые ошибки и неожиданные ответы.
    pub async fn check_invite(&self, token: &str) -> Result<InviteCheck, ApiError> {
        let path = {
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            ser.append_pair("token", token);
            format!("/api/auth/check-invite?{}", ser.finish())
        };
        let body: InviteCheckBody = self.get_json(&path).await?;
        Ok(if body.valid {
            InviteCheck::Valid {
                name: body.name.unwrap_or_default(),
            }
        } else {
            match body.reason.as_deref() {
                Some("expired") => InviteCheck::Expired,
                Some("exhausted") => InviteCheck::Exhausted,
                _ => InviteCheck::Invalid,
            }
        })
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
