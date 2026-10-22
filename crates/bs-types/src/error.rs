//! Ошибки API и их коды.
//!
//! Сервер возвращает `{ error: string, code?: ErrorCode, details?: object }`.
//! Мы сохраняем `code` и `details` as-is, чтобы клиент мог реагировать
//! по коду, а не по тексту сообщения.

use serde::{Deserialize, Serialize};

/// Тело ошибки API, как его вернул сервер.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    /// Человекочитаемое сообщение.
    pub error: String,
    /// Машинный код (может отсутствовать).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub code: Option<String>,
    /// Дополнительные данные.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub details: Option<serde_json::Value>,
}

/// Известные коды ошибок Backspace. Не полный список — пополняется по мере
/// обнаружения в серверных роутах.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    Unauthorized,
    Forbidden,
    NotFound,
    BadRequest,
    Conflict,
    RateLimited,
    JoinRequestRequired,
    DirectoryPrivateSpace,
    SpaceOwnerOnly,
    RolePermissionsConflict,
    OverridesConflict,
    InstanceUnreachable,
    FederatedRegistrationClosed,
    RegistrationClosed,
}

impl ErrorCode {
    /// Разбор строки из `ApiErrorBody::code`.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "unauthorized" => Self::Unauthorized,
            "forbidden" => Self::Forbidden,
            "not_found" => Self::NotFound,
            "bad_request" => Self::BadRequest,
            "conflict" => Self::Conflict,
            "rate_limited" => Self::RateLimited,
            "join_request_required" => Self::JoinRequestRequired,
            "directory_private_space" => Self::DirectoryPrivateSpace,
            "space_owner_only" => Self::SpaceOwnerOnly,
            "role_permissions_conflict" => Self::RolePermissionsConflict,
            "overrides_conflict" => Self::OverridesConflict,
            "instance_unreachable" => Self::InstanceUnreachable,
            "federated_registration_closed" => Self::FederatedRegistrationClosed,
            "registration_closed" => Self::RegistrationClosed,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_body() {
        let body: ApiErrorBody = serde_json::from_str(r#"{"error":"nope"}"#).unwrap();
        assert_eq!(body.error, "nope");
        assert!(body.code.is_none());
        assert!(body.details.is_none());
    }

    #[test]
    fn parses_full_body() {
        let body: ApiErrorBody = serde_json::from_str(
            r#"{"error":"go away","code":"join_request_required","details":{"spaceId":"42"}}"#,
        )
        .unwrap();
        assert_eq!(body.code.as_deref(), Some("join_request_required"));
        assert_eq!(
            body.details.as_ref().unwrap().get("spaceId").unwrap(),
            &serde_json::Value::String("42".into())
        );
    }

    #[test]
    fn parses_known_code() {
        assert_eq!(
            ErrorCode::parse("rate_limited"),
            Some(ErrorCode::RateLimited)
        );
        assert_eq!(ErrorCode::parse("what"), None);
    }
}
