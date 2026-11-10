//! Экран социальных функций: друзья, заявки, поиск, личные каналы.
//!
//! Как и [`crate::chat`], модуль не знает ни про ratatui, ни про сеть:
//! он только раскладывает данные из [`Cache`] и результаты REST-поиска
//! в плоские строки, удобные для показа и тестирования.

use bs_core::Cache;
use bs_types::{DmChannel, Friend, FriendRequest, User};

/// Какая вкладка социального экрана активна.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SocialTab {
    /// Список друзей.
    #[default]
    Friends,
    /// Входящие и исходящие заявки.
    Requests,
    /// Поиск пользователей.
    Search,
    /// Личные каналы.
    Dms,
}

impl SocialTab {
    /// Подпись вкладки для заголовка.
    pub fn title(self) -> &'static str {
        match self {
            Self::Friends => "Друзья",
            Self::Requests => "Заявки",
            Self::Search => "Поиск",
            Self::Dms => "Личные",
        }
    }

    /// Следующая вкладка по кругу.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Friends => Self::Requests,
            Self::Requests => Self::Search,
            Self::Search => Self::Dms,
            Self::Dms => Self::Friends,
        }
    }

    /// Предыдущая вкладка по кругу.
    #[must_use]
    pub fn prev(self) -> Self {
        match self {
            Self::Friends => Self::Dms,
            Self::Requests => Self::Friends,
            Self::Search => Self::Requests,
            Self::Dms => Self::Search,
        }
    }
}

/// Строка списка друзей.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendRow {
    /// Идентификатор пользователя.
    pub id: String,
    /// Имя для показа.
    pub name: String,
    /// Однострочный статус (custom status или статус присутствия).
    pub status: String,
}

/// Строка списка заявок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRow {
    /// Идентификатор заявки.
    pub id: String,
    /// Имя собеседника.
    pub name: String,
    /// Заявка входящая (её можно принять).
    pub incoming: bool,
}

/// Строка списка личных каналов.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmRow {
    /// Идентификатор личного канала.
    pub id: String,
    /// Имя собеседника или название группы.
    pub name: String,
    /// Последнее сообщение, если есть.
    pub last: Option<String>,
}

/// Строка результатов поиска.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRow {
    /// Идентификатор пользователя.
    pub id: String,
    /// Имя для показа.
    pub name: String,
    /// Уже в друзьях.
    pub friend: bool,
}

/// Имя пользователя для показа: display name, иначе username.
fn display_name(user: &User) -> String {
    user.display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(user.username.as_str())
        .to_owned()
}

/// Статус друга одной строкой.
fn friend_status(friend: &Friend) -> String {
    friend
        .custom_status
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map_or_else(
            || format!("{:?}", friend.status).to_lowercase(),
            str::to_owned,
        )
}

/// Друзья из кэша в порядке показа.
pub fn friend_rows(cache: &Cache) -> Vec<FriendRow> {
    cache
        .friends()
        .map(|friend| FriendRow {
            id: friend.id.clone(),
            name: display_name_from_friend(friend),
            status: friend_status(friend),
        })
        .collect()
}

/// Имя друга для показа.
fn display_name_from_friend(friend: &Friend) -> String {
    friend
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(friend.username.as_str())
        .to_owned()
}

/// Заявки из кэша: сначала входящие, затем исходящие.
pub fn request_rows(cache: &Cache) -> Vec<RequestRow> {
    let me = cache.me().id.as_str();
    cache
        .friend_requests()
        .map(|request| RequestRow {
            id: request.id.clone(),
            name: request_name(request),
            incoming: request.to_id == me,
        })
        .collect()
}

fn request_name(request: &FriendRequest) -> String {
    request
        .user
        .as_ref()
        .map_or_else(|| request.id.clone(), display_name)
}

/// Личные каналы из кэша в порядке показа.
pub fn dm_rows(cache: &Cache) -> Vec<DmRow> {
    let me = cache.me().id.as_str();
    cache
        .dm_channels()
        .map(|channel| DmRow {
            id: channel.id.clone(),
            name: dm_name(channel, me),
            last: dm_last(channel),
        })
        .collect()
}

/// Имя личного канала: явное название группы или имена собеседников.
fn dm_name(channel: &DmChannel, me: &str) -> String {
    if let Some(name) = channel
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        return name.to_owned();
    }
    let others: Vec<String> = channel
        .members
        .iter()
        .filter(|member| member.id != me)
        .map(display_name)
        .collect();
    if others.is_empty() {
        channel.id.clone()
    } else {
        others.join(", ")
    }
}

fn dm_last(channel: &DmChannel) -> Option<String> {
    use bs_types::{DmChannelLastMessage, DmSystemEvent, GENERIC_SYSTEM_TEXT, MessageType};
    let (content, kind, author) = match channel.last_message.as_ref()? {
        DmChannelLastMessage::Preview(preview) => (
            preview.content.clone(),
            preview.message_type,
            preview.user_id.as_str(),
        ),
        DmChannelLastMessage::Full(message) => (
            message.message.content.clone(),
            message.message.message_type,
            message.message.user_id.as_str(),
        ),
    };
    if kind != Some(MessageType::System) {
        return content;
    }
    // Системное событие — текстом, а не сырым JSON.
    let actor = channel
        .members
        .iter()
        .find(|member| member.id == author)
        .map(display_name)
        .unwrap_or_default();
    Some(
        content
            .as_deref()
            .and_then(DmSystemEvent::parse)
            .map_or_else(
                || GENERIC_SYSTEM_TEXT.to_owned(),
                |event| event.text(&actor, false),
            ),
    )
}

/// Результаты поиска: пользователи, уже друзья, помечаются.
pub fn user_rows(cache: &Cache, users: &[User]) -> Vec<UserRow> {
    users
        .iter()
        .map(|user| UserRow {
            id: user.id.clone(),
            name: display_name(user),
            friend: cache.friend(&user.id).is_some(),
        })
        .collect()
}

/// Что выбрано на социальном экране.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocialSelection {
    /// Друг из списка друзей.
    Friend(String),
    /// Входящая заявка (её можно принять или отклонить).
    IncomingRequest(String),
    /// Исходящая заявка (её можно отозвать).
    OutgoingRequest(String),
    /// Пользователь из результатов поиска.
    User {
        /// Идентификатор пользователя.
        id: String,
        /// Уже в друзьях.
        friend: bool,
    },
    /// Личный канал.
    Dm(String),
}

/// Состояние социального экрана.
#[derive(Debug, Clone, Default)]
pub struct SocialState {
    tab: SocialTab,
    cursor: usize,
    query: String,
    search_results: Vec<User>,
    message: Option<String>,
}

impl SocialState {
    /// Пустое состояние на вкладке друзей.
    pub fn new() -> Self {
        Self::default()
    }

    /// Активная вкладка.
    pub fn tab(&self) -> SocialTab {
        self.tab
    }

    /// Переключает вкладку и сбрасывает курсор.
    pub fn set_tab(&mut self, tab: SocialTab) {
        if self.tab != tab {
            self.tab = tab;
            self.cursor = 0;
        }
    }

    /// Следующая вкладка.
    pub fn next_tab(&mut self) {
        self.tab = self.tab.next();
        self.cursor = 0;
    }

    /// Предыдущая вкладка.
    pub fn prev_tab(&mut self) {
        self.tab = self.tab.prev();
        self.cursor = 0;
    }

    /// Позиция курсора в активном списке.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Строка поиска.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Последнее сообщение экрана (ошибка или подтверждение).
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Показывает сообщение на экране.
    pub fn set_message(&mut self, message: Option<String>) {
        self.message = message;
    }

    /// Последние результаты поиска.
    pub fn search_results(&self) -> &[User] {
        &self.search_results
    }

    /// Заменяет результаты поиска и сбрасывает курсор.
    pub fn set_search_results(&mut self, users: Vec<User>) {
        self.search_results = users;
        self.cursor = 0;
    }

    /// Дописывает символ в строку поиска.
    pub fn push_query(&mut self, c: char) {
        if !c.is_control() {
            self.query.push(c);
        }
    }

    /// Удаляет последний символ строки поиска.
    pub fn backspace_query(&mut self) {
        self.query.pop();
    }

    /// Длина активного списка при текущих данных.
    pub fn len(&self, cache: &Cache) -> usize {
        match self.tab {
            SocialTab::Friends => friend_rows(cache).len(),
            SocialTab::Requests => request_rows(cache).len(),
            SocialTab::Search => self.search_results.len(),
            SocialTab::Dms => dm_rows(cache).len(),
        }
    }

    /// Сдвигает курсор на `delta` с зажимом в границы списка.
    pub fn move_cursor(&mut self, cache: &Cache, delta: isize) {
        let len = self.len(cache);
        if len == 0 {
            self.cursor = 0;
            return;
        }
        self.cursor = self.cursor.saturating_add_signed(delta).min(len - 1);
    }

    /// Держит курсор в границах после изменения данных.
    pub fn clamp(&mut self, cache: &Cache) {
        let len = self.len(cache);
        if len == 0 {
            self.cursor = 0;
        } else {
            self.cursor = self.cursor.min(len - 1);
        }
    }
}
