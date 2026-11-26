//! Сообщения, реакции, вложения, embeds, личные сообщения и activity.
//!
//! Источник истины — `backspace/packages/shared/src/types.ts`.
//! Время — epoch-миллисекунды (`i64`).

use serde::{Deserialize, Serialize};

use crate::id::{ChannelId, Emoji, MessageId, UserId};
use crate::user::User;

/// Автор сообщения: обычный пользователь или системное уведомление.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    User,
    System,
}

/// Сообщение в канале пространства.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: MessageId,
    pub channel_id: ChannelId,
    pub user_id: UserId,
    #[serde(default)]
    pub reply_to_id: Option<MessageId>,
    #[serde(default)]
    pub content: Option<String>,
    /// `user` или `system`; сервер может не прислать — тогда обычное сообщение.
    #[serde(rename = "type", default)]
    pub message_type: Option<MessageType>,
    #[serde(default)]
    pub edited_at: Option<i64>,
    pub created_at: i64,
    /// Проставлено только инстансом, хостящим пространство, если контент
    /// содержит `@everyone` или `@here`.
    #[serde(default)]
    pub mention_everyone: Option<bool>,
    #[serde(default)]
    pub mention_role_ids: Option<Vec<String>>,
}

/// Сообщение с автором, вложениями, embeds и реакциями.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageWithUser {
    #[serde(flatten)]
    pub message: Message,
    pub user: User,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    /// Рекурсивный вложенный ответ (может отсутствовать или быть `null`).
    #[serde(default)]
    pub reply_to: Option<Box<Self>>,
}

/// Реакция — плоская строка, не агрегат по эмодзи.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reaction {
    pub id: String,
    pub message_id: MessageId,
    pub user_id: UserId,
    pub emoji: Emoji,
    pub created_at: i64,
    #[serde(default)]
    pub user: Option<User>,
}

/// Вложение (файл, изображение, видео).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub message_id: MessageId,
    pub filename: String,
    pub original_name: String,
    pub mimetype: String,
    pub size: i64,
    #[serde(default)]
    pub thumbnail_filename: Option<String>,
    #[serde(default)]
    pub width: Option<i64>,
    #[serde(default)]
    pub height: Option<i64>,
    #[serde(default)]
    /// Длительность в секундах; в БД сервера колонка `real`, поэтому дробная.
    pub duration: Option<f64>,
    /// Воспроизводимость видео в браузере; `null` — неизвестно (оптимистично).
    /// Старые серверы отдавали флаг числом `0/1` — принимаем и так.
    #[serde(default, deserialize_with = "flag_or_number")]
    pub playable: Option<bool>,
    #[serde(default)]
    pub federation_status: Option<String>,
    #[serde(default)]
    pub federation_meta: Option<String>,
    pub created_at: i64,
}

/// Тип embed-а.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbedType {
    Generic,
    Video,
    Image,
    Audio,
    Rich,
}

/// Известный провайдер embed-а.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbedProvider {
    Youtube,
    Vimeo,
    Spotify,
}

/// Развёрнутое превью ссылки или медиа.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Embed {
    pub id: String,
    #[serde(default)]
    pub message_id: Option<MessageId>,
    #[serde(default)]
    pub dm_message_id: Option<String>,
    pub url: String,
    pub embed_type: EmbedType,
    #[serde(default)]
    pub provider: Option<EmbedProvider>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub embed_url: Option<String>,
    #[serde(default)]
    pub width: Option<i64>,
    #[serde(default)]
    pub height: Option<i64>,
    #[serde(default)]
    pub color: Option<String>,
    pub created_at: i64,
}

/// Короткое превью последнего сообщения DM-канала для сайдбара.
///
/// Лишние поля допускаются: если сервер добавит новое, ЛС не должны
/// перестать разбираться. Полное сообщение отличается обязательным `user`,
/// поэтому в [`DmChannelLastMessage`] оно проверяется первым.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmLastMessagePreview {
    pub id: MessageId,
    pub dm_channel_id: String,
    pub user_id: UserId,
    #[serde(default)]
    pub content: Option<String>,
    pub created_at: i64,
    #[serde(rename = "type", default)]
    pub message_type: Option<MessageType>,
    #[serde(default)]
    pub attachments: Option<Vec<DmPreviewAttachment>>,
}

/// Мини-описание вложения в превью (`{type, filename}` в TS).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmPreviewAttachment {
    #[serde(rename = "type")]
    pub attachment_type: String,
    pub filename: String,
}

/// Сообщение личной переписки.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmMessage {
    pub id: MessageId,
    pub dm_channel_id: String,
    pub user_id: UserId,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(rename = "type", default)]
    pub message_type: Option<MessageType>,
    pub created_at: i64,
    /// Совместимость: старые серверы дублировали `channelId`.
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    #[serde(default)]
    pub reply_to_id: Option<MessageId>,
    #[serde(default)]
    pub edited_at: Option<i64>,
    /// Идентичность релея федерации: исходный id и инстанс.
    #[serde(default)]
    pub source_message_id: Option<String>,
    #[serde(default)]
    pub source_instance: Option<String>,
}

/// DM-сообщение с автором и вложениями.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmMessageWithUser {
    #[serde(flatten)]
    pub message: DmMessage,
    pub user: User,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    #[serde(default)]
    pub reply_to: Option<Box<Self>>,
}

/// `lastMessage` в [`DmChannel`]: короткое превью или полное сообщение.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DmChannelLastMessage {
    Full(Box<DmMessageWithUser>),
    Preview(DmLastMessagePreview),
}

/// Личный канал (или группа).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmChannel {
    pub id: String,
    /// Ключ беседы; `null` — только для группы, которой нет ни на одном инстансе.
    pub federated_id: Option<String>,
    pub owner_id: Option<UserId>,
    pub owner_home_user_id: Option<String>,
    pub owner_home_instance: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub members: Vec<User>,
    #[serde(default)]
    pub last_message: Option<DmChannelLastMessage>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    pub metadata_updated_at: i64,
}

/// Тип activity (Rich Presence).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityType {
    Custom,
    Playing,
    Listening,
    Watching,
    Streaming,
}

/// Временные метки activity.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityTimestamps {
    #[serde(default)]
    pub start: Option<i64>,
    #[serde(default)]
    pub end: Option<i64>,
}

/// Прогресс воспроизведения, не зависящий от часов отправителя: получатель
/// привязывает позицию к своим часам в момент получения.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPlayback {
    pub position_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
}

/// Картинки activity.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityAssets {
    #[serde(default)]
    pub large_image: Option<String>,
    #[serde(default)]
    pub large_text: Option<String>,
    #[serde(default)]
    pub small_image: Option<String>,
    #[serde(default)]
    pub small_text: Option<String>,
}

/// Активность пользователя для присутствия.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    #[serde(rename = "type")]
    pub activity_type: ActivityType,
    pub name: String,
    #[serde(default)]
    pub details: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub timestamps: Option<ActivityTimestamps>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback: Option<ActivityPlayback>,
    #[serde(default)]
    pub assets: Option<ActivityAssets>,
    #[serde(default)]
    pub url: Option<String>,
}

/// Состояние звонка в DM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallState {
    Ringing,
    Active,
}

/// Активный звонок, как его отдаёт `ready`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveCallInfo {
    #[serde(default)]
    pub dm_channel_id: Option<String>,
    #[serde(default)]
    pub federated_call_id: Option<String>,
    pub caller_id: UserId,
    #[serde(default)]
    pub participants: Vec<UserId>,
    pub started_at: i64,
    pub state: CallState,
    #[serde(default)]
    pub federated_call_host: Option<String>,
    #[serde(default)]
    pub livekit_url: Option<String>,
    #[serde(default)]
    pub livekit_token: Option<String>,
}

/// Тело `POST /api/livekit/token` (`LiveKitTokenRequest` в TS).
///
/// Для голосового канала заполнен `channel_id`, для DM-звонка —
/// `dm_channel_id`; незаполненное поле не сериализуется.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveKitTokenRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dm_channel_id: Option<String>,
}

/// Ответ `POST /api/livekit/token` (`LiveKitTokenResponse` в TS):
/// JWT с грантами комнаты и `wss://`-адрес LiveKit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveKitTokenResponse {
    pub token: String,
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: &str = r#"{
        "id":"u1","username":"alice","displayName":null,"avatar":null,
        "banner":null,"accentColor":null,"avatarColor":null,"bio":null,
        "status":"online","customStatus":null,"isAdmin":false,
        "createdAt":1,"homeInstance":null,"homeUserId":null,
        "replicatedInstances":[]
    }"#;

    #[test]
    fn parses_real_message_with_user() {
        let json = format!(
            r#"{{
                "id":"m1","channelId":"c1","userId":"u1","replyToId":null,
                "content":"привет","type":"user","editedAt":null,
                "createdAt":1755000000000,"mentionEveryone":true,"mentionRoleIds":["r1"],
                "user":{ALICE},
                "attachments":[],"embeds":[],"reactions":[],"replyTo":null
            }}"#
        );
        let m: MessageWithUser = serde_json::from_str(&json).unwrap();
        assert_eq!(m.message.id, "m1");
        assert_eq!(m.message.message_type, Some(MessageType::User));
        assert_eq!(m.message.mention_everyone, Some(true));
        assert_eq!(
            m.message.mention_role_ids.as_deref(),
            Some(&["r1".to_string()][..])
        );
        assert_eq!(m.user.username, "alice");
        assert!(m.reply_to.is_none());
    }

    #[test]
    fn minimal_message_without_optional_fields() {
        let json = r#"{"id":"m1","channelId":"c1","userId":"u1",
            "replyToId":null,"content":null,"editedAt":null,"createdAt":5}"#;
        let m: Message = serde_json::from_str(json).unwrap();
        assert!(m.message_type.is_none());
        assert!(m.content.is_none());
        assert!(m.mention_everyone.is_none());
    }

    #[test]
    fn reaction_is_flat_row() {
        let json = r#"{"id":"re1","messageId":"m1","userId":"u1",
            "emoji":"👍","createdAt":9}"#;
        let r: Reaction = serde_json::from_str(json).unwrap();
        assert_eq!(r.message_id, "m1");
        assert_eq!(r.emoji, "👍");
        assert!(r.user.is_none());
    }

    #[test]
    fn attachment_with_nullable_media_fields() {
        let json = r#"{"id":"a1","messageId":"m1","filename":"x.png",
            "originalName":"x.png","mimetype":"image/png","size":1234,
            "thumbnailFilename":null,"width":null,"height":null,
            "duration":null,"playable":null,"createdAt":7}"#;
        let a: Attachment = serde_json::from_str(json).unwrap();
        assert_eq!(a.size, 1234);
        assert!(a.playable.is_none());
        assert!(a.width.is_none());
    }

    #[test]
    fn attachment_with_fractional_duration() {
        // Сервер хранит `duration` как REAL: у видео и аудио число дробное.
        let json = r#"{"id":"a1","messageId":"m1","filename":"x.mp4",
            "originalName":"x.mp4","mimetype":"video/mp4","size":99,
            "thumbnailFilename":"t.jpg","width":640,"height":360,
            "duration":12.48,"playable":true,"createdAt":7}"#;
        let a: Attachment = serde_json::from_str(json).unwrap();
        assert_eq!(a.duration, Some(12.48));
    }

    #[test]
    fn embed_with_provider() {
        let json = r#"{"id":"e1","messageId":"m1","dmMessageId":null,
            "url":"https://youtu.be/x","embedType":"video","provider":"youtube",
            "title":"T","description":null,"image":null,"embedUrl":null,
            "width":null,"height":null,"color":null,"createdAt":1}"#;
        let e: Embed = serde_json::from_str(json).unwrap();
        assert_eq!(e.embed_type, EmbedType::Video);
        assert_eq!(e.provider, Some(EmbedProvider::Youtube));
    }

    #[test]
    fn dm_channel_with_preview_last_message() {
        let json = format!(
            r#"{{
                "id":"dm1","federatedId":null,"ownerId":null,
                "ownerHomeUserId":null,"ownerHomeInstance":null,"createdAt":1,
                "members":[{ALICE}],
                "lastMessage":{{"id":"m1","dmChannelId":"dm1","userId":"u1",
                    "content":"hi","createdAt":2}},
                "name":null,"icon":null,"metadataUpdatedAt":3
            }}"#
        );
        let dm: DmChannel = serde_json::from_str(&json).unwrap();
        assert!(dm.federated_id.is_none());
        assert_eq!(dm.members.len(), 1);
        match dm.last_message {
            Some(DmChannelLastMessage::Preview(p)) => assert_eq!(p.content.as_deref(), Some("hi")),
            other => panic!("ожидалось превью, получено {other:?}"),
        }
    }

    #[test]
    fn preview_with_unknown_field_still_parses() {
        let json = format!(
            r#"{{
                "id":"dm1","federatedId":null,"ownerId":null,
                "ownerHomeUserId":null,"ownerHomeInstance":null,"createdAt":1,
                "members":[{ALICE}],
                "lastMessage":{{"id":"m1","dmChannelId":"dm1","userId":"u1",
                    "content":"hi","createdAt":2,"type":"user","futureField":7}},
                "name":null,"icon":null,"metadataUpdatedAt":3
            }}"#
        );
        let dm: DmChannel = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            dm.last_message,
            Some(DmChannelLastMessage::Preview(_))
        ));
    }

    #[test]
    fn dm_channel_with_full_last_message() {
        let json = format!(
            r#"{{
                "id":"dm1","federatedId":"f1","ownerId":"u1",
                "ownerHomeUserId":null,"ownerHomeInstance":null,"createdAt":1,
                "members":[{ALICE}],
                "lastMessage":{{"id":"m1","dmChannelId":"dm1","userId":"u1",
                    "content":"yo","createdAt":2,"user":{ALICE},
                    "attachments":[],"embeds":[],"reactions":[]}},
                "name":"Group","icon":null,"metadataUpdatedAt":3
            }}"#
        );
        let dm: DmChannel = serde_json::from_str(&json).unwrap();
        match dm.last_message {
            Some(DmChannelLastMessage::Full(m)) => assert_eq!(m.user.username, "alice"),
            other => panic!("ожидалось полное сообщение, получено {other:?}"),
        }
    }

    #[test]
    fn activity_parses_partial_fields() {
        let json = r#"{"type":"listening","name":"Spotify",
            "details":"Song","timestamps":{"start":10},
            "assets":{"largeImage":"img"}}"#;
        let a: Activity = serde_json::from_str(json).unwrap();
        assert_eq!(a.activity_type, ActivityType::Listening);
        assert!(a.timestamps.as_ref().unwrap().end.is_none());
        assert!(a.assets.unwrap().large_text.is_none());
    }

    #[test]
    fn active_call_state_parsing() {
        let json = r#"{"dmChannelId":null,"callerId":"u1","participants":["u1","u2"],
            "startedAt":5,"state":"ringing"}"#;
        let c: ActiveCallInfo = serde_json::from_str(json).unwrap();
        assert_eq!(c.state, CallState::Ringing);
        assert_eq!(c.participants.len(), 2);
    }
}

/// `true`/`false`, `0`/`1` или `null`.
fn flag_or_number<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Flag {
        Bool(bool),
        Int(i64),
    }
    Ok(
        Option::<Flag>::deserialize(deserializer)?.map(|flag| match flag {
            Flag::Bool(value) => value,
            Flag::Int(value) => value != 0,
        }),
    )
}

#[cfg(test)]
mod flag_tests {
    use super::*;

    #[derive(Deserialize)]
    struct Probe {
        #[serde(default, deserialize_with = "flag_or_number")]
        playable: Option<bool>,
    }

    #[allow(clippy::expect_used, reason = "тесты")]
    fn parse(json: &str) -> Option<bool> {
        serde_json::from_str::<Probe>(json).expect("json").playable
    }

    #[test]
    fn playable_accepts_numbers() {
        assert_eq!(parse(r#"{"playable": 1}"#), Some(true));
        assert_eq!(parse(r#"{"playable": 0}"#), Some(false));
        assert_eq!(parse(r#"{"playable": true}"#), Some(true));
        assert_eq!(parse(r#"{"playable": null}"#), None);
        assert_eq!(parse("{}"), None);
    }
}
