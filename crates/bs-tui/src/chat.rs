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

/// Строка ленты сообщений, готовая к показу.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    /// Идентификатор сообщения.
    pub id: String,
    /// Имя автора.
    pub author: String,
    /// Текст сообщения.
    pub text: String,
    /// Сообщение было изменено после отправки.
    pub edited: bool,
    /// Автор — текущий пользователь (можно править и удалять).
    pub mine: bool,
}

/// Последние `limit` сообщений канала, готовые к показу.
///
/// `me` — id текущего пользователя, по нему заполняется [`MessageRow::mine`].
/// Системные сообщения без контента пропускаются.
pub fn message_rows(cache: &Cache, channel_id: &str, limit: usize, me: &str) -> Vec<MessageRow> {
    let all = cache.messages(channel_id);
    let start = all.len().saturating_sub(limit);
    all[start..]
        .iter()
        .filter_map(|message| message_row(message, me))
        .collect()
}

fn message_row(message: &MessageWithUser, me: &str) -> Option<MessageRow> {
    let content = message.message.content.as_deref()?.trim();
    if content.is_empty() {
        return None;
    }
    Some(MessageRow {
        id: message.message.id.clone(),
        author: message.user.username.clone(),
        text: content.to_owned(),
        edited: message.message.edited_at.is_some(),
        mine: message.user.id == me,
    })
}

/// Что делает Enter в композере.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ComposerMode {
    /// Обычное новое сообщение.
    #[default]
    New,
    /// Правка уже существующего сообщения.
    Edit {
        /// Идентификатор редактируемого сообщения.
        message_id: String,
    },
}

/// Действие, доступное по одиночной клавише при пустом композере.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatCommand {
    /// Начать правку выбранного своего сообщения.
    Edit,
    /// Удалить выбранное своё сообщение.
    Delete,
    /// Поставить реакцию на выбранное сообщение.
    React,
    /// Убрать реакцию с выбранного сообщения.
    Unreact,
    /// Подтвердить прочтение текущего канала.
    Ack,
}

/// Эмодзи, которым оперируют клавиши `+` и `-`.
pub const DEFAULT_REACTION: &str = "👍";

/// Раскладка одиночных клавиш в команды чата.
///
/// Работает только при пустом композере и вне режима правки — иначе те же
/// символы должны попадать в текст сообщения.
#[must_use]
pub fn chat_command(c: char) -> Option<ChatCommand> {
    match c {
        'e' => Some(ChatCommand::Edit),
        'd' => Some(ChatCommand::Delete),
        '+' => Some(ChatCommand::React),
        '-' => Some(ChatCommand::Unreact),
        'a' => Some(ChatCommand::Ack),
        _ => None,
    }
}

/// Выбор в сайдбаре, выбранное сообщение и текст композера.
#[derive(Debug, Clone, Default)]
pub struct ChatState {
    space: usize,
    channel: usize,
    composer: String,
    typing_sent_for: Option<String>,
    selected: Option<String>,
    mode: ComposerMode,
    last_ack: Option<(String, String)>,
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
            self.selected = None;
            self.cancel_edit();
            return;
        }
        self.space = self.space.min(spaces.len() - 1);
        let channels = spaces[self.space].channels.len();
        self.channel = if channels == 0 {
            0
        } else {
            self.channel.min(channels - 1)
        };
        self.clamp_selection(cache);
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
            self.reset_selection();
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
        let next = self.channel.saturating_add_signed(delta).min(count - 1);
        if next != self.channel {
            self.channel = next;
            self.reset_selection();
        }
    }

    /// id выбранного канала.
    pub fn current_channel_id(&self, cache: &Cache) -> Option<String> {
        let spaces = sidebar(cache);
        let space = spaces.get(self.space)?;
        let channel = space.channels.get(self.channel)?;
        Some(channel.id.clone())
    }

    /// id выбранного сообщения в текущем канале.
    pub fn selected_message_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Выбранное сообщение, если оно всё ещё в текущем канале.
    pub fn selected_message<'a>(&self, cache: &'a Cache) -> Option<&'a MessageWithUser> {
        let channel_id = self.current_channel_id(cache)?;
        let id = self.selected.as_deref()?;
        cache
            .messages(&channel_id)
            .iter()
            .find(|message| message.message.id == id)
    }

    /// Режим композера.
    pub fn mode(&self) -> &ComposerMode {
        &self.mode
    }

    /// Правится ли сейчас существующее сообщение.
    pub fn is_editing(&self) -> bool {
        matches!(self.mode, ComposerMode::Edit { .. })
    }

    /// id редактируемого сообщения, если идёт правка.
    pub fn editing_message_id(&self) -> Option<&str> {
        match &self.mode {
            ComposerMode::Edit { message_id } => Some(message_id),
            ComposerMode::New => None,
        }
    }

    /// Сдвигает выбор сообщения на `delta` с зажимом в границы ленты.
    ///
    /// Если ничего не выбрано, стартует с последнего сообщения канала.
    pub fn move_selection(&mut self, cache: &Cache, delta: isize) {
        let Some(channel_id) = self.current_channel_id(cache) else {
            self.selected = None;
            return;
        };
        let messages = cache.messages(&channel_id);
        if messages.is_empty() {
            self.selected = None;
            return;
        }
        let base = self
            .selected
            .as_deref()
            .and_then(|id| messages.iter().position(|m| m.message.id == id))
            .unwrap_or(messages.len() - 1);
        let next = base.saturating_add_signed(delta).min(messages.len() - 1);
        self.selected = Some(messages[next].message.id.clone());
    }

    /// Держит выбор сообщения в границах текущего канала.
    ///
    /// Если выбранное сообщение исчезло (удалено или сменился канал),
    /// выбирается последнее в ленте, а незавершённая правка отменяется.
    pub fn clamp_selection(&mut self, cache: &Cache) {
        let channel_id = self.current_channel_id(cache);
        let messages = channel_id
            .as_deref()
            .map_or(&[][..], |id| cache.messages(id));
        let still_there = self
            .selected
            .as_deref()
            .is_some_and(|id| messages.iter().any(|m| m.message.id == id));
        if !still_there {
            self.selected = messages.last().map(|m| m.message.id.clone());
        }
        let stale_edit = self.is_editing() && self.selected.as_deref() != self.editing_message_id();
        if stale_edit {
            self.cancel_edit();
        }
    }

    /// Забывает выбор сообщения и отменяет правку (при смене канала).
    pub fn reset_selection(&mut self) {
        self.selected = None;
        self.cancel_edit();
    }

    /// Переводит композер в режим правки сообщения `message_id`.
    pub fn begin_edit(&mut self, message_id: String, content: String) {
        self.composer = content;
        self.typing_sent_for = None;
        self.selected = Some(message_id.clone());
        self.mode = ComposerMode::Edit { message_id };
    }

    /// Выходит из режима правки, очищая композер.
    pub fn cancel_edit(&mut self) {
        if self.is_editing() {
            self.mode = ComposerMode::New;
            self.composer.clear();
            self.typing_sent_for = None;
        }
    }

    /// Возвращает композер в режим нового сообщения, не трогая текст.
    pub fn finish_edit(&mut self) {
        self.mode = ComposerMode::New;
    }

    /// Стоит ли подтвердить прочтение канала до `message_id`.
    ///
    /// Один и тот же `(канал, сообщение)` подтверждается ровно один раз.
    pub fn should_ack(&mut self, channel_id: &str, message_id: &str) -> bool {
        let key = (channel_id.to_owned(), message_id.to_owned());
        if self.last_ack.as_ref() == Some(&key) {
            return false;
        }
        self.last_ack = Some(key);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bs_core::apply;
    use bs_types::{ReadyPayload, ServerEvent};

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

    fn author(id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "username": id,
            "displayName": null,
            "avatar": null,
            "status": "online",
            "isAdmin": false,
            "createdAt": 0
        })
    }

    fn message_event(
        id: &str,
        channel_id: &str,
        author_id: &str,
        content: &str,
        edited_at: Option<i64>,
    ) -> ServerEvent {
        let message = serde_json::json!({
            "id": id,
            "channelId": channel_id,
            "userId": author_id,
            "content": content,
            "editedAt": edited_at,
            "createdAt": 1_750_000_000_000_i64,
            "user": author(author_id)
        });
        serde_json::from_value(serde_json::json!({
            "type": "message_created",
            "message": message
        }))
        .unwrap()
    }

    fn deleted_event(id: &str, channel_id: &str) -> ServerEvent {
        serde_json::from_value(serde_json::json!({
            "type": "message_deleted",
            "messageId": id,
            "channelId": channel_id
        }))
        .unwrap()
    }

    /// `c1` с четырьмя сообщениями: m1–m3 мои, m4 — чужие.
    fn cache_with_messages() -> Cache {
        let mut cache = two_spaces();
        for (id, content, edited) in [
            ("m1", "one", None),
            ("m2", "two", None),
            ("m3", "three", Some(1_750_000_001_000_i64)),
        ] {
            assert!(apply(
                &mut cache,
                message_event(id, "c1", "u1", content, edited)
            ));
        }
        assert!(apply(
            &mut cache,
            message_event("m4", "c1", "u2", "four", None)
        ));
        cache
    }

    #[test]
    fn message_rows_skip_empty_content() {
        let cache = cache_from(serde_json::json!({ "user": me() }));
        assert!(message_rows(&cache, "c1", 10, "u1").is_empty());
        assert!(typing_names(&cache, "c1").is_empty());
    }

    #[test]
    fn message_rows_report_author_ownership_and_edits() {
        let cache = cache_with_messages();
        let rows = message_rows(&cache, "c1", 10, "u1");
        let ids: Vec<_> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["m1", "m2", "m3", "m4"]);
        assert!(rows[0].mine && rows[1].mine && rows[2].mine);
        assert!(!rows[3].mine);
        assert_eq!(rows[3].author, "u2");
        assert!(!rows[0].edited);
        assert!(rows[2].edited);
    }

    #[test]
    fn message_rows_respect_limit() {
        let cache = cache_with_messages();
        let rows = message_rows(&cache, "c1", 2, "u1");
        let ids: Vec<_> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["m3", "m4"]);
    }

    #[test]
    fn selection_starts_at_last_message_and_clamps() {
        let cache = cache_with_messages();
        let mut state = ChatState::new();
        assert_eq!(state.selected_message_id(), None);
        state.move_selection(&cache, -1);
        assert_eq!(state.selected_message_id(), Some("m3"));
        state.move_selection(&cache, -99);
        assert_eq!(state.selected_message_id(), Some("m1"));
        state.move_selection(&cache, 99);
        assert_eq!(state.selected_message_id(), Some("m4"));
        assert!(state.selected_message(&cache).is_some());
    }

    #[test]
    fn selection_is_empty_without_messages() {
        let cache = two_spaces();
        let mut state = ChatState::new();
        state.move_selection(&cache, 1);
        assert_eq!(state.selected_message_id(), None);
        assert!(state.selected_message(&cache).is_none());
    }

    #[test]
    fn changing_channel_drops_selection_and_edit() {
        let cache = cache_with_messages();
        let mut state = ChatState::new();
        state.move_selection(&cache, -1);
        state.begin_edit("m3".to_owned(), "three".to_owned());
        assert!(state.is_editing());
        state.move_channel(&cache, 1);
        assert_eq!(state.selected_message_id(), None);
        assert!(!state.is_editing());
        assert_eq!(state.composer(), "");
    }

    #[test]
    fn begin_and_cancel_edit_manage_composer() {
        let mut state = ChatState::new();
        assert!(!state.is_editing());
        assert_eq!(state.editing_message_id(), None);
        state.begin_edit("m1".to_owned(), "one".to_owned());
        assert!(state.is_editing());
        assert_eq!(state.editing_message_id(), Some("m1"));
        assert_eq!(state.selected_message_id(), Some("m1"));
        assert_eq!(state.composer(), "one");
        state.push_char('!');
        state.cancel_edit();
        assert!(!state.is_editing());
        assert_eq!(state.composer(), "");
    }

    #[test]
    fn cancel_edit_is_noop_in_new_mode() {
        let mut state = ChatState::new();
        state.push_char('x');
        state.cancel_edit();
        assert_eq!(state.composer(), "x");
    }

    #[test]
    fn vanished_message_moves_selection_and_cancels_edit() {
        let mut cache = cache_with_messages();
        let mut state = ChatState::new();
        state.begin_edit("m3".to_owned(), "three".to_owned());
        assert!(apply(&mut cache, deleted_event("m3", "c1")));
        state.clamp(&cache);
        assert_eq!(state.selected_message_id(), Some("m4"));
        assert!(!state.is_editing());
        assert_eq!(state.composer(), "");
    }

    #[test]
    fn ack_is_sent_once_per_channel_message_pair() {
        let mut state = ChatState::new();
        assert!(state.should_ack("c1", "m1"));
        assert!(!state.should_ack("c1", "m1"));
        assert!(state.should_ack("c1", "m2"));
        assert!(state.should_ack("c2", "m1"));
    }

    #[test]
    fn chat_commands_map_single_keys() {
        assert_eq!(chat_command('e'), Some(ChatCommand::Edit));
        assert_eq!(chat_command('d'), Some(ChatCommand::Delete));
        assert_eq!(chat_command('+'), Some(ChatCommand::React));
        assert_eq!(chat_command('-'), Some(ChatCommand::Unreact));
        assert_eq!(chat_command('a'), Some(ChatCommand::Ack));
        assert_eq!(chat_command('x'), None);
        assert_eq!(chat_command('E'), None);
    }
}
