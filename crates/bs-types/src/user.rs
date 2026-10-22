//! Пользователь и связанные типы.
//!
//! Повторяет `User`, `ReplicatedInstance`, `UserStatus`, `ChosenUserStatus`
//! из `packages/shared/src/types.ts` (Backspace). Все поля, кроме помеченных
//! `Option`, сервер отдаёт всегда.

use serde::{Deserialize, Serialize};

use crate::id::UserId;

/// Палитра цветов аватара, которую понимает веб-клиент.
///
/// В TS это union строк; здесь — алиас, чтобы новый цвет от сервера не ломал
/// парсинг. Известные значения — в [`AVATAR_COLORS`](crate::AVATAR_COLORS).
pub type AvatarColor = String;

/// Пользователь Backspace.
///
/// `federationHomeOrphaned` и `detachedHomeInstance` приходят только в
/// self-view (собственный профиль), поэтому опциональны.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    pub accent_color: Option<String>,
    pub avatar_color: Option<AvatarColor>,
    pub bio: Option<String>,
    pub status: UserStatus,
    pub custom_status: Option<String>,
    pub is_admin: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_deleted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discoverable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_updated_at: Option<i64>,
    /// Unix-время в миллисекундах (как у сервера), не RFC 3339.
    pub created_at: i64,
    pub home_instance: Option<String>,
    pub home_user_id: Option<String>,
    #[serde(default)]
    pub replicated_instances: Vec<ReplicatedInstance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_activity: Option<bool>,
    /// Self-view: домашний инстанс потерян, аккаунт стал суверенным локальным.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub federation_home_orphaned: Option<bool>,
    /// Self-view: прежний домашний инстанс отсоединённого аккаунта.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached_home_instance: Option<String>,
}

/// Запись о репликации аккаунта на другом инстансе.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplicatedInstance {
    /// Полный origin с протоколом, например `https://orbit.ddns.net`.
    pub origin: String,
    /// Имя пользователя на этом инстансе.
    pub username: String,
    /// Устаревшее поле, оставлено для совместимости.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

/// Текущий статус присутствия, как его видит сервер.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserStatus {
    Online,
    Idle,
    #[serde(rename = "dnd")]
    DoNotDisturb,
    Offline,
}

/// Статус, который пользователь выбирает сам. `offline` выбрать нельзя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChosenUserStatus {
    Online,
    Idle,
    #[serde(rename = "dnd")]
    DoNotDisturb,
}

/// Идентичность присутствия за пределами локального id: `homeUserId` и
/// `homeInstance` (оба `None` у нативного пользователя этого инстанса).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceIdentity {
    pub home_user_id: Option<String>,
    pub home_instance: Option<String>,
}

impl User {
    /// Отображаемое имя с фолбэком на `username`.
    #[must_use]
    pub fn effective_name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.username)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_user_payload() {
        let json = r#"{
            "id":"123456789012345678",
            "username":"alice",
            "displayName":null,
            "avatar":null,
            "banner":null,
            "accentColor":null,
            "avatarColor":"mint",
            "bio":null,
            "status":"dnd",
            "customStatus":null,
            "isAdmin":false,
            "createdAt":1750000000000,
            "homeInstance":null,
            "homeUserId":null,
            "replicatedInstances":[]
        }"#;
        let user: User = serde_json::from_str(json).unwrap();
        assert_eq!(user.id, "123456789012345678");
        assert_eq!(user.status, UserStatus::DoNotDisturb);
        assert_eq!(user.avatar_color.as_deref(), Some("mint"));
        assert_eq!(user.created_at, 1_750_000_000_000);
        assert!(user.replicated_instances.is_empty());
        assert_eq!(user.effective_name(), "alice");
    }

    #[test]
    fn parses_federated_user() {
        let json = r##"{
            "id":"1","username":"bob@orbit.ddns.net","displayName":"Bob",
            "avatar":"abc.png","banner":null,"accentColor":"#fff","avatarColor":"sky",
            "bio":"hi","status":"online","customStatus":null,"isAdmin":true,
            "createdAt":1,"homeInstance":"orbit.ddns.net","homeUserId":"9",
            "replicatedInstances":[{"origin":"https://orbit.ddns.net","username":"bob"}]
        }"##;
        let user: User = serde_json::from_str(json).unwrap();
        assert_eq!(user.home_instance.as_deref(), Some("orbit.ddns.net"));
        assert_eq!(user.home_user_id.as_deref(), Some("9"));
        assert_eq!(user.replicated_instances.len(), 1);
        assert_eq!(user.effective_name(), "Bob");
    }

    #[test]
    fn chosen_status_has_no_offline() {
        assert!(serde_json::from_str::<ChosenUserStatus>(r#""offline""#).is_err());
        assert_eq!(
            serde_json::from_str::<ChosenUserStatus>(r#""dnd""#).unwrap(),
            ChosenUserStatus::DoNotDisturb
        );
    }
}
