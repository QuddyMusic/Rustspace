//! Федеративные эндпоинты: креденшел на удалённый инстанс и re-attach.
//!
//! Все методы требуют Bearer-токена инстанса, который выдаёт документ:
//! креденшел выдаёт **домашний** инстанс, `attach_proof` — тоже домашний,
//! а `reattach` вызывается на том инстансе, где живёт отсоединённый аккаунт.
//!
//! # Пример
//! ```no_run
//! # async fn run() -> Result<(), bs_api::ApiError> {
//! use bs_api::{ApiClient, FederationCredentialRequest};
//!
//! let mut home = ApiClient::home("https://home.example.com")?;
//! home.set_token(Some("jwt".to_owned()));
//! let cred = home
//!     .federation_credential(&FederationCredentialRequest::new("https://orbit.example.com"))
//!     .await?;
//! println!("secret={} provisioned={}", cred.secret, cred.provisioned);
//! # Ok(())
//! # }
//! ```

use bs_types::User;
use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::error::ApiError;

/// Тело `POST /api/users/@me/federation-credential`.
///
/// Запрос идемпотентен: повторный вызов возвращает тот же `secret`
/// (сервер вставляет строку с `onConflictDoNothing` и перечитывает её),
/// поэтому гонка двух клиентов не приводит к двум разным секретам.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FederationCredentialRequest {
    /// Origin удалённого инстанса, например `https://orbit.example.com`.
    pub origin: String,
    /// Отметить, что удалённый аккаунт уже переведён на этот секрет.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mark_provisioned: Option<bool>,
}

impl FederationCredentialRequest {
    /// Запрос секрета без отметки о провижининге.
    #[must_use]
    pub fn new(origin: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            mark_provisioned: None,
        }
    }

    /// Тот же запрос, но с отметкой `markProvisioned`.
    #[must_use]
    pub fn marking_provisioned(origin: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            mark_provisioned: Some(true),
        }
    }
}

/// Ответ `POST /api/users/@me/federation-credential`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FederationCredentialResponse {
    /// Origin, для которого выдан секрет (канонизированный сервером).
    pub origin: String,
    /// Секрет, которым удалённый аккаунт этого пользователя аутентифицируется.
    pub secret: String,
    /// `true`, если сервер уже знает, что удалённый аккаунт использует `secret`.
    pub provisioned: bool,
}

/// Тело `POST /api/users/@me/change-password`.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    /// Текущий пароль; на домашнем инстансе обязателен, для федеративного
    /// аккаунта можно не передавать.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_password: Option<String>,
    /// Новый пароль (для удалённого аккаунта — выданный домашним секрет).
    pub new_password: String,
}

impl ChangePasswordRequest {
    /// Смена пароля без указания текущего (федеративный аккаунт).
    #[must_use]
    pub fn new(new_password: impl Into<String>) -> Self {
        Self {
            current_password: None,
            new_password: new_password.into(),
        }
    }

    /// Смена пароля с подтверждением текущего (домашний аккаунт).
    #[must_use]
    pub fn with_current(
        current_password: impl Into<String>,
        new_password: impl Into<String>,
    ) -> Self {
        Self {
            current_password: Some(current_password.into()),
            new_password: new_password.into(),
        }
    }
}

/// Ответ `POST /api/users/@me/change-password`.
///
/// Сервер отзывает прочие сессии аккаунта, поэтому возвращённый токен
/// нужно немедленно применить к клиенту удалённого инстанса.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordResponse {
    /// Свежий JWT вместо отозванного.
    pub token: String,
}

/// Ответ `POST /api/auth/attach-proof`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachProofResponse {
    /// Одноразовый токен (64 hex-символа), привязанный к целевому домену.
    pub token: String,
}

/// Тело `POST /api/users/@me/reattach`.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ReattachRequest {
    /// Токен из [`AttachProofResponse`].
    pub token: String,
}

/// Ответ `POST /api/users/@me/reattach` — перепривязанное self-view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReattachResponse {
    /// Всегда `true`: сервер отвечает этим полем только при успехе.
    pub success: bool,
    /// Профиль аккаунта после перепривязки (новое имя и id).
    pub user: User,
}

impl ApiClient {
    /// `POST /api/users/@me/federation-credential` — выдаёт (или перечитывает)
    /// секрет, которым этот пользователь входит на `origin`.
    ///
    /// Вызывать нужно на **домашнем** инстансе аккаунта: удалённый инстанс
    /// ответит `409 federation_credential_home_only`, а собственный origin —
    /// `400 federation_credential_remote_only`.
    ///
    /// # Ошибки
    /// - `400 origin_required` / `origin_invalid` — пустой или неразбираемый origin;
    /// - `400 federation_credential_remote_only` — передан свой же origin;
    /// - `401 account_deleted` — аккаунт удалён;
    /// - `409 federation_credential_home_only` — вызвано не на домашнем инстансе.
    pub async fn federation_credential(
        &self,
        request: &FederationCredentialRequest,
    ) -> Result<FederationCredentialResponse, ApiError> {
        self.post_json("/api/users/@me/federation-credential", request)
            .await
    }

    /// `POST /api/users/@me/change-password` — меняет пароль аккаунта и
    /// возвращает новый токен (прежние сессии отзываются).
    ///
    /// На удалённом инстансе так устанавливается выданный домашним
    /// инстансом секрет: `new_password` = `secret`.
    pub async fn change_password(
        &self,
        request: &ChangePasswordRequest,
    ) -> Result<ChangePasswordResponse, ApiError> {
        self.post_json("/api/users/@me/change-password", request)
            .await
    }

    /// `POST /api/auth/attach-proof` — минтит одноразовый токен для
    /// перепривязки отсоединённого аккаунта к `target_domain`.
    ///
    /// Вызывается на домашнем инстансе под сессией нативного аккаунта.
    /// `target_domain` — имя хоста **без схемы и порта**: сервер сверяет его
    /// с доменом аутентифицированного пира.
    pub async fn attach_proof(&self, target_domain: &str) -> Result<AttachProofResponse, ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            target_domain: &'a str,
        }
        self.post_json("/api/auth/attach-proof", &Body { target_domain })
            .await
    }

    /// `POST /api/users/@me/reattach` — перепривязывает отсоединённый аккаунт
    /// к его прежнему дому по одноразовому токену.
    ///
    /// Вызывается на инстансе, где аккаунт отсоединён, под его сессией.
    /// Токен одноразовый и привязан к домену вызывающего пира.
    ///
    /// # Ошибки
    /// - `400` — токен не 64-символьный hex;
    /// - `403` — аккаунт не отсоединён (перепривязка бессмысленна);
    /// - `404` — аккаунт не найден или удалён.
    pub async fn reattach(&self, token: &str) -> Result<ReattachResponse, ApiError> {
        let body = ReattachRequest {
            token: token.to_owned(),
        };
        self.post_json("/api/users/@me/reattach", &body).await
    }
}

// Секреты в `Debug` не печатаются: структуры попадают в `tracing`/`dbg!`.

impl std::fmt::Debug for FederationCredentialResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FederationCredentialResponse")
            .field("origin", &self.origin)
            .field("secret", &"<redacted>")
            .field("provisioned", &self.provisioned)
            .finish()
    }
}

impl std::fmt::Debug for ChangePasswordRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChangePasswordRequest")
            .field("current_password", &"<redacted>")
            .field("new_password", &"<redacted>")
            .finish()
    }
}

impl std::fmt::Debug for ChangePasswordResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChangePasswordResponse")
            .field("token", &"<redacted>")
            .finish()
    }
}

impl std::fmt::Debug for AttachProofResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachProofResponse")
            .field("token", &"<redacted>")
            .finish()
    }
}

impl std::fmt::Debug for ReattachRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReattachRequest")
            .field("token", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn credential_request_omits_flag_by_default() {
        let json = serde_json::to_value(FederationCredentialRequest::new(
            "https://orbit.example.com",
        ))
        .unwrap();
        assert_eq!(json["origin"], "https://orbit.example.com");
        assert!(json.get("markProvisioned").is_none());
    }

    #[test]
    fn credential_request_marks_provisioned() {
        let json = serde_json::to_value(FederationCredentialRequest::marking_provisioned(
            "https://orbit.example.com",
        ))
        .unwrap();
        assert_eq!(json["markProvisioned"], true);
    }

    #[test]
    fn credential_response_parses_camel_case() {
        let raw = r#"{"origin":"https://orbit.example.com","secret":"s3cr3t","provisioned":true}"#;
        let parsed: FederationCredentialResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.origin, "https://orbit.example.com");
        assert_eq!(parsed.secret, "s3cr3t");
        assert!(parsed.provisioned);
    }

    #[test]
    fn change_password_request_omits_current_when_absent() {
        let json = serde_json::to_value(ChangePasswordRequest::new("new-secret")).unwrap();
        assert_eq!(json["newPassword"], "new-secret");
        assert!(json.get("currentPassword").is_none());
    }

    #[test]
    fn change_password_request_keeps_current_when_set() {
        let json = serde_json::to_value(ChangePasswordRequest::with_current("old", "new")).unwrap();
        assert_eq!(json["currentPassword"], "old");
        assert_eq!(json["newPassword"], "new");
    }

    #[test]
    fn change_password_response_parses_token() {
        let parsed: ChangePasswordResponse = serde_json::from_str(r#"{"token":"jwt"}"#).unwrap();
        assert_eq!(parsed.token, "jwt");
    }

    #[test]
    fn attach_proof_response_parses_token() {
        let parsed: AttachProofResponse = serde_json::from_str(r#"{"token":"abc"}"#).unwrap();
        assert_eq!(parsed.token, "abc");
    }

    #[test]
    fn reattach_request_serializes_token() {
        let json = serde_json::to_value(ReattachRequest {
            token: "deadbeef".to_owned(),
        })
        .unwrap();
        assert_eq!(json["token"], "deadbeef");
    }

    #[test]
    fn reattach_response_parses_rebound_user() {
        let raw = r#"{"success":true,"user":{"id":"1","username":"alice@orbit.ddns.net","displayName":null,"avatar":null,"banner":null,"accentColor":null,"avatarColor":null,"bio":null,"status":"online","customStatus":null,"isAdmin":false,"createdAt":1750000000000,"homeInstance":"https://orbit.ddns.net","homeUserId":"9","replicatedInstances":[]}}"#;
        let parsed: ReattachResponse = serde_json::from_str(raw).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.user.username, "alice@orbit.ddns.net");
        assert_eq!(parsed.user.home_user_id.as_deref(), Some("9"));
    }

    #[test]
    fn reattach_response_rejects_missing_user() {
        assert!(serde_json::from_str::<ReattachResponse>(r#"{"success":true}"#).is_err());
    }
}
