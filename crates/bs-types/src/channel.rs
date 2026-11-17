//! Каналы, категории, оверрайды прав, read state, настройки уведомлений.
//!
//! Источник истины — `backspace/packages/shared/src/types.ts`.
//! Время — epoch-миллисекунды (`i64`).

use serde::{Deserialize, Serialize};

use crate::id::{CategoryId, ChannelId, MessageId, SpaceId};

/// Уровень уведомлений по умолчанию (`DEFAULT_NOTIFICATION_LEVEL`).
pub const DEFAULT_NOTIFICATION_LEVEL: NotificationLevel = NotificationLevel::Mentions;

/// Тип канала.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    Text,
    Voice,
}

/// Канал пространства.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: ChannelId,
    pub space_id: SpaceId,
    pub name: String,
    /// В JSON поле называется `type` (зарезервированное слово в Rust).
    #[serde(rename = "type")]
    pub channel_type: ChannelType,
    #[serde(default)]
    pub topic: Option<String>,
    pub position: i64,
    #[serde(default)]
    pub category_id: Option<CategoryId>,
    #[serde(default)]
    pub is_private: Option<bool>,
    pub created_at: i64,
    #[serde(default)]
    pub last_message_id: Option<MessageId>,
    /// Эффективные права текущего пользователя (decimal string).
    #[serde(default)]
    pub my_permissions: Option<String>,
    /// Канал «18+» (Rustspace; у оригинала поля нет).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nsfw: bool,
}

/// Категория каналов.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelCategory {
    pub id: CategoryId,
    pub space_id: SpaceId,
    pub name: String,
    pub position: i64,
    #[serde(default)]
    pub is_private: Option<bool>,
    pub created_at: i64,
}

/// Кому применяется оверрайд прав.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverrideTargetType {
    Role,
    Member,
}

/// Оверрайд прав на категорию (не на канал — поле называется `categoryId`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryOverride {
    pub category_id: CategoryId,
    pub target_type: OverrideTargetType,
    pub target_id: String,
    /// Разрешённые биты (decimal string).
    pub allow: String,
    /// Запрещённые биты (decimal string).
    pub deny: String,
}

/// Состояние прочтения канала. В протоколе — короткое: только два поля.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadState {
    pub channel_id: ChannelId,
    #[serde(default)]
    pub last_read_message_id: Option<MessageId>,
}

/// Уровень уведомлений.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    All,
    Mentions,
    Nothing,
}

/// Длительность mute-а (`NotificationMuteDuration` в TS).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NotificationMuteDuration {
    #[serde(rename = "1h")]
    OneHour,
    #[serde(rename = "8h")]
    EightHours,
    #[serde(rename = "24h")]
    TwentyFourHours,
    #[serde(rename = "indefinite")]
    Indefinite,
}

/// Настройка уведомлений для канала (`channelId != None`) или пространства.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationSetting {
    #[serde(default)]
    pub suppress_everyone: Option<bool>,
    #[serde(default)]
    pub suppress_roles: Option<bool>,
    pub space_id: SpaceId,
    /// `None` — настройка уровня пространства.
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    /// `None` — наследовать уровень.
    #[serde(default)]
    pub level: Option<NotificationLevel>,
    pub muted: bool,
    /// `None` — mute бессрочный (см. [`is_mute_active`]).
    #[serde(default)]
    pub muted_until: Option<i64>,
    pub updated_at: i64,
}

/// Активен ли mute в момент `now` (epoch-мс).
///
/// Точный порт TS-функции `isMuteActive(setting, now)`:
/// `mutedUntil === null || mutedUntil > now`. Флаг `muted` здесь **не**
/// проверяется — вызывающая сторона учитывает его отдельно, как в вебе.
#[must_use]
pub const fn is_mute_active(setting: &NotificationSetting, now: i64) -> bool {
    match setting.muted_until {
        None => true,
        Some(until) => until > now,
    }
}

/// Откуда взят действующий уровень уведомлений.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevelSource {
    Channel,
    Space,
    Default,
}

/// Вычисленная политика уведомлений для канала.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // Плоский DTO: вычисленная политика сервера, поля независимы.
pub struct ChannelNotificationPolicy {
    pub suppress_everyone: bool,
    pub suppress_roles: bool,
    pub level: NotificationLevel,
    pub level_source: NotificationLevelSource,
    pub muted: bool,
    #[serde(default)]
    pub muted_until: Option<i64>,
    pub channel_muted: bool,
    pub space_muted: bool,
}

/// Тело запроса на изменение настройки уведомлений.
///
/// `level?: Level | null` и `mute?: Duration | null`: отсутствие поля — не
/// менять, `null` — сбросить. Для сериализации это `Option<Option<T>>`;
/// при десериализации serde схлопывает `null` в `None` — для запросов,
/// которые мы только отправляем, это допустимо.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNotificationSettingRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppress_everyone: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppress_roles: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<Option<NotificationLevel>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mute: Option<Option<NotificationMuteDuration>>,
}

/// Ответ `GET /api/notifications/settings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationSettingsResponse {
    #[serde(default)]
    pub settings: Vec<NotificationSetting>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_channel_payload() {
        let json = r#"{
            "id": "c1", "spaceId": "s1", "name": "general", "type": "text",
            "topic": "main", "position": 3, "categoryId": "cat1",
            "isPrivate": false, "createdAt": 1755000000000,
            "lastMessageId": "m9", "myPermissions": "1024"
        }"#;
        let ch: Channel = serde_json::from_str(json).unwrap();
        assert_eq!(ch.channel_type, ChannelType::Text);
        assert_eq!(ch.category_id.as_deref(), Some("cat1"));
        assert_eq!(ch.created_at, 1_755_000_000_000);
        assert_eq!(ch.last_message_id.as_deref(), Some("m9"));
    }

    #[test]
    fn voice_channel_without_optional_fields() {
        let json = r#"{
            "id": "c2", "spaceId": "s1", "name": "Voice", "type": "voice",
            "position": 0, "createdAt": 1
        }"#;
        let ch: Channel = serde_json::from_str(json).unwrap();
        assert_eq!(ch.channel_type, ChannelType::Voice);
        assert!(ch.category_id.is_none());
        assert!(ch.topic.is_none());
    }

    #[test]
    fn read_state_is_short() {
        let json = r#"{"channelId":"c1","lastReadMessageId":"m5"}"#;
        let rs: ReadState = serde_json::from_str(json).unwrap();
        assert_eq!(rs.channel_id, "c1");
        assert_eq!(rs.last_read_message_id.as_deref(), Some("m5"));
    }

    #[test]
    fn notification_levels_and_default() {
        assert_eq!(
            serde_json::from_str::<NotificationLevel>("\"nothing\"").unwrap(),
            NotificationLevel::Nothing
        );
        assert_eq!(DEFAULT_NOTIFICATION_LEVEL, NotificationLevel::Mentions);
    }

    #[test]
    fn mute_durations_are_short_strings() {
        assert_eq!(
            serde_json::from_str::<NotificationMuteDuration>("\"1h\"").unwrap(),
            NotificationMuteDuration::OneHour
        );
        assert_eq!(
            serde_json::to_string(&NotificationMuteDuration::Indefinite).unwrap(),
            "\"indefinite\""
        );
    }

    #[test]
    fn mute_active_matches_ts_semantics() {
        let mut setting = NotificationSetting {
            suppress_everyone: None,
            suppress_roles: None,
            space_id: "s1".into(),
            channel_id: None,
            level: None,
            muted: true,
            muted_until: None,
            updated_at: 1,
        };
        // mutedUntil === null → бессрочный mute.
        assert!(is_mute_active(&setting, 1_000));
        setting.muted_until = Some(2_000);
        assert!(is_mute_active(&setting, 1_000));
        assert!(!is_mute_active(&setting, 2_000));
    }

    #[test]
    fn update_request_omits_absent_fields() {
        let req = UpdateNotificationSettingRequest {
            level: Some(Some(NotificationLevel::All)),
            ..UpdateNotificationSettingRequest::default()
        };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"level":"all"}"#);
    }

    #[test]
    fn policy_parses() {
        let json = r#"{
            "suppressEveryone": false, "suppressRoles": true,
            "level": "mentions", "levelSource": "space",
            "muted": false, "mutedUntil": null,
            "channelMuted": false, "spaceMuted": false
        }"#;
        let p: ChannelNotificationPolicy = serde_json::from_str(json).unwrap();
        assert_eq!(p.level, NotificationLevel::Mentions);
        assert_eq!(p.level_source, NotificationLevelSource::Space);
    }
}
