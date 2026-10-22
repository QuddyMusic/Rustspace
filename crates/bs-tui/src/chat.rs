//! Экран чата: выбор пространства/канала и композер.
//!
//! Как и [`crate::login`], модуль не знает ни про ratatui, ни про сеть —
//! только про [`Cache`]. Поэтому всё это тестируется без терминала.

use std::time::{SystemTime, UNIX_EPOCH};

use bs_core::Cache;
use bs_types::MessageWithUser;

/// Пространство в сайдбаре: id, имя и его каналы.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceEntry {
    /// Идентификатор пространства.
    pub id: String,
    /// Отображаемое имя.
    pub name: String,
    /// Каналы пространства в порядке показа.
    pub channels: Vec<ChannelEntry>,
}

/// Канал в сайдбаре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelEntry {
    /// Идентификатор канала.
    pub id: String,
    /// Отображаемое имя.
    pub name: String,
    /// Сколько непрочитанных числится за каналом.
    pub unread: i64,
}

/// Собирает сайдбар в детерминированном порядке.
///
/// `Cache` хранит пространства в `HashMap`, поэтому без сортировки список
/// прыгал бы между кадрами. Пространства — по имени, каналы — по `position`,
/// затем по имени.
pub fn sidebar(cache: &Cache) -> Vec<SpaceEntry> {
    let mut spaces: Vec<SpaceEntry> = cache
        .spaces()
        .map(|swc| {
            let mut channels: Vec<ChannelEntry> = swc
                .channels
                .iter()
                .map(|channel| ChannelEntry {
                    id: channel.id.clone(),
                    name: channel.name.clone(),
                    unread: cache.unread_count(&channel.id),
                })
                .collect();
            channels.sort_by(|a, b| a.name.cmp(&b.name));
            SpaceEntry {
                id: swc.space.id.clone(),
                name: swc.space.name.clone(),
                channels,
            }
        })
        .collect();
    spaces.sort_by(|a, b| a.name.cmp(&b.name));
    spaces
}

/// Текущее время в миллисекундах эпохи — для фильтра протухших typing.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Имена тех, кто сейчас печатает в канале.
pub fn typing_names(cache: &Cache, channel_id: &str) -> Vec<String> {
    cache
        .typing_in(channel_id, now_ms())
        .map(|entry| entry.username.clone())
        .collect()
}

/// Последние `limit` сообщений канала, готовые к показу.
///
/// Возвращает `(автор, текст)`; системные сообщения без контента пропускаются.
pub fn message_rows(cache: &Cache, channel_id: &str, limit: usize) -> Vec<(String, String)> {
    let all = cache.messages(channel_id);
    let start = all.len().saturating_sub(limit);
    all[start..].iter().filter_map(message_row).collect()
}

fn message_row(message: &MessageWithUser) -> Option<(String, String)> {
    let content = message.message.content.as_deref()?.trim();
    if content.is_empty() {
        return None;
    }
    Some((message.user.username.clone(), content.to_owned()))
}

/// Выбор в сайдбаре и текст композера.
#[derive(Debug, Clone, Default)]
pub struct ChatState {
    space: usize,
    channel: usize,
    composer: String,
    typing_sent_for: Option<String>,
}

impl ChatState {
    /// Пустое состояние: первое пространство, первый канал.
    pub fn new() -> Self {
        Self::default()
    }

    /// Индекс выбранного пространства.
    pub fn space_index(&self) -> usize {
        self.space
    }

    /// Индекс выбранного канала внутри пространства.
    pub fn channel_index(&self) -> usize {
        self.channel
    }

    /// Текст композера.
    pub fn composer(&self) -> &str {
        &self.composer
    }

    /// Дописывает символ в композер.
    pub fn push_char(&mut self, c: char) {
        if !c.is_control() {
            self.composer.push(c);
        }
    }

    /// Удаляет последний символ композера.
    pub fn backspace(&mut self) {
        self.composer.pop();
    }

    /// Забирает текст композера, очищая его. Пустой текст не отдаётся.
    pub fn take_composer(&mut self) -> Option<String> {
        let text = self.composer.trim().to_owned();
        self.composer.clear();
        self.typing_sent_for = None;
        if text.is_empty() { None } else { Some(text) }
    }

    /// Стоит ли отослать `typing_start` для канала: один раз на заполнение.
    pub fn should_send_typing(&mut self, channel_id: &str) -> bool {
        if self.composer.is_empty() || self.typing_sent_for.as_deref() == Some(channel_id) {
            return false;
        }
        self.typing_sent_for = Some(channel_id.to_owned());
        true
    }

    /// Пересчитывает выбор после смены данных: индексы зажимаются в границы.
    pub fn clamp(&mut self, cache: &Cache) {
        let spaces = sidebar(cache);
        if spaces.is_empty() {
            self.space = 0;
            self.channel = 0;
            return;
        }
        self.space = self.space.min(spaces.len() - 1);
        let channels = spaces[self.space].channels.len();
        self.channel = if channels == 0 {
            0
        } else {
            self.channel.min(channels - 1)
        };
    }

    /// Сдвигает выбор пространства на `delta` с зажимом в границы.
    pub fn move_space(&mut self, cache: &Cache, delta: isize) {
        let count = sidebar(cache).len();
        if count == 0 {
            return;
        }
        let next = self.space.saturating_add_signed(delta).min(count - 1);
        if next != self.space {
            self.space = next;
            self.channel = 0;
        }
    }

    /// Сдвигает выбор канала на `delta` с зажимом в границы.
    pub fn move_channel(&mut self, cache: &Cache, delta: isize) {
        let spaces = sidebar(cache);
        let Some(space) = spaces.get(self.space) else {
            return;
        };
        let count = space.channels.len();
        if count == 0 {
            return;
        }
        self.channel = self.channel.saturating_add_signed(delta).min(count - 1);
    }

    /// id выбранного канала.
    pub fn current_channel_id(&self, cache: &Cache) -> Option<String> {
        let spaces = sidebar(cache);
        let space = spaces.get(self.space)?;
        let channel = space.channels.get(self.channel)?;
        Some(channel.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bs_types::ReadyPayload;

    fn cache_from(json: serde_json::Value) -> Cache {
        let payload: ReadyPayload = serde_json::from_value(json).unwrap();
        Cache::from_ready(payload)
    }

    fn me() -> serde_json::Value {
        serde_json::json!({
            "id": "u1",
            "username": "me",
            "displayName": null,
            "avatar": null,
            "status": "online",
            "isAdmin": false,
            "createdAt": 0
        })
    }

    fn space(id: &str, name: &str, channels: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": name,
            "ownerId": "u1",
            "visibility": "private",
            "directoryListed": false,
            "createdAt": 0,
            "channels": channels,
            "categories": [],
            "members": [],
            "roles": []
        })
    }

    fn channel(id: &str, space_id: &str, name: &str, position: i64) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "spaceId": space_id,
            "name": name,
            "type": "text",
            "position": position,
            "createdAt": 0
        })
    }

    fn two_spaces() -> Cache {
        cache_from(serde_json::json!({
            "user": me(),
            "spaces": [
                space("s2", "Beta", &serde_json::json!([channel("c3", "s2", "general", 0)])),
                space("s1", "Alpha", &serde_json::json!([
                    channel("c2", "s1", "zeta", 1),
                    channel("c1", "s1", "alpha", 0)
                ]))
            ]
        }))
    }

    #[test]
    fn sidebar_sorts_spaces_and_channels() {
        let cache = two_spaces();
        let spaces = sidebar(&cache);
        let names: Vec<_> = spaces.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "Beta"]);
        let channels: Vec<_> = spaces[0].channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(channels, vec!["alpha", "zeta"]);
    }

    #[test]
    fn empty_cache_gives_empty_sidebar() {
        let cache = cache_from(serde_json::json!({ "user": me() }));
        assert!(sidebar(&cache).is_empty());
    }

    #[test]
    fn clamp_survives_shrinking_data() {
        let mut state = ChatState::new();
        let cache = two_spaces();
        state.space = 99;
        state.channel = 99;
        state.clamp(&cache);
        assert_eq!(state.space_index(), 1);
        assert_eq!(state.channel_index(), 0);
    }

    #[test]
    fn move_space_resets_channel_and_respects_bounds() {
        let cache = two_spaces();
        let mut state = ChatState::new();
        state.channel = 5;
        state.move_space(&cache, 1);
        assert_eq!(state.space_index(), 1);
        assert_eq!(state.channel_index(), 0);
        state.move_space(&cache, 1);
        assert_eq!(state.space_index(), 1);
        state.move_space(&cache, -5);
        assert_eq!(state.space_index(), 0);
    }

    #[test]
    fn move_channel_clamps_inside_space() {
        let cache = two_spaces();
        let mut state = ChatState::new();
        state.move_channel(&cache, 1);
        assert_eq!(state.channel_index(), 1);
        state.move_channel(&cache, 1);
        assert_eq!(state.channel_index(), 1);
        state.move_channel(&cache, -9);
        assert_eq!(state.channel_index(), 0);
    }

    #[test]
    fn current_channel_follows_selection() {
        let cache = two_spaces();
        let mut state = ChatState::new();
        assert_eq!(state.current_channel_id(&cache).as_deref(), Some("c1"));
        state.move_channel(&cache, 1);
        assert_eq!(state.current_channel_id(&cache).as_deref(), Some("c2"));
        state.move_space(&cache, 1);
        assert_eq!(state.current_channel_id(&cache).as_deref(), Some("c3"));
    }

    #[test]
    fn composer_ignores_control_chars_and_trims_on_send() {
        let mut state = ChatState::new();
        state.push_char('\n');
        state.push_char(' ');
        state.push_char('h');
        state.push_char('i');
        state.push_char(' ');
        assert_eq!(state.composer(), " hi ");
        assert_eq!(state.take_composer().as_deref(), Some("hi"));
        assert_eq!(state.composer(), "");
        assert!(state.take_composer().is_none());
    }

    #[test]
    fn typing_is_sent_once_per_channel_fill() {
        let mut state = ChatState::new();
        state.push_char('a');
        assert!(state.should_send_typing("c1"));
        assert!(!state.should_send_typing("c1"));
        state.take_composer();
        state.push_char('b');
        assert!(state.should_send_typing("c1"));
    }

    #[test]
    fn typing_not_sent_for_empty_composer() {
        let mut state = ChatState::new();
        assert!(!state.should_send_typing("c1"));
    }

    #[test]
    fn message_rows_skip_empty_content() {
        let cache = cache_from(serde_json::json!({ "user": me() }));
        assert!(message_rows(&cache, "c1", 10).is_empty());
        assert!(typing_names(&cache, "c1").is_empty());
    }
}
