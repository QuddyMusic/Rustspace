//! Структуры кэша состояния Backspace.
//!
//! Кэш — снимок того, что сервер прислал в `ready`, плюс дельты из
//! последующих событий. Заполняется из [`ReadyPayload`] в
//! [`Cache::from_ready`], обновляется функциями из [`crate::apply`] (этап 4).

use std::collections::HashMap;

use bs_types::{
    ActiveCallInfo, Activity, Channel, ChannelId, DmChannel, DmMessageWithUser, Friend,
    FriendRequest, MessageWithUser, NotificationSetting, PresenceIdentity, ReadState, ReadyPayload, SpaceFolder,
    SpaceId, SpaceLayoutItem, SpaceVoiceState, SpaceWithChannelsAndMembers, User, UserId,
    UserStatus, VoiceUserState,
};

/// Сколько «печатает…» живёт без продления (мс).
///
/// Сервер присылает `typing` только в момент нажатия клавиши, поэтому запись
/// считается протухшей, если новое событие не пришло за это время.
pub const TYPING_TTL_MS: i64 = 8_000;

/// Запись «пользователь печатает в канале».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypingEntry {
    /// Кто печатает.
    pub user_id: UserId,
    /// Имя на момент события — чтобы показать его без поиска по кэшу.
    pub username: String,
    /// Момент (epoch-мс), после которого запись считается протухшей.
    pub expires_at: i64,
}

impl TypingEntry {
    /// Жива ли запись на момент `now_ms` (epoch-мс).
    #[must_use]
    pub fn is_active(&self, now_ms: i64) -> bool {
        self.expires_at > now_ms
    }
}

/// Полный кэш инстанса.
///
/// Поля с `pub(crate)`-видимостью — для [`crate::apply`]; внешний код
/// работает через read-аксессоры.
#[derive(Debug)]
#[must_use]
pub struct Cache {
    pub(crate) me: User,
    pub(crate) spaces: HashMap<SpaceId, SpaceWithChannelsAndMembers>,
    pub(crate) dm_channels: HashMap<String, DmChannel>,
    pub(crate) messages: HashMap<ChannelId, Vec<MessageWithUser>>,
    /// Сообщения личных каналов по id личного канала.
    pub(crate) dm_messages: HashMap<String, Vec<DmMessageWithUser>>,
    pub(crate) typing: HashMap<ChannelId, Vec<TypingEntry>>,
    pub(crate) folders: Vec<SpaceFolder>,
    pub(crate) space_layout: Vec<SpaceLayoutItem>,
    pub(crate) layout_updated_at: Option<i64>,
    pub(crate) voice_states: HashMap<ChannelId, Vec<UserId>>,
    pub(crate) voice_channel_elapsed_seconds: HashMap<ChannelId, i64>,
    pub(crate) voice_user_states: HashMap<UserId, VoiceUserState>,
    pub(crate) supports_poke: bool,
    pub(crate) unread_counts: HashMap<ChannelId, i64>,
    /// Настройки уведомлений: (пространство, канал либо `None` для всего пространства).
    pub(crate) notification_settings: HashMap<(SpaceId, Option<ChannelId>), NotificationSetting>,
    pub(crate) read_states: HashMap<ChannelId, ReadState>,
    pub(crate) active_calls: Vec<ActiveCallInfo>,
    pub(crate) space_voice_states: HashMap<SpaceId, SpaceVoiceState>,
    pub(crate) user_activities: HashMap<UserId, Vec<Activity>>,
    pub(crate) user_activity_identities: HashMap<UserId, PresenceIdentity>,
    /// Последний известный статус пользователя, в том числе для тех, кого нет
    /// ни в одном пространстве и ни в одном DM (fallback к `User::status`).
    pub(crate) user_statuses: HashMap<UserId, UserStatus>,
    pub(crate) rejected_peer_origins: Vec<String>,
    pub(crate) awaiting_approval_peer_origins: Vec<String>,
    pub(crate) active_peer_origins: Vec<String>,
    pub(crate) pending_approval_count: i64,
    /// Друзья по `UserId`.
    pub(crate) friends: HashMap<UserId, Friend>,
    /// Заявки в друзья по id заявки (входящие и исходящие, только `pending`).
    pub(crate) friend_requests: HashMap<String, FriendRequest>,
}

impl Cache {
    /// Собирает кэш из `ready`-payload.
    ///
    /// Все опциональные поля `ReadyPayload` (кроме `user`) при отсутствии в
    /// JSON заменяются пустыми коллекциями или `false`/`0`.
    pub fn from_ready(payload: ReadyPayload) -> Self {
        let ReadyPayload {
            user,
            spaces,
            dm_channels,
            folders,
            space_layout,
            layout_updated_at,
            voice_states,
            voice_channel_elapsed_seconds,
            voice_user_states,
            supports_poke,
            unread_counts,
            read_states,
            active_calls,
            space_voice_states,
            user_activities,
            user_activity_identities,
            rejected_peer_origins,
            awaiting_approval_peer_origins,
            active_peer_origins,
            pending_approval_count,
        } = payload;

        // Снимок статусов: сам пользователь, участники пространств и DM.
        let mut user_statuses = HashMap::new();
        user_statuses.insert(user.id.clone(), user.status);
        for space in &spaces {
            for member in &space.members {
                user_statuses.insert(member.user.id.clone(), member.user.status);
            }
        }
        for dm in &dm_channels {
            for member in &dm.members {
                user_statuses.insert(member.id.clone(), member.status);
            }
        }

        Self {
            me: user,
            spaces: spaces
                .into_iter()
                .map(|s| (s.space.id.clone(), s))
                .collect(),
            dm_channels: dm_channels.into_iter().map(|d| (d.id.clone(), d)).collect(),
            messages: HashMap::new(),
            dm_messages: HashMap::new(),
            typing: HashMap::new(),
            folders: folders.unwrap_or_default(),
            space_layout: space_layout.unwrap_or_default(),
            layout_updated_at,
            voice_states: voice_states.unwrap_or_default(),
            voice_channel_elapsed_seconds: voice_channel_elapsed_seconds.unwrap_or_default(),
            voice_user_states: voice_user_states.unwrap_or_default(),
            supports_poke: supports_poke.unwrap_or(false),
            unread_counts: unread_counts.unwrap_or_default(),
            notification_settings: HashMap::new(),
            read_states: read_states
                .unwrap_or_default()
                .into_iter()
                .map(|rs| (rs.channel_id.clone(), rs))
                .collect(),
            active_calls: active_calls.unwrap_or_default(),
            space_voice_states: space_voice_states.unwrap_or_default(),
            user_activities: user_activities.unwrap_or_default(),
            user_activity_identities: user_activity_identities.unwrap_or_default(),
            user_statuses,
            rejected_peer_origins: rejected_peer_origins.unwrap_or_default(),
            awaiting_approval_peer_origins: awaiting_approval_peer_origins.unwrap_or_default(),
            active_peer_origins: active_peer_origins.unwrap_or_default(),
            pending_approval_count: pending_approval_count.unwrap_or(0),
            friends: HashMap::new(),
            friend_requests: HashMap::new(),
        }
    }

    /// Собственный профиль пользователя этого инстанса.
    #[must_use]
    pub fn me(&self) -> &User {
        &self.me
    }

    /// Пространства, доступные текущему пользователю (итератор по `values`).
    pub fn spaces(&self) -> impl Iterator<Item = &SpaceWithChannelsAndMembers> {
        self.spaces.values()
    }

    /// Пространство по id.
    #[must_use]
    pub fn space(&self, id: &str) -> Option<&SpaceWithChannelsAndMembers> {
        self.spaces.get(id)
    }

    /// Каналы одного пространства.
    pub fn channels(&self, space_id: &str) -> impl Iterator<Item = &Channel> {
        self.spaces
            .get(space_id)
            .into_iter()
            .flat_map(|swc| swc.channels.iter())
    }

    /// Все каналы всех пространств.
    pub fn all_channels(&self) -> impl Iterator<Item = &Channel> {
        self.spaces.values().flat_map(|swc| swc.channels.iter())
    }

    /// Канал по id (линейный поиск по всем пространствам).
    #[must_use]
    pub fn channel(&self, channel_id: &str) -> Option<&Channel> {
        self.all_channels().find(|c| c.id == channel_id)
    }

    /// Личные каналы (DM).
    pub fn dm_channels(&self) -> impl Iterator<Item = &DmChannel> {
        self.dm_channels.values()
    }

    /// Личный канал по id.
    #[must_use]
    pub fn dm_channel(&self, id: &str) -> Option<&DmChannel> {
        self.dm_channels.get(id)
    }

    /// Закэшированные сообщения канала (пустой срез, если канал не загружен).
    #[must_use]
    pub fn messages(&self, channel_id: &str) -> &[MessageWithUser] {
        self.messages.get(channel_id).map_or(&[], Vec::as_slice)
    }

    /// Добавляет или обновляет личный канал (например, после `POST /api/dm`:
    /// сервер не обязан присылать создателю `dm_channel_created`).
    pub fn upsert_dm_channel(&mut self, channel: DmChannel) {
        self.dm_channels.insert(channel.id.clone(), channel);
    }

    /// Добавляет или обновляет пространство целиком (после создания или
    /// вступления через REST: событие о нём создателю не гарантировано).
    pub fn upsert_space(&mut self, space: SpaceWithChannelsAndMembers) {
        for member in &space.members {
            self.user_statuses
                .entry(member.user.id.clone())
                .or_insert(member.user.status);
        }
        self.spaces.insert(space.space.id.clone(), space);
    }

    /// Забывает пространство (после выхода из него).
    pub fn remove_space(&mut self, space_id: &str) {
        self.spaces.remove(space_id);
    }

    /// Закэшированные сообщения личного канала (пустой срез, если не загружены).
    #[must_use]
    pub fn dm_messages(&self, dm_channel_id: &str) -> &[DmMessageWithUser] {
        self.dm_messages
            .get(dm_channel_id)
            .map_or(&[], Vec::as_slice)
    }

    /// Вливает страницу истории канала пространства в кэш.
    ///
    /// Уже известные сообщения не перезаписываются: их ведёт поток живых
    /// событий, он свежее ответа REST. Возвращает число новых сообщений.
    pub fn merge_history(&mut self, channel_id: &str, page: Vec<MessageWithUser>) -> usize {
        let list = self.messages.entry(channel_id.to_owned()).or_default();
        merge_sorted(list, page, |m| {
            (m.message.created_at, m.message.id.as_str())
        })
    }

    /// Вливает страницу истории личного канала в кэш (см. [`Self::merge_history`]).
    pub fn merge_dm_history(&mut self, dm_channel_id: &str, page: Vec<DmMessageWithUser>) -> usize {
        let list = self
            .dm_messages
            .entry(dm_channel_id.to_owned())
            .or_default();
        merge_sorted(list, page, |m| {
            (m.message.created_at, m.message.id.as_str())
        })
    }

    /// Кто печатает в канале на момент `now_ms` (протухшие записи отброшены).
    pub fn typing_in(&self, channel_id: &str, now_ms: i64) -> impl Iterator<Item = &TypingEntry> {
        self.typing
            .get(channel_id)
            .into_iter()
            .flatten()
            .filter(move |e| e.is_active(now_ms))
    }

    /// Печатает ли конкретный пользователь в канале на момент `now_ms`.
    #[must_use]
    pub fn is_typing(&self, channel_id: &str, user_id: &str, now_ms: i64) -> bool {
        self.typing_in(channel_id, now_ms)
            .any(|e| e.user_id == user_id)
    }

    /// Последний известный статус пользователя.
    ///
    /// Работает и для тех, кого нет ни в одном пространстве и ни в одном DM —
    /// на таких пользователей `User::status` недоступен.
    #[must_use]
    pub fn user_status(&self, user_id: &str) -> Option<UserStatus> {
        self.user_statuses.get(user_id).copied()
    }

    /// Папки пространств пользователя.
    #[must_use]
    pub fn folders(&self) -> &[SpaceFolder] {
        &self.folders
    }

    /// Порядок пространств/папок в сайдбаре.
    #[must_use]
    pub fn space_layout(&self) -> &[SpaceLayoutItem] {
        &self.space_layout
    }

    /// Время последнего обновления раскладки (epoch-мс).
    #[must_use]
    pub fn layout_updated_at(&self) -> Option<i64> {
        self.layout_updated_at
    }

    /// Пользователи в голосовом канале (пустой срез, если пусто).
    #[must_use]
    pub fn users_in_voice(&self, channel_id: &str) -> &[UserId] {
        self.voice_states.get(channel_id).map_or(&[], Vec::as_slice)
    }

    /// Прошло секунд с начала голосовой сессии в канале (если известно).
    #[must_use]
    pub fn voice_channel_elapsed_seconds(&self, channel_id: &str) -> Option<i64> {
        self.voice_channel_elapsed_seconds.get(channel_id).copied()
    }

    /// Голосовое состояние пользователя (mute/deafen/камера/шэринг).
    #[must_use]
    pub fn voice_user_state(&self, user_id: &str) -> Option<&VoiceUserState> {
        self.voice_user_states.get(user_id)
    }

    /// Поддерживает ли инстанс `channel_poke`.
    #[must_use]
    pub fn supports_poke(&self) -> bool {
        self.supports_poke
    }

    /// Заменяет все настройки уведомлений списком с сервера.
    pub fn set_notification_settings(&mut self, list: Vec<NotificationSetting>) {
        self.notification_settings = list
            .into_iter()
            .map(|s| ((s.space_id.clone(), s.channel_id.clone()), s))
            .collect();
    }

    /// Применяет одну настройку; более старая запись не затирает новую.
    pub fn apply_notification_setting(&mut self, setting: NotificationSetting) {
        let key = (setting.space_id.clone(), setting.channel_id.clone());
        if self
            .notification_settings
            .get(&key)
            .is_none_or(|old| setting.updated_at >= old.updated_at)
        {
            self.notification_settings.insert(key, setting);
        }
    }

    /// Сохранённая настройка пространства (`channel_id == None`) или канала.
    #[must_use]
    pub fn notification_setting(
        &self,
        space_id: &str,
        channel_id: Option<&str>,
    ) -> Option<&NotificationSetting> {
        self.notification_settings
            .get(&(space_id.to_owned(), channel_id.map(str::to_owned)))
    }

    /// Что действует для канала пространства в момент `now`.
    #[must_use]
    pub fn channel_policy(
        &self,
        space_id: &str,
        channel_id: &str,
        now: i64,
    ) -> bs_types::ChannelNotificationPolicy {
        crate::notifications::resolve_policy(
            self.notification_setting(space_id, None),
            self.notification_setting(space_id, Some(channel_id)),
            now,
        )
    }

    /// Число непрочитанных в канале (0, если счётчик не приходил).
    #[must_use]
    pub fn unread_count(&self, channel_id: &str) -> i64 {
        self.unread_counts.get(channel_id).copied().unwrap_or(0)
    }

    /// Read state канала.
    #[must_use]
    pub fn read_state(&self, channel_id: &str) -> Option<&ReadState> {
        self.read_states.get(channel_id)
    }

    /// Активные DM-звонки (снимок `ready`).
    #[must_use]
    pub fn active_calls(&self) -> &[ActiveCallInfo] {
        &self.active_calls
    }

    /// Состояние голоса на уровне пространства (space mute/deafen).
    #[must_use]
    pub fn space_voice_state(&self, space_id: &str) -> Option<&SpaceVoiceState> {
        self.space_voice_states.get(space_id)
    }

    /// Rich Presence пользователя (пустой срез, если активности нет).
    #[must_use]
    pub fn user_activities(&self, user_id: &str) -> &[Activity] {
        self.user_activities.get(user_id).map_or(&[], Vec::as_slice)
    }

    /// Федеративная идентичность пользователя для activity.
    #[must_use]
    pub fn user_activity_identity(&self, user_id: &str) -> Option<&PresenceIdentity> {
        self.user_activity_identities.get(user_id)
    }

    /// Отклонённые федеративные узлы.
    #[must_use]
    pub fn rejected_peer_origins(&self) -> &[String] {
        &self.rejected_peer_origins
    }

    /// Узлы, ожидающие одобрения пейринга.
    #[must_use]
    pub fn awaiting_approval_peer_origins(&self) -> &[String] {
        &self.awaiting_approval_peer_origins
    }

    /// Активные (пейринг подтверждён) узлы.
    #[must_use]
    pub fn active_peer_origins(&self) -> &[String] {
        &self.active_peer_origins
    }

    /// Количество заявок на одобрение пейринга.
    #[must_use]
    pub fn pending_approval_count(&self) -> i64 {
        self.pending_approval_count
    }

    /// Друзья, отсортированные по отображаемому имени (без учёта регистра).
    pub fn friends(&self) -> impl Iterator<Item = &Friend> {
        let mut list: Vec<(&Friend, String)> = self
            .friends
            .values()
            .map(|friend| (friend, friend_sort_key(friend)))
            .collect();
        list.sort_by(|a, b| a.1.cmp(&b.1));
        list.into_iter().map(|(friend, _)| friend)
    }

    /// Друг по идентификатору пользователя.
    pub fn friend(&self, user_id: &str) -> Option<&Friend> {
        self.friends.get(user_id)
    }

    /// Все известные заявки в друзья (входящие и исходящие), по времени создания.
    pub fn friend_requests(&self) -> impl Iterator<Item = &FriendRequest> {
        let mut list: Vec<&FriendRequest> = self.friend_requests.values().collect();
        list.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        list.into_iter()
    }

    /// Заявка в друзья по её идентификатору.
    pub fn friend_request(&self, id: &str) -> Option<&FriendRequest> {
        self.friend_requests.get(id)
    }

    /// Входящие заявки (адресованы текущему пользователю).
    pub fn incoming_friend_requests(&self) -> impl Iterator<Item = &FriendRequest> {
        self.friend_requests()
            .filter(|request| request.to_id == self.me.id)
    }

    /// Исходящие заявки (отправлены текущим пользователем).
    pub fn outgoing_friend_requests(&self) -> impl Iterator<Item = &FriendRequest> {
        self.friend_requests()
            .filter(|request| request.from_id == self.me.id)
    }
}

/// Ключ сортировки друзей: отображаемое имя (или username) в нижнем регистре.
fn friend_sort_key(friend: &Friend) -> String {
    friend
        .display_name
        .as_deref()
        .unwrap_or(friend.username.as_str())
        .to_lowercase()
}

/// Ключ порядка сообщений: время, затем snowflake-id (короче — меньше).
fn order_key(created_at: i64, id: &str) -> (i64, usize, &str) {
    (created_at, id.len(), id)
}

/// Вставляет `item` в отсортированный список; существующий id не трогает.
///
/// Возвращает `true`, если сообщение добавлено.
#[allow(
    clippy::redundant_pub_crate,
    reason = "используется из apply; `pub` тут нарушил бы unreachable_pub"
)]
pub(crate) fn insert_sorted<T>(
    list: &mut Vec<T>,
    item: T,
    key: impl for<'a> Fn(&'a T) -> (i64, &'a str),
) -> bool {
    let (created_at, id) = key(&item);
    if list.iter().any(|existing| key(existing).1 == id) {
        return false;
    }
    let wanted = order_key(created_at, id);
    let pos = list.partition_point(|existing| {
        let (c, i) = key(existing);
        order_key(c, i) <= wanted
    });
    list.insert(pos, item);
    true
}

fn merge_sorted<T>(
    list: &mut Vec<T>,
    page: Vec<T>,
    key: impl for<'a> Fn(&'a T) -> (i64, &'a str) + Copy,
) -> usize {
    let mut added = 0;
    for item in page {
        if insert_sorted(list, item, key) {
            added += 1;
        }
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;

    const READY_JSON: &str = r#"{
        "user": {
            "id": "u1", "username": "alice", "displayName": null, "avatar": null,
            "banner": null, "accentColor": null, "avatarColor": null, "bio": null,
            "status": "online", "customStatus": null, "isAdmin": false,
            "createdAt": 1, "homeInstance": null, "homeUserId": null,
            "replicatedInstances": []
        },
        "spaces": [{
            "id": "s1", "name": "Space", "ownerId": "u1",
            "visibility": "public", "directoryListed": false, "createdAt": 1,
            "channels": [{
                "id": "c1", "spaceId": "s1", "name": "general", "type": "text",
                "position": 0, "createdAt": 1
            }],
            "categories": [], "members": [], "roles": []
        }],
        "dmChannels": [{
            "id": "dm1", "federatedId": null, "ownerId": null,
            "ownerHomeUserId": null, "ownerHomeInstance": null, "createdAt": 1,
            "members": [], "metadataUpdatedAt": 2
        }],
        "folders": [], "spaceLayout": [], "layoutUpdatedAt": null,
        "voiceStates": {"c1": ["u1"]},
        "voiceChannelElapsedSeconds": {"c1": 42},
        "voiceUserStates": {"u1": {"isMuted": false, "isDeafened": false,
            "isCameraOn": false, "isScreenSharing": false}},
        "supportsPoke": true,
        "unreadCounts": {"c1": 3},
        "readStates": [{"channelId": "c1", "lastReadMessageId": "m1"}],
        "activeCalls": [],
        "spaceVoiceStates": {},
        "userActivities": {}, "userActivityIdentities": {},
        "rejectedPeerOrigins": ["evil.example"],
        "awaitingApprovalPeerOrigins": [],
        "activePeerOrigins": ["friend.example"],
        "pendingApprovalCount": 1
    }"#;

    fn ready() -> ReadyPayload {
        serde_json::from_str(READY_JSON).expect("готовый payload парсится")
    }

    #[test]
    fn from_ready_copies_core_fields() {
        let cache = Cache::from_ready(ready());
        assert_eq!(cache.me().username, "alice");
        assert_eq!(cache.spaces().count(), 1);
        assert_eq!(cache.all_channels().count(), 1);
        assert_eq!(cache.dm_channels().count(), 1);
        assert_eq!(cache.unread_count("c1"), 3);
        assert_eq!(cache.unread_count("nope"), 0);
        assert_eq!(
            cache
                .read_state("c1")
                .and_then(|s| s.last_read_message_id.as_deref()),
            Some("m1")
        );
        assert_eq!(cache.users_in_voice("c1"), ["u1".to_string()]);
        assert!(cache.users_in_voice("none").is_empty());
        assert_eq!(cache.voice_channel_elapsed_seconds("c1"), Some(42));
        assert!(cache.voice_user_state("u1").is_some());
        assert!(cache.supports_poke());
        assert_eq!(cache.rejected_peer_origins(), ["evil.example".to_string()]);
        assert_eq!(cache.active_peer_origins(), ["friend.example".to_string()]);
        assert!(cache.awaiting_approval_peer_origins().is_empty());
        assert_eq!(cache.pending_approval_count(), 1);
        assert!(cache.messages("c1").is_empty());
        assert!(cache.active_calls().is_empty());
    }

    #[test]
    fn lookup_helpers() {
        let cache = Cache::from_ready(ready());
        assert_eq!(
            cache.space("s1").map(|s| s.space.name.as_str()),
            Some("Space")
        );
        assert!(cache.space("nope").is_none());
        assert_eq!(
            cache.channel("c1").map(|c| c.name.as_str()),
            Some("general")
        );
        assert!(cache.channel("nope").is_none());
        assert_eq!(cache.channels("s1").count(), 1);
        assert_eq!(cache.channels("nope").count(), 0);
        assert!(cache.dm_channel("dm1").is_some());
        assert!(cache.dm_channel("nope").is_none());
    }

    #[test]
    fn minimal_payload_defaults() {
        let json = r#"{
            "user": {
                "id": "u1", "username": "bob", "displayName": null, "avatar": null,
                "banner": null, "accentColor": null, "avatarColor": null, "bio": null,
                "status": "offline", "customStatus": null, "isAdmin": false,
                "createdAt": 1, "homeInstance": null, "homeUserId": null,
                "replicatedInstances": []
            }
        }"#;
        let payload: ReadyPayload = serde_json::from_str(json).expect("минимум парсится");
        let cache = Cache::from_ready(payload);
        assert_eq!(cache.me().username, "bob");
        assert_eq!(cache.spaces().count(), 0);
        assert!(cache.folders().is_empty());
        assert!(cache.space_layout().is_empty());
        assert!(cache.layout_updated_at().is_none());
        assert!(!cache.supports_poke());
        assert_eq!(cache.pending_approval_count(), 0);
        assert!(cache.active_calls().is_empty());
        assert!(cache.space_voice_state("s1").is_none());
        assert!(cache.user_activities("u1").is_empty());
        assert!(cache.user_activity_identity("u1").is_none());
        assert!(cache.typing_in("c1", 0).next().is_none());
    }

    #[test]
    fn typing_entries_expire() {
        let mut cache = Cache::from_ready(ready());
        cache.typing.insert(
            "c1".to_string(),
            vec![
                TypingEntry {
                    user_id: "u2".to_string(),
                    username: "bob".to_string(),
                    expires_at: 1_000,
                },
                TypingEntry {
                    user_id: "u3".to_string(),
                    username: "carol".to_string(),
                    expires_at: 5_000,
                },
            ],
        );

        let names: Vec<&str> = cache
            .typing_in("c1", 1_500)
            .map(|e| e.username.as_str())
            .collect();
        assert_eq!(names, ["carol"]);
        assert!(!cache.is_typing("c1", "u2", 1_500));
        assert!(cache.is_typing("c1", "u3", 1_500));
        assert!(cache.typing_in("c1", 5_000).next().is_none());
        assert!(cache.typing_in("nope", 0).next().is_none());
    }

    #[test]
    fn user_status_snapshot_from_ready() {
        let cache = Cache::from_ready(ready());
        assert_eq!(cache.user_status("u1"), Some(UserStatus::Online));
        assert_eq!(cache.user_status("nobody"), None);
    }
}
