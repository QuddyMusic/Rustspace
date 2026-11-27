//! WebSocket-события: сервер → клиент и клиент → сервер.
//!
//! Источник истины — `export type ClientEvent` / `export type ServerEvent`
//! в `backspace/packages/shared/src/types.ts`. События размечены полем `type`
//! (`snake_case`). Неизвестные типы попадают в `Unknown`, чтобы старый клиент
//! не падал на новых событиях сервера.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::channel::{Channel, ChannelCategory, NotificationSetting, ReadState};
use crate::id::{ChannelId, MessageId, SpaceId, UserId};
use crate::message::{
    ActiveCallInfo, Activity, DmChannel, DmMessageWithUser, Embed, MessageWithUser, Reaction,
};
use crate::space::{JoinRequest, Space, SpaceFolder, SpaceLayoutItem, SpaceWithChannelsAndMembers};
use crate::user::{ChosenUserStatus, User, UserStatus};

/// Платформа клиента в `auth` (`ClientKind` в TS).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientKind {
    Web,
    Desktop,
    Mobile,
}

/// Федеративная идентичность с обязательными полями.
///
/// Отличается от [`crate::user::PresenceIdentity`] тем, что оба поля
/// обязательны: используется там, где сервер их всегда присылает.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FederatedIdentity {
    pub home_user_id: String,
    pub home_instance: String,
}

/// Статус заявки в друзья.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FriendRequestStatus {
    Pending,
    Accepted,
    Declined,
}

/// Друг — профиль пользователя плюс дата добавления.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Friend {
    pub id: UserId,
    pub username: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub accent_color: Option<String>,
    #[serde(default)]
    pub avatar_color: Option<String>,
    #[serde(default)]
    pub bio: Option<String>,
    pub status: UserStatus,
    #[serde(default)]
    pub custom_status: Option<String>,
    pub created_at: i64,
    pub added_at: i64,
    #[serde(default)]
    pub home_user_id: Option<String>,
    #[serde(default)]
    pub home_instance: Option<String>,
}

/// Заявка в друзья.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FriendRequest {
    pub id: String,
    pub from_id: UserId,
    pub to_id: UserId,
    pub status: FriendRequestStatus,
    pub created_at: i64,
    /// Собеседник: отправитель для входящей заявки, получатель для исходящей.
    #[serde(default)]
    pub user: Option<User>,
}

/// Вид уведомления федерации (`PeeringNotificationKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeeringNotificationKind {
    Approved,
    Denied,
    Expired,
}

/// Действие в `voice_state_update`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceStateAction {
    Join,
    Leave,
}

/// Причина принудительного отключения от голоса.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceDisconnectReason {
    Displaced,
    SessionClosed,
    Rejected,
}

/// Фаза звонка в `dm_call_undeliverable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DmCallPhase {
    Start,
    Accept,
    Reject,
    End,
    HostUnreachable,
}

/// Причина недоставки DM-звонка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DmCallUndeliverableReason {
    PeerRejected,
    PeerAwaitingApproval,
    PeerTransientFailure,
    LivekitUnavailable,
    NoRecipient,
    IdentityNotAccepted,
}

/// Одна неудача доставки звонка конкретному узлу.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmCallUndeliverableFailure {
    pub reason: DmCallUndeliverableReason,
    #[serde(default)]
    pub peer_origin: Option<String>,
    #[serde(default)]
    pub peer_label: Option<String>,
    #[serde(default)]
    pub affected_user_ids: Option<Vec<UserId>>,
}

/// Состояние голоса участника (флаги камеры/микрофона).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // Плоский DTO: флаги приходят из JSON сервера как независимые bool.
pub struct VoiceUserState {
    pub is_muted: bool,
    pub is_deafened: bool,
    pub is_camera_on: bool,
    pub is_screen_sharing: bool,
}

/// Состояние голоса участника на уровне пространства.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceVoiceState {
    pub space_muted: bool,
    pub space_deafened: bool,
    /// Приходит только в `space_voice_state`, в `ready` отсутствует.
    #[serde(default)]
    pub permission_muted: Option<bool>,
}

/// Контекст, затронутый отказом федеративного узла.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FederationAffectedContext {
    /// `dm` или `friend`.
    pub context_type: String,
    pub context_id: String,
    pub context_label: String,
}

/// Пользователь, затронутый лимитом при отклонении файла федерацией.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FederationFileAffectedUser {
    pub user_id: UserId,
    pub username: String,
    pub limit: i64,
}

/// Payload события `ready` — снимок состояния после `auth`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyPayload {
    pub user: User,
    #[serde(default)]
    pub spaces: Vec<SpaceWithChannelsAndMembers>,
    #[serde(default)]
    pub dm_channels: Vec<DmChannel>,
    #[serde(default)]
    pub folders: Option<Vec<SpaceFolder>>,
    #[serde(default)]
    pub space_layout: Option<Vec<SpaceLayoutItem>>,
    #[serde(default)]
    pub layout_updated_at: Option<i64>,
    /// `channelId` → список `userId` в голосовом канале.
    #[serde(default)]
    pub voice_states: Option<HashMap<ChannelId, Vec<UserId>>>,
    #[serde(default)]
    pub voice_channel_elapsed_seconds: Option<HashMap<ChannelId, i64>>,
    #[serde(default)]
    pub voice_user_states: Option<HashMap<UserId, VoiceUserState>>,
    #[serde(default)]
    pub supports_poke: Option<bool>,
    /// `channelId` → число непрочитанных.
    #[serde(default)]
    pub unread_counts: Option<HashMap<ChannelId, i64>>,
    #[serde(default)]
    pub read_states: Option<Vec<ReadState>>,
    #[serde(default)]
    pub active_calls: Option<Vec<ActiveCallInfo>>,
    #[serde(default)]
    pub space_voice_states: Option<HashMap<SpaceId, SpaceVoiceState>>,
    #[serde(default)]
    pub user_activities: Option<HashMap<UserId, Vec<Activity>>>,
    #[serde(default)]
    pub user_activity_identities: Option<HashMap<UserId, crate::user::PresenceIdentity>>,
    #[serde(default)]
    pub rejected_peer_origins: Option<Vec<String>>,
    #[serde(default)]
    pub awaiting_approval_peer_origins: Option<Vec<String>>,
    #[serde(default)]
    pub active_peer_origins: Option<Vec<String>>,
    #[serde(default)]
    pub pending_approval_count: Option<i64>,
}

/// Событие от сервера клиенту.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[non_exhaustive]
pub enum ServerEvent {
    /// Снимок состояния после успешного `auth`.
    Ready(Box<ReadyPayload>),

    // ─── Сообщения каналов ────────────────────────────────────────────────
    /// Новое сообщение в канале пространства.
    MessageCreated {
        message: MessageWithUser,
    },
    /// Сообщение изменено.
    MessageUpdated {
        message: MessageWithUser,
    },
    /// Сообщение удалено.
    MessageDeleted {
        message_id: MessageId,
        channel_id: ChannelId,
    },
    /// Кто-то печатает в канале.
    Typing {
        channel_id: ChannelId,
        user_id: UserId,
        username: String,
    },
    /// «Тык» — пинок пользователя в канале.
    ChannelPoke {
        message: MessageWithUser,
    },
    /// Подтверждение прочтения.
    ChannelAck {
        channel_id: ChannelId,
        message_id: MessageId,
    },
    /// Канал помечен непрочитанным.
    MarkUnread {
        channel_id: ChannelId,
        message_id: MessageId,
    },
    /// Изменились счётчики непрочитанных.
    ChannelUnreadCount {
        counts: HashMap<ChannelId, i64>,
    },
    /// Сервер доразрешил embeds для сообщения.
    EmbedsResolved {
        message_id: MessageId,
        channel_id: ChannelId,
        embeds: Vec<Embed>,
    },

    // ─── Реакции ──────────────────────────────────────────────────────────
    /// Добавлена реакция.
    ReactionAdded {
        message_id: MessageId,
        reaction: Reaction,
    },
    /// Реакция убрана.
    ReactionRemoved {
        message_id: MessageId,
        user_id: UserId,
        emoji: String,
    },

    // ─── Присутствие ──────────────────────────────────────────────────────
    /// Обновление статуса присутствия; `homeUserId`/`homeInstance` —
    /// федеративная идентичность (может отсутствовать).
    PresenceUpdate {
        user_id: UserId,
        status: UserStatus,
        #[serde(default)]
        activities: Option<Vec<Activity>>,
        #[serde(default)]
        home_user_id: Option<String>,
        #[serde(default)]
        home_instance: Option<String>,
    },

    // ─── Пространства и участники ─────────────────────────────────────────
    /// Участник вошёл в пространство.
    MemberJoined {
        space_id: SpaceId,
        member: crate::space::MemberWithUser,
    },
    /// Участник обновлён (ник, роли).
    MemberUpdated {
        space_id: SpaceId,
        member: crate::space::MemberWithUser,
    },
    /// Участник покинул пространство.
    MemberLeft {
        space_id: SpaceId,
        user_id: UserId,
    },
    /// Участник забанен.
    MemberBanned {
        space_id: SpaceId,
        #[serde(default)]
        reason: Option<String>,
    },
    /// Пространство обновлено.
    SpaceUpdated {
        space: Space,
    },
    /// Изменились роли или доступ к пространству — нужно перезапросить детали.
    SpaceAccessChanged {
        space_id: SpaceId,
    },
    /// Пользователь обновлён.
    UserUpdated {
        user: User,
    },

    // ─── Каналы и категории ───────────────────────────────────────────────
    ChannelCreated {
        channel: Channel,
        space_id: SpaceId,
    },
    ChannelUpdated {
        channel: Channel,
        space_id: SpaceId,
    },
    ChannelDeleted {
        channel_id: ChannelId,
        space_id: SpaceId,
    },
    CategoryCreated {
        category: ChannelCategory,
        space_id: SpaceId,
    },
    CategoryUpdated {
        category: ChannelCategory,
        space_id: SpaceId,
    },
    CategoryDeleted {
        category_id: crate::id::CategoryId,
        space_id: SpaceId,
    },
    /// Массовое обновление раскладки каналов пространства.
    ChannelLayoutUpdated {
        space_id: SpaceId,
        channels: Vec<Channel>,
        categories: Vec<ChannelCategory>,
    },
    /// Личная раскладка сайдбара пользователя обновилась.
    SpaceLayoutUpdated {
        layout: Vec<SpaceLayoutItem>,
        folders: Vec<SpaceFolder>,
        #[serde(default)]
        updated_at: Option<i64>,
    },

    // ─── Заявки на вступление ─────────────────────────────────────────────
    JoinRequestReceived {
        request: JoinRequest,
    },
    JoinRequestAccepted {
        request: JoinRequest,
        space: SpaceWithChannelsAndMembers,
    },
    JoinRequestDeclined {
        request: JoinRequest,
    },

    // ─── Голос ────────────────────────────────────────────────────────────
    /// Участник вошёл или покинул голосовой канал.
    VoiceStateUpdate {
        channel_id: ChannelId,
        user_id: UserId,
        action: VoiceStateAction,
        #[serde(default)]
        channel_elapsed_seconds: Option<i64>,
    },
    /// Флаги микрофона/камеры/шеринга обновились.
    VoiceStatusUpdate {
        user_id: UserId,
        channel_id: ChannelId,
        is_muted: bool,
        is_deafened: bool,
        is_camera_on: bool,
        is_screen_sharing: bool,
    },
    /// Полный снимок голосового состояния пространства.
    SpaceVoiceState {
        space_id: SpaceId,
        voice_states: HashMap<ChannelId, Vec<UserId>>,
        voice_channel_elapsed_seconds: HashMap<ChannelId, i64>,
        voice_user_states: HashMap<UserId, VoiceUserState>,
        space_voice_states: HashMap<UserId, SpaceVoiceState>,
    },
    /// Модератор замьютил участника на уровне пространства.
    VoiceSpaceMuted {
        user_id: UserId,
        channel_id: ChannelId,
        space_id: SpaceId,
        muted: bool,
    },
    /// Модератор задефенил участника на уровне пространства.
    VoiceSpaceDeafened {
        user_id: UserId,
        channel_id: ChannelId,
        space_id: SpaceId,
        deafened: bool,
    },
    /// Участник замьючен из-за нехватки прав (не модератором).
    VoicePermissionMuted {
        user_id: UserId,
        space_id: SpaceId,
        muted: bool,
    },
    /// Участника переместили в другой голосовой канал.
    VoiceMoved {
        user_id: UserId,
        old_channel_id: ChannelId,
        new_channel_id: ChannelId,
    },
    /// Участника принудительно отключили от голоса.
    VoiceDisconnected {
        user_id: UserId,
        channel_id: ChannelId,
        #[serde(default)]
        reason: Option<VoiceDisconnectReason>,
    },

    // ─── Личные сообщения ─────────────────────────────────────────────────
    DmMessageCreated {
        message: DmMessageWithUser,
    },
    DmMessageUpdated {
        message: DmMessageWithUser,
    },
    DmMessageDeleted {
        message_id: MessageId,
        dm_channel_id: String,
    },
    DmTyping {
        dm_channel_id: String,
        user_id: UserId,
        username: String,
    },
    DmTypingStop {
        dm_channel_id: String,
        user_id: UserId,
    },
    DmChannelCreated {
        dm_channel: DmChannel,
    },
    DmChannelClosed {
        dm_channel_id: String,
    },
    DmChannelUpdated {
        dm_channel_id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        icon: Option<String>,
    },
    DmMemberAdded {
        dm_channel_id: String,
        user: User,
    },
    DmMemberRemoved {
        dm_channel_id: String,
        user_id: UserId,
    },
    DmOwnerUpdated {
        dm_channel_id: String,
        new_owner_id: UserId,
        #[serde(default)]
        new_owner_home_user_id: Option<String>,
        #[serde(default)]
        new_owner_home_instance: Option<String>,
    },
    DmEmbedsResolved {
        message_id: MessageId,
        dm_channel_id: String,
        embeds: Vec<Embed>,
    },

    // ─── DM-звонки ────────────────────────────────────────────────────────
    DmCallIncoming {
        dm_channel_id: Option<String>,
        #[serde(default)]
        federated_call_id: Option<String>,
        caller_id: UserId,
        caller_name: String,
        #[serde(default)]
        livekit_url: Option<String>,
        #[serde(default)]
        livekit_token: Option<String>,
        #[serde(default)]
        call_origin: Option<String>,
    },
    DmCallAccepted {
        dm_channel_id: Option<String>,
        #[serde(default)]
        federated_call_id: Option<String>,
        #[serde(default)]
        answered_by: Option<FederatedIdentity>,
    },
    DmCallRejected {
        dm_channel_id: Option<String>,
        #[serde(default)]
        federated_call_id: Option<String>,
    },
    DmCallEnded {
        dm_channel_id: Option<String>,
        #[serde(default)]
        federated_call_id: Option<String>,
    },
    DmCallUndeliverable {
        dm_channel_id: Option<String>,
        federated_call_id: String,
        terminal: bool,
        phase: DmCallPhase,
        failures: Vec<DmCallUndeliverableFailure>,
    },

    // ─── Друзья ───────────────────────────────────────────────────────────
    FriendRequestReceived {
        request: FriendRequest,
    },
    FriendRequestSent {
        request: FriendRequest,
    },
    FriendRequestAccepted {
        friend: Friend,
        request_id: String,
    },
    FriendRequestCancelled {
        request_id: String,
        user_id: UserId,
    },
    FriendRequestDeclined {
        request_id: String,
        user_id: UserId,
    },
    FriendRequestRelayFailed {
        request_id: String,
        reason: String,
        message: String,
        target_handle: String,
    },
    FriendRemoved {
        user_id: UserId,
    },

    // ─── Уведомления ──────────────────────────────────────────────────────
    NotificationSettingsUpdated {
        setting: NotificationSetting,
    },

    // ─── Федерация ────────────────────────────────────────────────────────
    FederationFileRejected {
        message_id: MessageId,
        dm_channel_id: String,
        attachment_id: String,
        affected_users: Vec<FederationFileAffectedUser>,
    },
    FederationPeerRejected {
        peer_origin: String,
        #[serde(default)]
        peer_label: Option<String>,
        reason: String,
        #[serde(default)]
        reason_code: Option<String>,
        affected_contexts: Vec<FederationAffectedContext>,
    },
    FederationPeerActive {
        peer_origin: String,
    },
    FederationPeersChanged,
    FederationPeerResetDetected {
        origin: String,
    },
    FederationApprovalRequestReceived {
        origin: String,
        #[serde(default)]
        instance_name: Option<String>,
    },
    PeeringSubscriptionChanged,
    PeeringNotificationReceived {
        kind: PeeringNotificationKind,
    },

    // ─── Служебные ────────────────────────────────────────────────────────
    /// Ответ на клиентский `ping`.
    Pong,
    /// Ошибка, связанная с конкретным запросом.
    Error {
        message: String,
        #[serde(default)]
        code: Option<String>,
        #[serde(default)]
        details: Option<serde_json::Map<String, serde_json::Value>>,
        #[serde(default)]
        dm_channel_id: Option<String>,
    },
    // ─── Rustspace ────────────────────────────────────────────────────────
    /// Изменились закреплённые сообщения чата (`kind`: `channel` или `dm`).
    RsPinsUpdated {
        kind: String,
        channel_id: String,
    },
    /// В ленту личного чата добавились старые сообщения (перенос истории
    /// моста): загруженную ленту стоит перечитать.
    RsDmHistory {
        channel_id: String,
    },
    /// Добавлена закладка (PR оригинала #474).
    BookmarkAdded {
        bookmark: serde_json::Value,
    },
    /// Убрана закладка (PR оригинала #474).
    BookmarkRemoved {
        kind: String,
        channel_id: String,
        message_id: String,
    },
    /// Неизвестное событие — не ломаем парсинг.
    #[serde(other)]
    Unknown,
}

/// Событие от клиента серверу.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[non_exhaustive]
pub enum ClientEvent {
    /// Аутентификация: должна прийти в первые 10 секунд после connect.
    Auth {
        token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client: Option<ClientKind>,
    },
    /// Отправить сообщение в канал пространства.
    MessageCreate {
        channel_id: ChannelId,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to_id: Option<MessageId>,
    },
    /// Изменить сообщение.
    MessageEdit {
        message_id: MessageId,
        content: String,
    },
    /// Удалить сообщение.
    MessageDelete { message_id: MessageId },
    /// «Ткнуть» пользователя в канале.
    ChannelPoke {
        channel_id: ChannelId,
        target_user_id: UserId,
    },
    /// Начать печатать.
    TypingStart { channel_id: ChannelId },
    /// Сменить собственный статус.
    PresenceUpdate { status: ChosenUserStatus },
    /// Войти в голосовой канал.
    VoiceJoin { channel_id: ChannelId },
    /// Покинуть голосовой канал (без параметров).
    VoiceLeave,
    /// Отправить личное сообщение.
    DmMessageCreate {
        dm_channel_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to_id: Option<MessageId>,
    },
    /// Начать печатать в DM.
    DmTypingStart { dm_channel_id: String },
    /// Изменить DM-сообщение.
    DmMessageEdit {
        message_id: MessageId,
        content: String,
    },
    /// Удалить DM-сообщение.
    DmMessageDelete { message_id: MessageId },
    /// Добавить реакцию.
    ReactionAdd {
        message_id: MessageId,
        emoji: String,
    },
    /// Убрать реакцию.
    ReactionRemove {
        message_id: MessageId,
        emoji: String,
    },
    /// Подтвердить прочтение канала.
    ChannelAck {
        channel_id: ChannelId,
        message_id: MessageId,
    },
    /// Пометить канал непрочитанным.
    MarkUnread {
        channel_id: ChannelId,
        message_id: MessageId,
    },
    /// Начать DM-звонок.
    DmCallStart { dm_channel_id: String },
    /// Принять DM-звонок.
    DmCallAccept {
        dm_channel_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        federated_call_id: Option<String>,
    },
    /// Отклонить DM-звонок.
    DmCallReject {
        dm_channel_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        federated_call_id: Option<String>,
    },
    /// Завершить DM-звонок.
    DmCallEnd {
        dm_channel_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        federated_call_id: Option<String>,
    },
    /// Обновить собственные флаги голоса.
    VoiceStatus {
        is_muted: bool,
        is_deafened: bool,
        is_camera_on: bool,
        is_screen_sharing: bool,
    },
    /// Замьютить участника на уровне пространства.
    VoiceSpaceMute { user_id: UserId, muted: bool },
    /// Задефенить участника на уровне пространства.
    VoiceSpaceDeafen { user_id: UserId, deafened: bool },
    /// Переместить участника в другой голосовой канал.
    VoiceMove {
        user_id: UserId,
        target_channel_id: ChannelId,
    },
    /// Отключить участника от голоса.
    VoiceDisconnect { user_id: UserId },
    /// Обновить список активностей (Rich Presence).
    ActivityUpdate { activities: Vec<Activity> },
    /// Клиентский heartbeat.
    Ping,
    /// Неизвестное событие.
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ping_is_unit() {
        let ev: ClientEvent = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert_eq!(ev, ClientEvent::Ping);
        assert_eq!(
            serde_json::to_string(&ClientEvent::Ping).unwrap(),
            r#"{"type":"ping"}"#
        );
    }

    #[test]
    fn client_auth_with_platform() {
        let ev: ClientEvent =
            serde_json::from_str(r#"{"type":"auth","token":"jwt","client":"desktop"}"#).unwrap();
        match ev {
            ClientEvent::Auth { token, client } => {
                assert_eq!(token, "jwt");
                assert_eq!(client, Some(ClientKind::Desktop));
            }
            other => panic!("ожидался auth, получено {other:?}"),
        }
    }

    #[test]
    fn unknown_server_and_client_events_are_tolerated() {
        assert_eq!(
            serde_json::from_str::<ServerEvent>(r#"{"type":"future_thing","x":1}"#).unwrap(),
            ServerEvent::Unknown
        );
        assert_eq!(
            serde_json::from_str::<ClientEvent>(r#"{"type":"weird"}"#).unwrap(),
            ClientEvent::Unknown
        );
    }

    #[test]
    fn server_pong_is_unit() {
        assert_eq!(
            serde_json::from_str::<ServerEvent>(r#"{"type":"pong"}"#).unwrap(),
            ServerEvent::Pong
        );
    }

    const ALICE: &str = r#"{
        "id":"u1","username":"alice","displayName":null,"avatar":null,
        "banner":null,"accentColor":null,"avatarColor":null,"bio":null,
        "status":"online","customStatus":null,"isAdmin":false,
        "createdAt":1,"homeInstance":null,"homeUserId":null,
        "replicatedInstances":[]
    }"#;

    #[test]
    fn message_created_parses_nested_user() {
        let json = format!(
            r#"{{"type":"message_created","message":{{
                "id":"m1","channelId":"c1","userId":"u1","replyToId":null,
                "content":"hi","type":"user","editedAt":null,"createdAt":5,
                "user":{ALICE},"attachments":[],"embeds":[],"reactions":[]
            }}}}"#
        );
        let ev: ServerEvent = serde_json::from_str(&json).unwrap();
        match ev {
            ServerEvent::MessageCreated { message } => {
                assert_eq!(message.message.id, "m1");
                assert_eq!(message.message.content.as_deref(), Some("hi"));
            }
            other => panic!("ожидалось message_created, получено {other:?}"),
        }
    }

    #[test]
    fn ready_minimal_payload() {
        let json = format!(
            r#"{{"type":"ready","user":{ALICE},"spaces":[],"dmChannels":[],
                "supportsPoke":true,"unreadCounts":{{"c1":3}},
                "readStates":[{{"channelId":"c1","lastReadMessageId":"m2"}}]}}"#
        );
        let ev: ServerEvent = serde_json::from_str(&json).unwrap();
        match ev {
            ServerEvent::Ready(r) => {
                assert_eq!(r.user.username, "alice");
                assert_eq!(r.supports_poke, Some(true));
                assert_eq!(r.unread_counts.as_ref().unwrap().get("c1"), Some(&3));
                assert_eq!(r.read_states.as_ref().unwrap().len(), 1);
                assert!(r.folders.is_none());
            }
            other => panic!("ожидалось ready, получено {other:?}"),
        }
    }

    #[test]
    fn typing_event() {
        let json = r#"{"type":"typing","channelId":"c1","userId":"u1","username":"alice"}"#;
        let ev: ServerEvent = serde_json::from_str(json).unwrap();
        assert_eq!(
            ev,
            ServerEvent::Typing {
                channel_id: "c1".into(),
                user_id: "u1".into(),
                username: "alice".into()
            }
        );
    }

    #[test]
    fn presence_update_with_identity() {
        let json = r#"{"type":"presence_update","userId":"u1","status":"idle",
            "activities":[{"type":"playing","name":"Rust"}],
            "homeUserId":"42","homeInstance":"orbit.example"}"#;
        let ev: ServerEvent = serde_json::from_str(json).unwrap();
        match ev {
            ServerEvent::PresenceUpdate {
                status,
                activities,
                home_instance,
                ..
            } => {
                assert_eq!(status, UserStatus::Idle);
                assert_eq!(activities.unwrap().len(), 1);
                assert_eq!(home_instance.as_deref(), Some("orbit.example"));
            }
            other => panic!("ожидалось presence_update, получено {other:?}"),
        }
    }

    #[test]
    fn voice_state_update_action() {
        let json = r#"{"type":"voice_state_update","channelId":"c1","userId":"u1",
            "action":"leave","channelElapsedSeconds":12}"#;
        let ev: ServerEvent = serde_json::from_str(json).unwrap();
        match ev {
            ServerEvent::VoiceStateUpdate {
                action,
                channel_elapsed_seconds,
                ..
            } => {
                assert_eq!(action, VoiceStateAction::Leave);
                assert_eq!(channel_elapsed_seconds, Some(12));
            }
            other => panic!("ожидалось voice_state_update, получено {other:?}"),
        }
    }

    #[test]
    fn dm_call_undeliverable() {
        let json = r#"{"type":"dm_call_undeliverable","dmChannelId":null,
            "federatedCallId":"fc1","terminal":true,"phase":"start",
            "failures":[{"reason":"peer_rejected","peerOrigin":"https://x.example"}]}"#;
        let ev: ServerEvent = serde_json::from_str(json).unwrap();
        match ev {
            ServerEvent::DmCallUndeliverable {
                terminal,
                phase,
                failures,
                ..
            } => {
                assert!(terminal);
                assert_eq!(phase, DmCallPhase::Start);
                assert_eq!(failures[0].reason, DmCallUndeliverableReason::PeerRejected);
            }
            other => panic!("ожидалось dm_call_undeliverable, получено {other:?}"),
        }
    }

    #[test]
    fn error_event_optional_code() {
        let json = r#"{"type":"error","message":"nope","code":"invalid_credentials"}"#;
        let ev: ServerEvent = serde_json::from_str(json).unwrap();
        match ev {
            ServerEvent::Error { message, code, .. } => {
                assert_eq!(message, "nope");
                assert_eq!(code.as_deref(), Some("invalid_credentials"));
            }
            other => panic!("ожидалось error, получено {other:?}"),
        }
    }

    #[test]
    fn client_channel_ack_serializes_type() {
        let ev = ClientEvent::ChannelAck {
            channel_id: "c1".into(),
            message_id: "m9".into(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(
            json,
            r#"{"type":"channel_ack","channelId":"c1","messageId":"m9"}"#
        );
    }
}
