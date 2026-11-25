//! Объединённый кэш нескольких инстансов.
//!
//! [`Cache`] описывает состояние одного инстанса. В мультиинстансном
//! клиенте их несколько, и одинаковые идентификаторы (каналов, сообщений,
//! пользователей) в разных инстансах — это разные сущности. [`MultiCache`]
//! держит по кэшу на [`Origin`] и отдаёт объединённые представления
//! с пометкой инстанса-источника.
//!
//! Маршрутизация событий: [`MultiCache::apply_event`] принимает
//! [`OriginEvent`] из мультиплексора `bs-instances` и раскладывает его
//! в кэш нужного инстанса; [`MultiCache::drain`] неблокирующе забирает
//! пачку событий (для кадра UI).
//!
//! # Пример
//! ```
//! use bs_core::MultiCache;
//! use bs_types::{Origin, ReadyPayload, ServerEvent};
//!
//! # fn demo(home: ReadyPayload, remote: ReadyPayload, event: ServerEvent) {
//! let mut multi = MultiCache::new();
//! multi.insert_ready(Origin::Home, home);
//! multi.insert_ready(Origin::parse_web_str("https://peer.example").unwrap(), remote);
//!
//! // Событие адресуется конкретному инстансу.
//! multi.apply(&Origin::Home, event);
//!
//! // Объединённый список пространств с origin-метками.
//! for (origin, space) in multi.spaces() {
//!     println!("{:?}: {}", origin, space.space.name);
//! }
//! # }
//! ```

use std::collections::HashMap;

use bs_instances::{InstanceEvent, OriginEvent};
use bs_rt::sync::broadcast;
use bs_types::{
    Channel, DmChannel, MessageWithUser, Origin, ReadyPayload, ServerEvent,
    SpaceWithChannelsAndMembers,
};

use crate::apply::apply;
use crate::state::Cache;

/// Результат маршрутизации одного события.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Routed {
    /// Инстанс не подключён — событие отброшено.
    UnknownOrigin,
    /// Инстанс есть, но кэш не изменился (игнорируемое или рефетч-событие).
    Unchanged,
    /// Кэш инстанса изменился.
    Changed,
}

impl Routed {
    /// Изменился ли кэш.
    #[must_use]
    pub const fn is_changed(self) -> bool {
        matches!(self, Self::Changed)
    }

    /// Было ли событие принято (инстанс известен).
    #[must_use]
    pub const fn is_accepted(self) -> bool {
        !matches!(self, Self::UnknownOrigin)
    }
}

/// Итог неблокирующего разбора потока событий.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrainStats {
    /// Сколько событий принято известными инстансами.
    pub applied: usize,
    /// Сколько из них изменили кэш.
    pub changed: usize,
    /// Сколько событий пришло от неизвестных инстансов.
    pub unknown: usize,
    /// Был ли разрыв потока (`Lagged`) — часть событий потеряна.
    pub lagged: bool,
}

impl DrainStats {
    /// Были ли изменения состояния.
    #[must_use]
    pub const fn is_changed(&self) -> bool {
        self.changed > 0
    }
}

/// Кэши всех подключённых инстансов с маршрутизацией событий.
#[derive(Debug, Default)]
pub struct MultiCache {
    caches: HashMap<Origin, Cache>,
}

impl MultiCache {
    /// Пустой набор (ни одного инстанса).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Набор из готовых снимков инстансов.
    #[must_use]
    pub fn from_ready(instances: impl IntoIterator<Item = (Origin, ReadyPayload)>) -> Self {
        let mut multi = Self::new();
        for (origin, payload) in instances {
            multi.insert_ready(origin, payload);
        }
        multi
    }

    /// Заводит (или перезаписывает) кэш инстанса из `ready`.
    ///
    /// Возвращает `true`, если инстанс был добавлен впервые.
    pub fn insert_ready(&mut self, origin: Origin, payload: ReadyPayload) -> bool {
        self.caches
            .insert(origin, Cache::from_ready(payload))
            .is_none()
    }

    /// Вставляет уже построенный кэш, возвращая прежний.
    pub fn insert_cache(&mut self, origin: Origin, cache: Cache) -> Option<Cache> {
        self.caches.insert(origin, cache)
    }

    /// Убирает инстанс (например, при выходе из него).
    pub fn remove(&mut self, origin: &Origin) -> Option<Cache> {
        self.caches.remove(origin)
    }

    /// Кэш инстанса.
    #[must_use]
    pub fn cache(&self, origin: &Origin) -> Option<&Cache> {
        self.caches.get(origin)
    }

    /// Мутабельный кэш инстанса (для точечных локальных правок UI).
    pub fn cache_mut(&mut self, origin: &Origin) -> Option<&mut Cache> {
        self.caches.get_mut(origin)
    }

    /// Подключён ли инстанс.
    #[must_use]
    pub fn contains(&self, origin: &Origin) -> bool {
        self.caches.contains_key(origin)
    }

    /// Число инстансов.
    #[must_use]
    pub fn len(&self) -> usize {
        self.caches.len()
    }

    /// Ни одного инстанса.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.caches.is_empty()
    }

    /// Origin-ы в стабильном порядке: сначала домашний, затем удалённые
    /// по возрастанию строкового представления.
    #[must_use]
    pub fn origins(&self) -> Vec<Origin> {
        let mut origins: Vec<Origin> = self.caches.keys().cloned().collect();
        sort_origins(&mut origins);
        origins
    }

    /// Применяет событие к кэшу указанного инстанса.
    pub fn apply(&mut self, origin: &Origin, event: ServerEvent) -> Routed {
        self.caches
            .get_mut(origin)
            .map_or(Routed::UnknownOrigin, |cache| {
                if apply(cache, event) {
                    Routed::Changed
                } else {
                    Routed::Unchanged
                }
            })
    }

    /// Маршрутизирует событие из мультиплексора по его origin.
    pub fn apply_event(&mut self, event: OriginEvent) -> Routed {
        match event.event {
            InstanceEvent::Server(server) => self.apply(&event.origin, *server),
            // Состояние соединения и отказ авторизации кэш не меняют, но
            // инстанс известен — это не «чужое» событие.
            InstanceEvent::StateChanged(_) | InstanceEvent::AuthRejected { .. } => {
                if self.contains(&event.origin) {
                    Routed::Unchanged
                } else {
                    Routed::UnknownOrigin
                }
            }
        }
    }

    /// Неблокирующе забирает до `limit` событий из объединённого потока.
    ///
    /// Возвращает статистику. Закрытый поток не считается ошибкой —
    /// разбор просто прекращается.
    pub fn drain(&mut self, rx: &mut broadcast::Receiver<OriginEvent>, limit: usize) -> DrainStats {
        let mut stats = DrainStats::default();
        for _ in 0..limit {
            match rx.try_recv() {
                Ok(event) => match self.apply_event(event) {
                    Routed::UnknownOrigin => stats.unknown += 1,
                    Routed::Unchanged => stats.applied += 1,
                    Routed::Changed => {
                        stats.applied += 1;
                        stats.changed += 1;
                    }
                },
                Err(
                    broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed,
                ) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => stats.lagged = true,
            }
        }
        stats
    }
}

impl MultiCache {
    /// Объединённый список пространств всех инстансов с origin-метками.
    ///
    /// Порядок стабилен: домашний инстанс первым, затем удалённые по
    /// строковому представлению; внутри инстанса — по id пространства.
    #[must_use]
    pub fn spaces(&self) -> Vec<(&Origin, &SpaceWithChannelsAndMembers)> {
        self.collect_by_origin(|cache| {
            let mut list: Vec<&SpaceWithChannelsAndMembers> = cache.spaces().collect();
            list.sort_by(|a, b| a.space.id.cmp(&b.space.id));
            list
        })
    }

    /// Объединённый список каналов всех пространств всех инстансов.
    #[must_use]
    pub fn channels(&self) -> Vec<(&Origin, &Channel)> {
        self.collect_by_origin(|cache| {
            let mut list: Vec<&Channel> = cache.all_channels().collect();
            list.sort_by(|a, b| a.id.cmp(&b.id));
            list
        })
    }

    /// Объединённый список личных каналов (DM) всех инстансов.
    #[must_use]
    pub fn dm_channels(&self) -> Vec<(&Origin, &DmChannel)> {
        self.collect_by_origin(|cache| {
            let mut list: Vec<&DmChannel> = cache.dm_channels().collect();
            list.sort_by(|a, b| a.id.cmp(&b.id));
            list
        })
    }

    /// Суммарное число непрочитанных по всем инстансам.
    #[must_use]
    pub fn unread_total(&self) -> i64 {
        self.caches
            .values()
            .map(|cache| {
                cache
                    .all_channels()
                    .map(|channel| cache.unread_count(&channel.id))
                    .sum::<i64>()
            })
            .sum()
    }

    /// Суммарное число непрочитанных в конкретном инстансе.
    #[must_use]
    pub fn unread_total_in(&self, origin: &Origin) -> i64 {
        self.caches.get(origin).map_or(0, |cache| {
            cache
                .all_channels()
                .map(|channel| cache.unread_count(&channel.id))
                .sum()
        })
    }

    /// Профиль пользователя в инстансе.
    #[must_use]
    pub fn me(&self, origin: &Origin) -> Option<&bs_types::User> {
        self.caches.get(origin).map(Cache::me)
    }

    /// Сообщения канала в конкретном инстансе.
    #[must_use]
    pub fn messages(&self, origin: &Origin, channel_id: &str) -> &[MessageWithUser] {
        self.caches
            .get(origin)
            .map_or(&[], |cache| cache.messages(channel_id))
    }

    /// Сообщения личного канала в конкретном инстансе.
    #[must_use]
    pub fn dm_messages(
        &self,
        origin: &Origin,
        dm_channel_id: &str,
    ) -> &[bs_types::DmMessageWithUser] {
        self.caches
            .get(origin)
            .map_or(&[], |cache| cache.dm_messages(dm_channel_id))
    }

    /// Есть ли непрочитанные хотя бы в одном инстансе.
    #[must_use]
    pub fn has_unread(&self) -> bool {
        self.caches.values().any(|cache| {
            cache
                .all_channels()
                .any(|channel| cache.unread_count(&channel.id) > 0)
        })
    }

    /// Собирает плоский список `(origin, элемент)` по всем инстансам.
    fn collect_by_origin<'a, T, F>(&'a self, mut items: F) -> Vec<(&'a Origin, T)>
    where
        F: FnMut(&'a Cache) -> Vec<T>,
    {
        let mut out = Vec::new();
        for origin in self.origins() {
            let Some((key, cache)) = self.caches.get_key_value(&origin) else {
                continue;
            };
            for item in items(cache) {
                out.push((key, item));
            }
        }
        out
    }
}

/// Сортирует origin-ы: домашний первым, удалённые — по строке.
fn sort_origins(origins: &mut [Origin]) {
    origins.sort_by(|a, b| match (a.is_home(), b.is_home()) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.as_web_str().cmp(&b.as_web_str()),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bs_types::{ServerEvent, UserStatus};

    fn user(id: &str, name: &str) -> bs_types::User {
        serde_json::from_value(serde_json::json!({
            "id": id, "username": name, "displayName": null, "avatar": null,
            "banner": null, "accentColor": null, "avatarColor": null, "bio": null,
            "status": "online", "customStatus": null, "isAdmin": false,
            "createdAt": 1, "homeInstance": null, "homeUserId": null,
            "replicatedInstances": []
        }))
        .expect("user")
    }

    #[allow(clippy::needless_pass_by_value)] // удобно вызывать с литералами
    fn ready(me: bs_types::User, space: &str, channel: &str, unread: i64) -> ReadyPayload {
        serde_json::from_value(serde_json::json!({
            "user": me,
            "spaces": [{
                "id": space, "name": space, "ownerId": "u1", "visibility": "public",
                "directoryListed": false, "createdAt": 1,
                "channels": [{
                    "id": channel, "spaceId": space, "name": channel, "type": "text",
                    "position": 0, "createdAt": 1
                }],
                "categories": [], "members": [], "roles": []
            }],
            "dmChannels": [],
            "unreadCounts": { channel: unread }
        }))
        .expect("ready")
    }

    fn remote() -> Origin {
        Origin::parse_web_str("https://peer.example").expect("url")
    }

    fn empty_cache() -> Cache {
        Cache::from_ready(ready(user("u1", "alice"), "s", "c", 0))
    }

    #[test]
    fn same_ids_do_not_collide() {
        let mut multi = MultiCache::new();
        assert!(multi.insert_ready(Origin::Home, ready(user("u1", "alice"), "s1", "c1", 2)));
        assert!(multi.insert_ready(remote(), ready(user("u9", "bob"), "s1", "c1", 5)));

        assert_eq!(multi.len(), 2);
        assert_eq!(
            multi.me(&Origin::Home).map(|u| u.username.as_str()),
            Some("alice")
        );
        assert_eq!(
            multi.me(&remote()).map(|u| u.username.as_str()),
            Some("bob")
        );
        assert_eq!(multi.unread_total_in(&Origin::Home), 2);
        assert_eq!(multi.unread_total_in(&remote()), 5);
        assert_eq!(multi.unread_total(), 7);
        assert!(multi.has_unread());
    }

    #[test]
    fn origins_are_sorted_home_first() {
        let mut multi = MultiCache::new();
        let other = Origin::parse_web_str("https://aaa.example").expect("url");
        multi.insert_ready(remote(), ready(user("u2", "b"), "s", "c", 0));
        multi.insert_ready(other, ready(user("u3", "c"), "s", "c", 0));
        multi.insert_ready(Origin::Home, ready(user("u1", "a"), "s", "c", 0));

        let origins = multi.origins();
        assert_eq!(origins.first(), Some(&Origin::Home));
        assert_eq!(origins[1].as_web_str(), "https://aaa.example");
        assert_eq!(origins[2].as_web_str(), "https://peer.example");
    }

    #[test]
    fn apply_routes_only_to_named_origin() {
        let mut multi = MultiCache::new();
        multi.insert_ready(Origin::Home, ready(user("u1", "alice"), "s1", "c1", 0));
        multi.insert_ready(remote(), ready(user("u9", "bob"), "s1", "c1", 0));

        // Неизвестный инстанс — отброшено.
        let ghost = Origin::parse_web_str("https://ghost.example").expect("url");
        assert_eq!(
            multi.apply(
                &ghost,
                ServerEvent::UserUpdated {
                    user: user("u1", "alice")
                }
            ),
            Routed::UnknownOrigin
        );

        let event = ServerEvent::UserUpdated {
            user: user("u1", "alice").tap_status(UserStatus::Offline),
        };
        assert_eq!(multi.apply(&Origin::Home, event), Routed::Changed);
        assert_eq!(
            multi.cache(&Origin::Home).and_then(|c| c.user_status("u1")),
            Some(UserStatus::Offline)
        );
        // Соседний инстанс не тронут.
        assert_eq!(
            multi.cache(&remote()).and_then(|c| c.user_status("u1")),
            None
        );
    }

    #[test]
    fn merged_lists_keep_origin_labels() {
        let mut multi = MultiCache::new();
        multi.insert_ready(Origin::Home, ready(user("u1", "alice"), "s1", "c1", 0));
        multi.insert_ready(remote(), ready(user("u9", "bob"), "s1", "c1", 0));

        let spaces = multi.spaces();
        assert_eq!(spaces.len(), 2);
        assert_eq!(spaces[0].0, &Origin::Home);
        assert_eq!(spaces[1].0, &remote());

        let channels = multi.channels();
        assert_eq!(channels.len(), 2);
        assert_eq!(
            channels.iter().filter(|(o, _)| **o == Origin::Home).count(),
            1
        );
    }

    #[test]
    fn apply_event_ignores_connection_state() {
        let mut multi = MultiCache::new();
        multi.insert_ready(Origin::Home, ready(user("u1", "alice"), "s", "c", 0));

        let event = OriginEvent {
            origin: Origin::Home,
            event: InstanceEvent::StateChanged(bs_instances::ConnectionState::Connected),
        };
        assert_eq!(multi.apply_event(event), Routed::Unchanged);

        let unknown = OriginEvent {
            origin: remote(),
            event: InstanceEvent::AuthRejected {
                failure: bs_ws::AuthFailure::Invalid,
                message: "nope".into(),
                code: None,
            },
        };
        assert_eq!(multi.apply_event(unknown), Routed::UnknownOrigin);
    }

    #[test]
    fn drain_applies_buffered_events() {
        let mut multi = MultiCache::new();
        multi.insert_ready(Origin::Home, ready(user("u1", "alice"), "s", "c", 0));

        let (tx, mut rx) = broadcast::channel(8);
        tx.send(OriginEvent {
            origin: Origin::Home,
            event: InstanceEvent::Server(Box::new(ServerEvent::UserUpdated {
                user: user("u1", "alice").tap_status(UserStatus::Idle),
            })),
        })
        .expect("send");
        tx.send(OriginEvent {
            origin: remote(),
            event: InstanceEvent::Server(Box::new(ServerEvent::UserUpdated {
                user: user("u1", "alice"),
            })),
        })
        .expect("send");

        let stats = multi.drain(&mut rx, 16);
        assert_eq!(stats.applied, 1);
        assert_eq!(stats.changed, 1);
        assert_eq!(stats.unknown, 1);
        assert!(!stats.lagged);
        assert!(stats.is_changed());
        assert_eq!(
            multi.cache(&Origin::Home).and_then(|c| c.user_status("u1")),
            Some(UserStatus::Idle)
        );
    }

    #[test]
    fn drain_on_empty_stream_is_noop() {
        let mut multi = MultiCache::new();
        let (_tx, mut rx) = broadcast::channel::<OriginEvent>(4);
        let stats = multi.drain(&mut rx, 16);
        assert_eq!(stats, DrainStats::default());
        assert!(!stats.is_changed());
    }

    #[test]
    fn remove_drops_instance() {
        let mut multi =
            MultiCache::from_ready([(Origin::Home, ready(user("u1", "alice"), "s", "c", 0))]);
        assert!(multi.contains(&Origin::Home));
        assert!(multi.remove(&Origin::Home).is_some());
        assert!(multi.is_empty());
        assert!(!multi.has_unread());
        assert_eq!(multi.unread_total(), 0);
        assert!(multi.messages(&Origin::Home, "c").is_empty());
    }

    #[test]
    fn empty_multi_queries_are_safe() {
        let multi = MultiCache::new();
        assert!(multi.is_empty());
        assert!(multi.origins().is_empty());
        assert!(multi.spaces().is_empty());
        assert!(multi.channels().is_empty());
        assert!(multi.dm_channels().is_empty());
        assert!(multi.me(&Origin::Home).is_none());
        assert!(multi.cache(&Origin::Home).is_none());
        assert_eq!(multi.unread_total(), 0);
        assert_eq!(multi.unread_total_in(&Origin::Home), 0);
    }

    #[test]
    fn insert_cache_reports_replacement() {
        let mut multi = MultiCache::new();
        assert!(multi.insert_cache(Origin::Home, empty_cache()).is_none());
        assert!(multi.insert_cache(Origin::Home, empty_cache()).is_some());
        assert_eq!(multi.len(), 1);
    }

    trait TapStatus {
        fn tap_status(self, status: UserStatus) -> Self;
    }

    impl TapStatus for bs_types::User {
        fn tap_status(mut self, status: UserStatus) -> Self {
            self.status = status;
            self
        }
    }
}
