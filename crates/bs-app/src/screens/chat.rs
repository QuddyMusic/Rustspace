//! Основной экран в стиле Discord: колонка пространств, колонка каналов,
//! шапка и лента сообщений, поле ввода, панель участников.

use std::collections::HashMap;

use bs_core::Cache;
use bs_types::{ChannelId, UserStatus};
use egui::{Align, CornerRadius, Layout, Margin, RichText, Sense};

use crate::bridge::{BridgeRequest, ChannelKind, HistoryState};
use crate::fleet::Bridge;
use crate::icons::Icon;
use crate::logic::{
    author_name, connection_label, message_actions, outgoing_text, should_ack, should_load_older,
};
use crate::media::{self, AttachmentView};
use crate::screens::dialogs::{self, CreateSpaceForm, SettingsAction, SettingsState};
use crate::screens::home::{self, HomeState};
use crate::social::{dm_display_name, dm_last_at, dm_last_text};
use crate::theme;
use crate::widgets;

/// Что пользователь попросил сделать за пределами чата.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ChatAction {
    /// Ничего.
    #[default]
    None,
    /// Выйти из инстанса.
    Logout,
}

/// Выбранный канал.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedChannel {
    /// Инстанс, которому принадлежит канал (номер в мосте).
    pub(crate) origin: usize,
    /// Идентификатор канала внутри инстанса.
    pub(crate) channel: ChannelId,
    /// Канал пространства или личный канал: у них разные события и маршруты.
    pub(crate) kind: ChannelKind,
}

/// Канал в сайдбаре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarChannel {
    /// Идентификатор канала.
    pub(crate) id: String,
    /// Имя канала без `#`.
    pub(crate) name: String,
    /// Есть непрочитанные.
    pub(crate) unread: bool,
    /// Голосовой канал.
    pub(crate) voice: bool,
    /// Кто сейчас в голосовом канале.
    pub(crate) occupants: Vec<Occupant>,
}

/// Участник голосового канала в сайдбаре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Occupant {
    /// Имя.
    pub(crate) name: String,
    /// Микрофон выключен.
    pub(crate) muted: bool,
    /// Звук выключен.
    pub(crate) deafened: bool,
}

/// Группа каналов: категория или каналы без категории (`name == None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarGroup {
    /// Название категории.
    pub(crate) name: Option<String>,
    /// Каналы группы в порядке сервера.
    pub(crate) channels: Vec<SidebarChannel>,
}

/// Пространство в сайдбаре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarSpace {
    /// Инстанс, на котором живёт пространство.
    pub(crate) origin: usize,
    /// Идентификатор пространства (уникален только внутри инстанса).
    pub(crate) id: String,
    /// Имя пространства.
    pub(crate) name: String,
    /// Цвет значка из палитры сервера.
    pub(crate) color: Option<String>,
    /// Значение поля `icon` (картинка значка).
    pub(crate) icon: Option<String>,
    /// Каналы по категориям.
    pub(crate) groups: Vec<SidebarGroup>,
}

impl SidebarSpace {
    /// Все каналы пространства подряд.
    fn channels(&self) -> impl Iterator<Item = &SidebarChannel> {
        self.groups.iter().flat_map(|g| g.channels.iter())
    }

    /// Первый текстовый канал: его открываем по умолчанию.
    fn first_text(&self) -> Option<&SidebarChannel> {
        self.channels().find(|c| !c.voice)
    }

    /// Есть ли непрочитанное хотя бы в одном канале.
    fn has_unread(&self) -> bool {
        self.channels().any(|c| c.unread)
    }
}

/// Личный канал в сайдбаре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarDm {
    /// Инстанс, который доставляет эту беседу.
    pub(crate) origin: usize,
    /// Ключ беседы одинаков у всех её копий на разных инстансах.
    pub(crate) key: Option<String>,
    /// Идентификатор личного канала.
    pub(crate) id: String,
    /// Имя собеседника или название группы.
    pub(crate) label: String,
    /// Есть непрочитанные.
    pub(crate) unread: bool,
    /// Присутствие собеседника (только для беседы один на один).
    pub(crate) status: Option<UserStatus>,
    /// Начало последнего сообщения.
    pub(crate) preview: Option<String>,
    /// Время последнего сообщения (мс).
    pub(crate) at: Option<i64>,
    /// Аватар собеседника (беседа один на один) или иконка группы.
    pub(crate) avatar: Option<String>,
}

/// Кто сидит в голосовом канале, с флагами микрофона и звука.
fn occupants(cache: &Cache, names: &HashMap<String, String>, channel_id: &str) -> Vec<Occupant> {
    cache
        .users_in_voice(channel_id)
        .iter()
        .map(|id| {
            let state = cache.voice_user_state(id);
            Occupant {
                name: names.get(id).cloned().unwrap_or_else(|| id.clone()),
                muted: state.is_some_and(|s| s.is_muted),
                deafened: state.is_some_and(|s| s.is_deafened),
            }
        })
        .collect()
}

/// Собирает сайдбар: пространства, в них категории и каналы в порядке сервера.
#[must_use]
pub(crate) fn sidebar_items(cache: &Cache) -> Vec<SidebarSpace> {
    let names = name_map(cache);
    let mut spaces: Vec<_> = cache.spaces().collect();
    spaces.sort_by(|a, b| a.space.id.cmp(&b.space.id));
    spaces
        .into_iter()
        .map(|space| {
            let to_row = |channel: &bs_types::Channel| {
                let voice = channel.channel_type == bs_types::ChannelType::Voice;
                SidebarChannel {
                    id: channel.id.clone(),
                    name: channel.name.clone(),
                    unread: cache.unread_count(&channel.id) > 0,
                    voice,
                    occupants: if voice {
                        occupants(cache, &names, &channel.id)
                    } else {
                        Vec::new()
                    },
                }
            };
            let mut all: Vec<&bs_types::Channel> = cache.channels(&space.space.id).collect();
            all.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.id.cmp(&b.id)));

            let mut categories: Vec<_> = space.categories.iter().collect();
            categories.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.id.cmp(&b.id)));

            let mut groups = Vec::new();
            let loose: Vec<SidebarChannel> = all
                .iter()
                .filter(|c| {
                    c.category_id
                        .as_ref()
                        .is_none_or(|id| !categories.iter().any(|cat| &cat.id == id))
                })
                .map(|c| to_row(c))
                .collect();
            if !loose.is_empty() {
                groups.push(SidebarGroup {
                    name: None,
                    channels: loose,
                });
            }
            for category in categories {
                let channels: Vec<SidebarChannel> = all
                    .iter()
                    .filter(|c| c.category_id.as_ref() == Some(&category.id))
                    .map(|c| to_row(c))
                    .collect();
                groups.push(SidebarGroup {
                    name: Some(category.name.clone()),
                    channels,
                });
            }
            SidebarSpace {
                origin: 0,
                id: space.space.id.clone(),
                name: space.space.name.clone(),
                color: space.space.avatar_color.clone(),
                icon: space.space.icon.clone(),
                groups,
            }
        })
        .collect()
}

/// Личные каналы: свежие (больший snowflake) выше.
#[must_use]
pub(crate) fn dm_sidebar_items(cache: &Cache) -> Vec<SidebarDm> {
    let me = cache.me().id.clone();
    let mut dms: Vec<_> = cache.dm_channels().collect();
    dms.sort_by(|a, b| b.id.len().cmp(&a.id.len()).then_with(|| b.id.cmp(&a.id)));
    dms.into_iter()
        .map(|dm| SidebarDm {
            origin: 0,
            key: dm.federated_id.clone(),
            id: dm.id.clone(),
            label: dm_display_name(dm, &me),
            unread: cache.unread_count(&dm.id) > 0,
            status: {
                let mut others = dm.members.iter().filter(|m| m.id != me);
                match (others.next(), others.next()) {
                    (Some(other), None) => {
                        Some(cache.user_status(&other.id).unwrap_or(other.status))
                    }
                    _ => None,
                }
            },
            preview: dm_last_text(dm),
            at: dm_last_at(dm),
            avatar: {
                let mut others = dm.members.iter().filter(|m| m.id != me);
                match (others.next(), others.next()) {
                    (Some(other), None) => other.avatar.clone(),
                    _ => dm.icon.clone(),
                }
            },
        })
        .collect()
}

/// Пространства и личные беседы всех работающих инстансов.
///
/// Пространства идут по инстансам (домашний первым). Копии одной беседы на
/// разных инстансах (одинаковый ключ) сливаются в одну; показывается копия
/// домашнего инстанса, как в оригинале: туда приходят ответы собеседника.
/// Файлы получают полный адрес владельца, чтобы их можно было грузить, не
/// зная, чьё это значение.
#[must_use]
pub(crate) fn merged_sidebar(bridge: &Bridge) -> (Vec<SidebarSpace>, Vec<SidebarDm>) {
    let parts = bridge
        .live_origins()
        .into_iter()
        .filter_map(|origin| {
            let cache = bridge.cache_at(origin)?;
            let base = bridge.base_at(origin)?;
            Some((origin, base, sidebar_items(cache), dm_sidebar_items(cache)))
        })
        .collect();
    merge_origins(parts)
}

/// Склейка списков нескольких инстансов (см. [`merged_sidebar`]).
#[must_use]
fn merge_origins(
    parts: Vec<(usize, &str, Vec<SidebarSpace>, Vec<SidebarDm>)>,
) -> (Vec<SidebarSpace>, Vec<SidebarDm>) {
    let mut spaces = Vec::new();
    let mut dms: Vec<SidebarDm> = Vec::new();
    for (origin, base, origin_spaces, origin_dms) in parts {
        for mut space in origin_spaces {
            space.origin = origin;
            space.icon = space.icon.map(|v| crate::images::absolute(base, &v));
            spaces.push(space);
        }
        for mut dm in origin_dms {
            if let Some(key) = &dm.key
                && dms.iter().any(|known| known.key.as_ref() == Some(key))
            {
                continue;
            }
            dm.origin = origin;
            dm.avatar = dm.avatar.map(|v| crate::images::absolute(base, &v));
            dms.push(dm);
        }
    }
    dms.sort_by(|a, b| {
        b.at.cmp(&a.at)
            .then_with(|| b.id.len().cmp(&a.id.len()))
            .then_with(|| b.id.cmp(&a.id))
    });
    (spaces, dms)
}

/// Участник в правой панели.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemberRow {
    /// Идентификатор пользователя.
    pub(crate) user_id: String,
    /// Отображаемое имя (ник на пространстве, иначе имя профиля).
    pub(crate) name: String,
    /// Присутствие.
    pub(crate) status: UserStatus,
    /// Владелец пространства.
    pub(crate) owner: bool,
    /// Аватар.
    pub(crate) avatar: Option<String>,
}

/// Секция правой панели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemberSection {
    /// Владелец пространства.
    Owner,
    /// В сети (включая «неактивен» и «не беспокоить»).
    Online,
    /// Не в сети.
    Offline,
}

impl MemberRow {
    /// Секция, в которой участник показывается.
    pub(crate) const fn section(&self) -> MemberSection {
        if self.owner {
            MemberSection::Owner
        } else if matches!(self.status, UserStatus::Offline) {
            MemberSection::Offline
        } else {
            MemberSection::Online
        }
    }
}

impl MemberSection {
    /// Заголовок секции без числа.
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Owner => "Владелец",
            Self::Online => "В сети",
            Self::Offline => "Не в сети",
        }
    }
}

/// Порядок статусов в секции: онлайн, неактивен, не беспокоить, офлайн.
const fn status_rank(status: UserStatus) -> u8 {
    match status {
        UserStatus::Online => 0,
        UserStatus::Idle => 1,
        UserStatus::DoNotDisturb => 2,
        UserStatus::Offline => 3,
    }
}

/// Участники пространства: владелец, затем те, кто в сети, затем остальные;
/// внутри секции — по статусу и алфавиту.
#[must_use]
pub(crate) fn member_rows(cache: &Cache, space_id: &str) -> Vec<MemberRow> {
    let Some(space) = cache.space(space_id) else {
        return Vec::new();
    };
    let owner_id = &space.space.owner_id;
    let mut rows: Vec<MemberRow> = space
        .members
        .iter()
        .map(|m| MemberRow {
            user_id: m.user.id.clone(),
            name: m.member.nickname.clone().unwrap_or_else(|| {
                author_name(m.user.display_name.as_deref(), &m.user.username).to_owned()
            }),
            status: cache.user_status(&m.user.id).unwrap_or(m.user.status),
            owner: &m.user.id == owner_id,
            avatar: m.user.avatar.clone(),
        })
        .collect();
    rows.sort_by(|a, b| {
        (a.section() as u8)
            .cmp(&(b.section() as u8))
            .then_with(|| status_rank(a.status).cmp(&status_rank(b.status)))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    rows
}

/// Одна строка ленты: одинаковая для каналов и личных сообщений.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MessageRow {
    id: String,
    author_id: String,
    author: String,
    body: String,
    edited: bool,
    /// Время создания, мс с эпохи.
    created_at: i64,
    /// Аватар автора.
    avatar: Option<String>,
    /// Вложения.
    attachments: Vec<AttachmentView>,
    /// Реакции, сгруппированные по эмодзи.
    reactions: Vec<super::body::ReactionGroup>,
    /// Автор и текст сообщения, на которое это — ответ.
    reply: Option<(String, String)>,
}

/// Окно, в котором подряд идущие сообщения одного автора склеиваются.
const GROUP_WINDOW_MS: i64 = 5 * 60 * 1000;

/// Нужно ли показывать заголовок (аватар и имя) у `cur` после `prev`.
///
/// Заголовок нужен у первого сообщения, при смене автора, паузе дольше
/// пяти минут или смене календарного дня.
#[must_use]
fn needs_header(prev: Option<&MessageRow>, cur: &MessageRow) -> bool {
    let Some(prev) = prev else {
        return true;
    };
    prev.author_id != cur.author_id
        || cur.created_at.saturating_sub(prev.created_at) > GROUP_WINDOW_MS
        || theme::local_day(prev.created_at) != theme::local_day(cur.created_at)
}

/// Имена пользователей, известных кэшу: для упоминаний и подсказок реакций.
fn name_map(cache: &Cache) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for space in cache.spaces() {
        for member in &space.members {
            names.insert(
                member.user.id.clone(),
                author_name(member.user.display_name.as_deref(), &member.user.username).to_owned(),
            );
        }
    }
    for dm in cache.dm_channels() {
        for user in &dm.members {
            names.insert(
                user.id.clone(),
                author_name(user.display_name.as_deref(), &user.username).to_owned(),
            );
        }
    }
    let me = cache.me();
    names.insert(
        me.id.clone(),
        author_name(me.display_name.as_deref(), &me.username).to_owned(),
    );
    names
}

fn reaction_groups(
    reactions: &[bs_types::Reaction],
    names: &HashMap<String, String>,
    me: &str,
) -> Vec<super::body::ReactionGroup> {
    super::body::group_reactions(
        reactions.iter().map(|r| {
            (
                r.emoji.as_str(),
                r.user_id.as_str(),
                names.get(&r.user_id).map_or("?", String::as_str),
            )
        }),
        me,
    )
}

/// Строки ленты канала `channel_id` из кэша (старые → новые).
fn message_rows(
    cache: &Cache,
    kind: ChannelKind,
    channel_id: &str,
    names: &HashMap<String, String>,
) -> Vec<MessageRow> {
    let me = cache.me().id.as_str();
    let quote = |user: &bs_types::User, content: Option<&String>| {
        (
            author_name(user.display_name.as_deref(), &user.username).to_owned(),
            content.cloned().unwrap_or_default(),
        )
    };
    match kind {
        ChannelKind::Space => cache
            .messages(channel_id)
            .iter()
            .map(|msg| MessageRow {
                id: msg.message.id.clone(),
                author_id: msg.user.id.clone(),
                author: author_name(msg.user.display_name.as_deref(), &msg.user.username)
                    .to_owned(),
                body: msg.message.content.clone().unwrap_or_default(),
                edited: msg.message.edited_at.is_some(),
                created_at: msg.message.created_at,
                avatar: msg.user.avatar.clone(),
                attachments: msg
                    .attachments
                    .iter()
                    .map(AttachmentView::from_api)
                    .collect(),
                reactions: reaction_groups(&msg.reactions, names, me),
                reply: msg
                    .reply_to
                    .as_ref()
                    .map(|r| quote(&r.user, r.message.content.as_ref())),
            })
            .collect(),
        ChannelKind::Dm => cache
            .dm_messages(channel_id)
            .iter()
            .map(|msg| MessageRow {
                id: msg.message.id.clone(),
                author_id: msg.user.id.clone(),
                author: author_name(msg.user.display_name.as_deref(), &msg.user.username)
                    .to_owned(),
                body: msg.message.content.clone().unwrap_or_default(),
                edited: msg.message.edited_at.is_some(),
                created_at: msg.message.created_at,
                avatar: msg.user.avatar.clone(),
                attachments: msg
                    .attachments
                    .iter()
                    .map(AttachmentView::from_api)
                    .collect(),
                reactions: reaction_groups(&msg.reactions, names, me),
                reply: msg
                    .reply_to
                    .as_ref()
                    .map(|r| quote(&r.user, r.message.content.as_ref())),
            })
            .collect(),
    }
}

/// Подпись «кто печатает» по списку имён.
#[must_use]
fn typing_text(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [one] => Some(format!("{one} печатает…")),
        [a, b] => Some(format!("{a} и {b} печатают…")),
        _ => Some("Несколько человек печатают…".to_owned()),
    }
}

/// Текущее время, мс с эпохи.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Что показано в центральной области.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum View {
    /// Друзья и «Сейчас онлайн».
    #[default]
    Friends,
    /// Обзор пространств.
    Explore,
    /// Лента выбранного канала.
    Channel,
}

/// Переход, который пользователь запросил кликом; применяется после отрисовки.
#[derive(Debug)]
enum Nav {
    /// Главная: друзья.
    Friends,
    /// Обзор пространств.
    Explore,
    /// Открыть пространство.
    Space((usize, String)),
    /// Открыть канал или беседу.
    Channel(SelectedChannel),
    /// Окно «Создать пространство».
    CreateSpace,
    /// Экран настроек.
    Settings,
    /// Искать людей по строке (вкладка «Добавить»).
    SearchPeople(String),
    /// Войти в голосовой канал.
    VoiceJoin {
        origin: usize,
        channel_id: String,
        channel: String,
        space: String,
    },
    /// Действие панели голоса.
    Voice(crate::voice::DockAction),
}

/// Состояние экрана чата.
#[derive(Debug, Default)]
pub(crate) struct ChatScreen {
    /// Голосовой звонок.
    pub(crate) voice: crate::voice::VoiceState,
    /// Выбранный канал и его инстанс.
    pub(crate) selected_channel: Option<SelectedChannel>,
    pub(crate) draft: String,
    /// Вложения, эмодзи, GIF и стикеры.
    composer: super::composer::Composer,
    /// Что показано в центре.
    view: View,
    /// Состояние друзей и обзора.
    home: HomeState,
    /// Пространство, открытое в колонке каналов; `None` — главная.
    active_space: Option<(usize, String)>,
    /// Последний открытый канал в каждом пространстве.
    last_by_space: HashMap<(usize, String), String>,
    /// Строка поиска слева сверху: фильтр бесед и каналов.
    sidebar_filter: String,
    /// Открытое окно «Создать пространство».
    create_space: Option<CreateSpaceForm>,
    /// Открытые настройки.
    settings: Option<SettingsState>,
    /// Картинка, открытая на весь экран (значение поля).
    viewer: Option<String>,
    /// Короткое сообщение внизу экрана и момент, до которого оно видно.
    toast: Option<(String, f64)>,
    /// Первичная настройка уже выполнена.
    initialized: bool,
    /// Показывать ли панель участников.
    show_members: bool,
    /// Правка сообщения: его id и набранный текст.
    edit: Option<EditState>,
    /// Последняя подтверждённая пара «инстанс+канал, сообщение».
    last_ack: Option<(String, String)>,
    /// Сообщение, которое было самым старым в момент запроса истории:
    /// после подгрузки лента прокручивается к нему, чтобы не «прыгать».
    scroll_anchor: Option<String>,
    /// Листали ли ленту текущего канала вниз (см. `should_load_older`).
    was_scrolled_down: bool,
    /// Канал, для которого относятся `scroll_anchor` и `was_scrolled_down`.
    scroll_key: Option<String>,
    /// Картинка под курсором при открытии меню: сообщение и значение поля.
    menu_image: Option<(String, String)>,
    /// Открытое окно профиля.
    profile: Option<super::profile::ProfileState>,
    /// Сообщение, на которое пишется ответ.
    reply: Option<ReplyTarget>,
    /// Сообщение, для которого открыт выбор реакции, и вкладка выбора.
    react_picker: Option<(String, usize)>,
    /// Имена пользователей для упоминаний (обновляется каждый кадр).
    names: HashMap<String, String>,
}

/// На что отвечаем.
#[derive(Debug, Clone)]
struct ReplyTarget {
    message_id: String,
    author: String,
    preview: String,
}

/// Что именно правим.
#[derive(Debug, Clone)]
struct EditState {
    message_id: String,
}

impl ChatScreen {
    /// Сбрасывает всё, что относилось к прошлой сессии.
    pub(crate) fn reset(&mut self) {
        *self = Self {
            show_members: true,
            initialized: true,
            ..Self::default()
        };
    }

    /// Открывает сообщение в режиме правки.
    pub(crate) fn begin_edit(&mut self, message_id: String, content: &str) {
        self.draft.clear();
        self.draft.push_str(content);
        self.edit = Some(EditState { message_id });
    }

    /// Отменяет правку и очищает поле.
    pub(crate) fn cancel_edit(&mut self) {
        self.edit = None;
        self.draft.clear();
    }

    /// `true`, если сейчас правится сообщение.
    pub(crate) fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// Открывает канал и переключает центр на ленту.
    pub(crate) fn open_channel(&mut self, selected: SelectedChannel) {
        // Правка относилась к прежнему каналу.
        if self.selected_channel.as_ref() != Some(&selected) {
            self.edit = None;
            self.reply = None;
            self.react_picker = None;
        }
        let space_open = self
            .active_space
            .as_ref()
            .is_some_and(|(origin, _)| *origin == selected.origin);
        if selected.kind == ChannelKind::Dm {
            self.active_space = None;
        } else if let Some(space) = self.active_space.as_ref().filter(|_| space_open) {
            self.last_by_space
                .insert(space.clone(), selected.channel.clone());
        }
        self.selected_channel = Some(selected);
        self.view = View::Channel;
    }

    /// Показывает пространство: запомненный канал или первый текстовый.
    fn open_space(&mut self, space: &SidebarSpace) {
        let key = (space.origin, space.id.clone());
        self.active_space = Some(key.clone());
        let remembered = self
            .last_by_space
            .get(&key)
            .filter(|id| space.channels().any(|c| &c.id == *id))
            .cloned()
            .or_else(|| space.first_text().map(|c| c.id.clone()));
        if let Some(channel) = remembered {
            self.open_channel(SelectedChannel {
                origin: space.origin,
                channel,
                kind: ChannelKind::Space,
            });
        } else {
            self.selected_channel = None;
            self.view = View::Channel;
        }
    }

    /// Выбирает инстанс, которому адресованы действия этого кадра: владельца
    /// открытого канала или пространства (иначе домашний). Если владелец
    /// пропал (отключён, потерял соединение), выбор сбрасывается.
    fn route_actions(&mut self, bridge: &mut Bridge, ctx: &egui::Context) {
        let wanted = match self.view {
            View::Channel => self.selected_channel.as_ref().map(|s| s.origin),
            View::Friends | View::Explore => self.active_space.as_ref().map(|(o, _)| *o),
        }
        .unwrap_or(0);
        bridge.focus_on(wanted);
        if bridge.focus_index() != wanted {
            self.selected_channel = None;
            self.active_space = None;
            self.view = View::Friends;
            bridge.focus_on(0);
        }
        if let Some(hub) = crate::images::ImageHub::of(ctx) {
            hub.set_current(bridge.base_at(bridge.focus_index()));
        }
    }

    /// Применяет запрошенный кликом переход.
    fn apply_nav(
        &mut self,
        nav: Nav,
        items: &[SidebarSpace],
        bridge: &mut Bridge,
        ctx: &egui::Context,
    ) {
        match nav {
            Nav::Friends => {
                self.active_space = None;
                self.view = View::Friends;
                bridge.request(BridgeRequest::Friends);
            }
            Nav::Explore => {
                self.active_space = None;
                self.view = View::Explore;
                bridge.load_explore(&self.home.explore_query);
            }
            Nav::Space((origin, id)) => {
                if let Some(space) = items.iter().find(|s| s.origin == origin && s.id == id) {
                    self.open_space(space);
                }
            }
            Nav::Channel(selected) => self.open_channel(selected),
            Nav::CreateSpace => self.create_space = Some(CreateSpaceForm::default()),
            Nav::Settings => {
                if let Some(cache) = bridge.cache() {
                    self.settings = Some(SettingsState::from_user(cache.me(), ctx.zoom_factor()));
                }
            }
            Nav::VoiceJoin {
                origin,
                channel_id,
                channel,
                space,
            } => {
                let same = self
                    .voice
                    .call
                    .as_ref()
                    .is_some_and(|c| c.origin == origin && c.channel_id == channel_id);
                if same {
                    self.voice.leave(bridge);
                } else {
                    self.voice
                        .join(bridge, origin, &channel_id, &channel, &space);
                }
            }
            Nav::Voice(action) => self.voice.apply(action, bridge),
            Nav::SearchPeople(query) => {
                self.active_space = None;
                self.view = View::Friends;
                self.home.start_search(&query, bridge);
                self.sidebar_filter.clear();
            }
        }
    }

    /// Рисует экран. Возвращает действие, которое нужно выполнить снаружи.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge) -> ChatAction {
        let mut action = ChatAction::None;
        if !self.initialized {
            self.initialized = true;
            self.show_members = true;
        }
        self.home.ensure_loaded(bridge);
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        if let Some(info) = bridge.take_info() {
            self.toast = Some((info, now + 3.0));
        }

        self.route_actions(bridge, &ctx);
        self.voice.poll(bridge);
        if let Some(text) = self.voice.notice.take() {
            self.toast = Some((text, now + 3.0));
        }
        let (items, dms) = merged_sidebar(bridge);

        if let Some(opened) = bridge.take_opened_space() {
            self.create_space = None;
            self.apply_nav(Nav::Space(opened), &items, bridge, &ctx);
            self.route_actions(bridge, &ctx);
        }

        if let Some(state) = self.settings.as_mut() {
            match dialogs::show_settings(ui, state, bridge) {
                SettingsAction::None => {}
                SettingsAction::Close => self.settings = None,
                SettingsAction::Logout => {
                    self.settings = None;
                    action = ChatAction::Logout;
                }
                SettingsAction::Connections(chosen) => route_connection(bridge, chosen),
            }
            self.show_toast(&ctx, now);
            return action;
        }

        let mut nav: Option<Nav> = None;
        let hosts: Vec<String> = (0..=bridge.live_origins().last().copied().unwrap_or(0))
            .map(|i| {
                bridge
                    .base_at(i)
                    .map(bs_identity::identity::host_of)
                    .unwrap_or_default()
            })
            .collect();
        self.show_rail(ui, &items, &dms, &hosts, &mut nav);
        self.show_left_column(ui, bridge, &items, &dms, &mut nav);

        match self.view {
            View::Friends => {
                home::show_active_now(ui, bridge);
                home::show_friends(ui, &mut self.home, bridge);
            }
            View::Explore => {
                let mut create = false;
                if let Some(id) = home::show_explore(ui, &mut self.home, bridge, &mut create) {
                    nav = Some(Nav::Space((0, id)));
                }
                if create {
                    nav = Some(Nav::CreateSpace);
                }
            }
            View::Channel => {
                if self.show_members
                    && let Some(space_id) = self.active_space.clone()
                {
                    if let Some((id, name)) = Self::show_member_panel(ui, bridge, &space_id.1) {
                        self.profile = Some(super::profile::ProfileState::open(bridge, &id, &name));
                    }
                }
                self.show_header(ui, bridge);
                self.show_composer(ui, bridge);
                self.show_messages(ui, bridge);
            }
        }

        if let Some(value) = self.viewer.clone()
            && media::viewer(&ctx, &value)
        {
            self.viewer = None;
        }
        if let Some(state) = self.profile.as_mut() {
            state.poll(bridge);
            let me = bridge
                .cache()
                .map(|cache| cache.me().id.clone())
                .unwrap_or_default();
            if super::profile::show(&ctx, state, bridge, &me)
                == super::profile::ProfileAction::Close
            {
                self.profile = None;
            }
        }
        if let Some(form) = self.create_space.as_mut()
            && !dialogs::show_create_space(&ctx, form, bridge)
        {
            self.create_space = None;
        }
        self.show_toast(&ctx, now);
        if let Some(nav) = nav {
            self.apply_nav(nav, &items, bridge, &ctx);
        }
        action
    }

    /// Всплывающее сообщение внизу по центру.
    fn show_toast(&mut self, ctx: &egui::Context, now: f64) {
        let Some((text, until)) = &self.toast else {
            return;
        };
        if now >= *until {
            self.toast = None;
            return;
        }
        egui::Area::new(egui::Id::unique("toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -28.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::BG_BUBBLE)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).color(theme::TEXT_BRIGHT));
                    });
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    /// Колонка пространств: «домой», пространства, «создать», «обзор».
    fn show_rail(
        &self,
        ui: &mut egui::Ui,
        items: &[SidebarSpace],
        dms: &[SidebarDm],
        bridge_hosts: &[String],
        nav: &mut Option<Nav>,
    ) {
        let dm_unread = dms.iter().any(|d| d.unread);
        egui::Panel::left("rail")
            .exact_size(72.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_RAIL)
                    .inner_margin(Margin::symmetric(0, 10)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("rail_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        let home = self.active_space.is_none() && self.view == View::Friends;
                        if theme::rail_home(ui, home, dm_unread)
                            .on_hover_text("Главная")
                            .clicked()
                        {
                            *nav = Some(Nav::Friends);
                        }
                        theme::rail_separator(ui);
                        for space in items {
                            let selected = self
                                .active_space
                                .as_ref()
                                .is_some_and(|(o, id)| *o == space.origin && *id == space.id);
                            let color = space
                                .color
                                .as_deref()
                                .and_then(theme::palette)
                                .unwrap_or_else(|| theme::color_for(&space.name));
                            let response = theme::rail_button(
                                ui,
                                &theme::initials(&space.name),
                                color,
                                selected,
                                space.has_unread(),
                                space.icon.as_deref(),
                            );
                            let owner = bridge_hosts.get(space.origin).cloned().unwrap_or_default();
                            if space.origin != 0 {
                                widgets::origin_badge(ui, response.rect, &owner);
                            }
                            let hover = if space.origin == 0 {
                                space.name.clone()
                            } else {
                                format!("{} · {owner}", space.name)
                            };
                            if response.on_hover_text(hover).clicked() && !selected {
                                *nav = Some(Nav::Space((space.origin, space.id.clone())));
                            }
                        }
                        if widgets::rail_action(ui, Icon::Plus, theme::MINT, false)
                            .on_hover_text("Создать пространство")
                            .clicked()
                        {
                            *nav = Some(Nav::CreateSpace);
                        }
                        if widgets::rail_action(
                            ui,
                            Icon::Compass,
                            theme::SKY,
                            self.view == View::Explore,
                        )
                        .on_hover_text("Обзор пространств")
                        .clicked()
                        {
                            *nav = Some(Nav::Explore);
                        }
                    });
            });
    }

    /// Вторая колонка: поиск, навигация, беседы или каналы, панель пользователя.
    fn show_left_column(
        &mut self,
        ui: &mut egui::Ui,
        bridge: &Bridge,
        items: &[SidebarSpace],
        dms: &[SidebarDm],
        nav: &mut Option<Nav>,
    ) {
        egui::Panel::left("channels")
            .resizable(true)
            .default_size(250.0)
            .size_range(200.0..=360.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(theme::BG_SIDEBAR))
            .show(ui, |ui| {
                egui::Panel::top("sidebar_search")
                    .exact_size(52.0)
                    .resizable(false)
                    .show_separator_line(false)
                    .frame(
                        egui::Frame::new()
                            .fill(theme::BG_SIDEBAR)
                            .inner_margin(Margin::symmetric(10, 10)),
                    )
                    .show(ui, |ui| {
                        let hint = if self.active_space.is_some() {
                            "Найти канал"
                        } else {
                            "Найти или начать беседу"
                        };
                        let response = widgets::search_box(ui, &mut self.sidebar_filter, hint);
                        let submitted =
                            response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if submitted
                            && self.active_space.is_none()
                            && !self.sidebar_filter.trim().is_empty()
                        {
                            *nav = Some(Nav::SearchPeople(self.sidebar_filter.trim().to_owned()));
                        }
                    });

                Self::show_user_bar(ui, bridge, nav);
                let dock = crate::voice::show_dock(ui, &self.voice);
                if dock != crate::voice::DockAction::None {
                    *nav = Some(Nav::Voice(dock));
                }

                let filter = self.sidebar_filter.trim().to_lowercase();
                egui::ScrollArea::vertical()
                    .id_salt("channel_list")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if let Some(space) = self.active_space.as_ref().and_then(|(origin, id)| {
                            items.iter().find(|s| s.origin == *origin && &s.id == id)
                        }) {
                            self.channel_list(ui, space, &filter, nav);
                        } else {
                            self.home_list(ui, dms, &filter, nav);
                        }
                    });
            });
    }

    /// Каналы пространства по категориям.
    fn channel_list(
        &self,
        ui: &mut egui::Ui,
        space: &SidebarSpace,
        filter: &str,
        nav: &mut Option<Nav>,
    ) {
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(
                RichText::new(theme::truncate_chars(&space.name, 24))
                    .size(15.5)
                    .family(theme::bold_family())
                    .color(theme::TEXT_BRIGHT),
            );
        });
        ui.add_space(6.0);
        for group in &space.groups {
            let visible: Vec<_> = group
                .channels
                .iter()
                .filter(|c| filter.is_empty() || c.name.to_lowercase().contains(filter))
                .collect();
            if visible.is_empty() {
                continue;
            }
            if let Some(name) = &group.name {
                theme::category_header(ui, name);
            }
            for channel in visible {
                if channel.voice {
                    let active = self.voice.call.as_ref().is_some_and(|c| {
                        (c.origin, c.channel_id.as_str()) == (space.origin, channel.id.as_str())
                    });
                    if theme::voice_row(ui, &channel.name, active)
                        .on_hover_text(if active {
                            "Нажмите, чтобы отключиться"
                        } else {
                            "Нажмите, чтобы подключиться"
                        })
                        .clicked()
                    {
                        *nav = Some(Nav::VoiceJoin {
                            origin: space.origin,
                            channel_id: channel.id.clone(),
                            channel: channel.name.clone(),
                            space: space.name.clone(),
                        });
                    }
                    for occupant in &channel.occupants {
                        theme::occupant_row(ui, &occupant.name, occupant.muted, occupant.deafened);
                    }
                    continue;
                }
                let selected = self.view == View::Channel
                    && self.selected_channel.as_ref().is_some_and(|current| {
                        current.kind == ChannelKind::Space
                            && current.origin == space.origin
                            && current.channel.as_str() == channel.id
                    });
                if theme::list_row(
                    ui,
                    &format!("#   {}", channel.name),
                    selected,
                    channel.unread,
                )
                .clicked()
                {
                    *nav = Some(Nav::Channel(SelectedChannel {
                        origin: space.origin,
                        channel: channel.id.clone(),
                        kind: ChannelKind::Space,
                    }));
                }
            }
        }
    }

    /// Главная: «Друзья», «Обзор» и личные беседы.
    fn home_list(&self, ui: &mut egui::Ui, dms: &[SidebarDm], filter: &str, nav: &mut Option<Nav>) {
        if widgets::nav_row(ui, Icon::User, "Друзья", self.view == View::Friends).clicked() {
            *nav = Some(Nav::Friends);
        }
        if widgets::nav_row(ui, Icon::Compass, "Обзор", self.view == View::Explore).clicked() {
            *nav = Some(Nav::Explore);
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            widgets::section_label(ui, "Личные сообщения");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(10.0);
                if ui
                    .add(widgets::flat_icon(Icon::Plus, 18.0, theme::TEXT_MUTED))
                    .on_hover_text("Найти человека")
                    .clicked()
                {
                    *nav = Some(Nav::SearchPeople(String::new()));
                }
            });
        });
        ui.add_space(4.0);
        let now = now_ms();
        let mut shown = 0;
        for dm in dms
            .iter()
            .filter(|d| filter.is_empty() || d.label.to_lowercase().contains(filter))
        {
            shown += 1;
            let selected = self.view == View::Channel
                && self.selected_channel.as_ref().is_some_and(|current| {
                    current.kind == ChannelKind::Dm
                        && current.origin == dm.origin
                        && current.channel.as_str() == dm.id
                });
            let stamp = dm.at.map(|at| theme::short_stamp(at, now));
            let response = widgets::dm_entry(
                ui,
                &widgets::DmEntry {
                    name: &dm.label,
                    status: dm.status,
                    preview: dm.preview.as_deref(),
                    stamp: stamp.as_deref(),
                    selected,
                    unread: dm.unread,
                    avatar: dm.avatar.as_deref(),
                },
            );
            if response.clicked() {
                *nav = Some(Nav::Channel(SelectedChannel {
                    origin: dm.origin,
                    channel: dm.id.clone(),
                    kind: ChannelKind::Dm,
                }));
            }
        }
        if shown == 0 {
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                ui.label(theme::muted(if filter.is_empty() {
                    "Бесед пока нет"
                } else {
                    "Нет совпадений — Enter, чтобы искать людей"
                }));
            });
        }
    }

    /// Нижняя панель колонки: «я», статус соединения и кнопка настроек.
    fn show_user_bar(ui: &mut egui::Ui, bridge: &Bridge, nav: &mut Option<Nav>) {
        let (name, handle, status, my_avatar) = bridge.cache_at(0).map_or_else(
            || (String::new(), String::new(), UserStatus::Online, None),
            |cache| {
                let me = cache.me();
                (
                    author_name(me.display_name.as_deref(), &me.username).to_owned(),
                    format!("@{}", me.username),
                    cache.user_status(&me.id).unwrap_or(me.status),
                    me.avatar.clone().map(|v| {
                        bridge
                            .base_at(0)
                            .map_or_else(|| v.clone(), |base| crate::images::absolute(base, &v))
                    }),
                )
            },
        );
        let connected = *bridge.connection() == bs_instances::ConnectionState::Connected;
        egui::Panel::bottom("user_bar")
            .exact_size(66.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_SIDEBAR)
                    .inner_margin(Margin::symmetric(10, 8)),
            )
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(theme::BG_RAIL)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::symmetric(10, 0))
                    .show(ui, |ui| {
                        ui.set_height(48.0);
                        ui.set_width(ui.available_width());
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            theme::avatar(ui, &name, 32.0, Some(status), my_avatar.as_deref());
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new(theme::truncate_chars(&name, 13))
                                        .size(14.0)
                                        .family(theme::bold_family())
                                        .color(theme::TEXT_BRIGHT),
                                );
                                let sub = if connected {
                                    handle
                                } else {
                                    connection_label(bridge.connection())
                                };
                                ui.label(theme::muted(theme::truncate_chars(&sub, 18)).size(11.0));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if widgets::icon_button(ui, Icon::Gear, 32.0)
                                    .on_hover_text("Настройки")
                                    .clicked()
                                {
                                    *nav = Some(Nav::Settings);
                                }
                            });
                        });
                    });
            });
    }

    /// Правая панель: участники пространства по секциям.
    fn show_member_panel(
        ui: &mut egui::Ui,
        bridge: &Bridge,
        space_id: &str,
    ) -> Option<(String, String)> {
        let mut clicked = None;
        let rows = bridge
            .cache()
            .map(|cache| member_rows(cache, space_id))
            .unwrap_or_default();
        egui::Panel::right("members")
            .exact_size(236.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_SIDEBAR)
                    .inner_margin(Margin::symmetric(10, 16)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("members_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut current: Option<MemberSection> = None;
                        for row in &rows {
                            let section = row.section();
                            if current != Some(section) {
                                if current.is_some() {
                                    ui.add_space(12.0);
                                }
                                let count = rows.iter().filter(|r| r.section() == section).count();
                                ui.horizontal(|ui| {
                                    ui.add_space(8.0);
                                    ui.label(theme::section_title(&format!(
                                        "{} — {count}",
                                        section.title()
                                    )));
                                });
                                ui.add_space(4.0);
                                current = Some(section);
                            }
                            if theme::member_row(ui, &row.name, row.status, row.avatar.as_deref())
                                .clicked()
                            {
                                clicked = Some((row.user_id.clone(), row.name.clone()));
                            }
                        }
                    });
            });
        clicked
    }

    /// Шапка ленты: название канала, тема, переключатель участников.
    fn show_header(&mut self, ui: &mut egui::Ui, bridge: &Bridge) {
        let (prefix, title, topic) = self.selected_channel.as_ref().map_or_else(
            || ("", String::new(), None),
            |selected| {
                let Some(cache) = bridge.cache() else {
                    return ("", String::new(), None);
                };
                match selected.kind {
                    ChannelKind::Space => cache.channel(&selected.channel).map_or_else(
                        || ("", String::new(), None),
                        |c| ("#", c.name.clone(), c.topic.clone()),
                    ),
                    ChannelKind::Dm => cache.dm_channel(&selected.channel).map_or_else(
                        || ("", String::new(), None),
                        |dm| ("@", dm_display_name(dm, &cache.me().id), None),
                    ),
                }
            },
        );
        let has_members = self.active_space.is_some();
        egui::Panel::top("chat_header")
            .exact_size(52.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_CHAT)
                    .inner_margin(Margin::symmetric(20, 0)),
            )
            .show(ui, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.label(RichText::new(prefix).size(20.0).color(theme::TEXT_FAINT));
                    ui.label(
                        RichText::new(&title)
                            .family(theme::bold_family())
                            .size(16.5)
                            .color(theme::TEXT_BRIGHT),
                    );
                    if let Some(topic) = topic.filter(|t| !t.trim().is_empty()) {
                        ui.add_space(6.0);
                        ui.label(theme::muted(theme::truncate_chars(&topic, 80)).size(13.0));
                    }
                    if has_members {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let color = if self.show_members {
                                theme::TEXT_BRIGHT
                            } else {
                                theme::TEXT_FAINT
                            };
                            if ui
                                .add(widgets::flat_icon(Icon::Users, 22.0, color))
                                .on_hover_text(if self.show_members {
                                    "Скрыть участников"
                                } else {
                                    "Показать участников"
                                })
                                .clicked()
                            {
                                self.show_members = !self.show_members;
                            }
                        });
                    }
                });
            });
    }
}

/// Строка состояния истории над лентой. Возвращает `true`, если нажали «Повторить».
fn show_history_header(ui: &mut egui::Ui, history: &HistoryState) -> bool {
    if history.loading {
        ui.vertical_centered(|ui| {
            ui.label(theme::muted("Загрузка истории…"));
        });
    } else if let Some(error) = &history.error {
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(format!("Не удалось загрузить историю: {error}")).color(theme::ROSE),
            );
        });
        return ui.button("Повторить").clicked();
    } else if history.exhausted {
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            ui.label(
                RichText::new("Это начало беседы")
                    .size(18.0)
                    .family(theme::bold_family())
                    .color(theme::TEXT_BRIGHT),
            );
            ui.label(theme::muted("Здесь пока всё, что было написано раньше."));
            ui.add_space(16.0);
        });
    }
    false
}

impl ChatScreen {
    fn show_composer(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge) {
        let hint = match (&self.selected_channel, bridge.cache()) {
            (Some(selected), Some(cache)) => match selected.kind {
                ChannelKind::Space => cache.channel(&selected.channel).map_or_else(
                    || "Сообщение".to_owned(),
                    |c| format!("Написать в #{}", c.name),
                ),
                ChannelKind::Dm => cache.dm_channel(&selected.channel).map_or_else(
                    || "Сообщение".to_owned(),
                    |dm| format!("Написать {}", dm_display_name(dm, &cache.me().id)),
                ),
            },
            _ => "Сообщение".to_owned(),
        };
        let typing = self.selected_channel.as_ref().and_then(|selected| {
            let cache = bridge.cache()?;
            let me = &cache.me().id;
            let names: Vec<String> = cache
                .typing_in(&selected.channel, now_ms())
                .filter(|t| &t.user_id != me)
                .map(|t| t.username.clone())
                .collect();
            typing_text(&names)
        });

        egui::Panel::bottom("composer")
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_CHAT)
                    .inner_margin(Margin {
                        left: 20,
                        right: 20,
                        top: 0,
                        bottom: 18,
                    }),
            )
            .show(ui, |ui| {
                // Строка над полем: «печатает…» или режим правки; высота постоянна.
                ui.horizontal(|ui| {
                    ui.set_min_height(20.0);
                    if let Some(reply) = self.reply.clone().filter(|_| !self.is_editing()) {
                        ui.label(
                            theme::muted(format!(
                                "Ответ для {}: {}",
                                reply.author,
                                theme::truncate_chars(&reply.preview.replace('\n', " "), 60)
                            ))
                            .size(12.5),
                        );
                        if ui
                            .small_button("✕")
                            .on_hover_text("Отменить ответ")
                            .clicked()
                        {
                            self.reply = None;
                        }
                    } else if self.is_editing() {
                        ui.label(
                            RichText::new("Правка сообщения")
                                .color(theme::AMBER)
                                .size(12.5),
                        );
                        if ui.small_button("Отмена (Esc)").clicked() {
                            self.cancel_edit();
                        }
                    } else if let Some(text) = &typing {
                        ui.label(theme::muted(text).size(12.5));
                    }
                });
                let enabled = self.selected_channel.is_some();
                let editing = self.is_editing();
                let outgoing =
                    self.composer
                        .show(ui, bridge, &mut self.draft, &hint, enabled, editing);
                if editing && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.cancel_edit();
                }
                if let Some(outgoing) = outgoing {
                    self.submit(bridge, outgoing);
                    ui.memory_mut(|m| m.request_focus(super::composer::edit_id()));
                }
            });
    }

    /// Отправляет черновик: новое сообщение или правку.
    fn submit(&mut self, bridge: &Bridge, outgoing: super::composer::Outgoing) {
        let Some(selected) = self.selected_channel.clone() else {
            return;
        };
        let content = outgoing.content;
        if !outgoing.keep_draft
            && let Some(edit) = self.edit.take()
        {
            if let Some(text) = outgoing_text(&content) {
                bridge.edit_message(selected.kind, edit.message_id, text);
            }
            self.draft.clear();
            return;
        }
        let text = outgoing_text(&content);
        let reply_to_id = self.reply.as_ref().map(|r| r.message_id.clone());
        match selected.kind {
            ChannelKind::Space if outgoing.attachments.is_empty() => {
                if let Some(text) = text {
                    bridge.send_message(selected.kind, selected.channel, text, reply_to_id);
                }
            }
            ChannelKind::Space => bridge.send_rest(
                selected.channel,
                bs_api::SendMessageBody {
                    content: text.unwrap_or_default(),
                    attachments: outgoing.attachments,
                    reply_to_id,
                },
            ),
            ChannelKind::Dm => {
                if text.is_some() || !outgoing.attachments.is_empty() {
                    bridge.send_dm_with(selected.channel, text, outgoing.attachments, reply_to_id);
                }
            }
        }
        if !outgoing.keep_draft {
            self.draft.clear();
            self.reply = None;
        }
    }

    /// Сбрасывает состояние прокрутки, если открыли другой канал.
    fn sync_scroll_key(&mut self, selected: &SelectedChannel) {
        let key = format!("{}:{}", selected.origin, selected.channel);
        if self.scroll_key.as_ref() != Some(&key) {
            self.scroll_key = Some(key);
            self.scroll_anchor = None;
            self.was_scrolled_down = false;
        }
    }

    /// Прочитано: `ack` уходит только когда последнее сообщение изменилось.
    fn ack_latest(
        &mut self,
        bridge: &Bridge,
        ack_key: &str,
        channel_id: &str,
        last_id: Option<&str>,
    ) {
        let Some(last_id) = last_id else {
            return;
        };
        let last = self.last_ack.as_ref();
        if should_ack(
            last.map(|(c, m)| (c.as_str(), m.as_str())),
            ack_key,
            last_id,
        ) {
            bridge.ack_channel(channel_id.to_owned(), last_id.to_owned());
            self.last_ack = Some((ack_key.to_owned(), last_id.to_owned()));
        }
    }

    fn show_messages(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_CHAT)
                    .inner_margin(Margin::symmetric(0, 0)),
            )
            .show(ui, |ui| {
                let Some(selected) = self.selected_channel.clone() else {
                    ui.centered_and_justified(|ui| {
                        ui.label(theme::muted(
                            "Выберите канал или беседу слева — или найдите друзей",
                        ));
                    });
                    return;
                };
                self.sync_scroll_key(&selected);
                let channel_id = selected.channel.clone();
                bridge.ensure_history(selected.kind, &channel_id);
                let history = bridge
                    .history_state(&channel_id)
                    .cloned()
                    .unwrap_or_default();

                let (me, rows) = {
                    let Some(cache) = bridge.cache() else {
                        return;
                    };
                    self.names = name_map(cache);
                    (
                        cache.me().id.clone(),
                        message_rows(cache, selected.kind, &channel_id, &self.names),
                    )
                };
                let ack_key = format!("{}:{channel_id}", selected.origin);
                let first_id = rows.first().map(|row| row.id.clone());
                let mut retry = false;
                let mut edit_request = None;

                let output = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        retry = show_history_header(ui, &history);
                        let mut prev: Option<&MessageRow> = None;
                        for row in &rows {
                            let show_header = needs_header(prev, row);
                            if show_header
                                && prev.is_none_or(|p| {
                                    theme::local_day(p.created_at)
                                        != theme::local_day(row.created_at)
                                })
                            {
                                theme::day_divider(ui, &theme::day_label(row.created_at));
                            }
                            let manage = message_actions(&row.author_id, &me).manage;
                            self.show_message(
                                ui,
                                bridge,
                                &selected,
                                row,
                                show_header,
                                manage,
                                first_id.as_deref(),
                                &mut edit_request,
                            );
                            prev = Some(row);
                        }
                        ui.add_space(8.0);
                    });
                if let Some((id, content)) = edit_request {
                    self.begin_edit(id, &content);
                }

                // Подгрузка старых сообщений при прокрутке к верху ленты.
                let offset = output.state.clamped_offset().y;
                if offset >= crate::logic::LOAD_OLDER_THRESHOLD_PX {
                    self.was_scrolled_down = true;
                }
                let wants_older = should_load_older(
                    offset,
                    output.content_size.y,
                    output.inner_rect.height(),
                    self.was_scrolled_down,
                );
                if retry || (wants_older && history.can_auto_load()) {
                    if self.scroll_anchor.is_none() {
                        self.scroll_anchor.clone_from(&first_id);
                    }
                    bridge.load_older(selected.kind, &channel_id);
                }

                self.ack_latest(
                    bridge,
                    &ack_key,
                    &channel_id,
                    rows.last().map(|row| row.id.as_str()),
                );
                self.show_react_picker(ui, bridge);
            });
    }

    /// Окно выбора эмодзи для реакции.
    fn show_react_picker(&mut self, ui: &egui::Ui, bridge: &Bridge) {
        let Some((message_id, mut tab)) = self.react_picker.clone() else {
            return;
        };
        let mut open = true;
        let mut chosen = None;
        egui::Window::new("Реакция")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                chosen = super::composer::emoji_picker(ui, &mut tab);
            });
        if let Some(glyph) = chosen {
            bridge.add_reaction(message_id, glyph.to_owned());
            self.react_picker = None;
        } else if open {
            self.react_picker = Some((message_id, tab));
        } else {
            self.react_picker = None;
        }
    }

    /// Одно сообщение: аватар и имя у первого в группе, тело, контекстное меню.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn show_message(
        &mut self,
        ui: &mut egui::Ui,
        bridge: &Bridge,
        selected: &SelectedChannel,
        row: &MessageRow,
        show_header: bool,
        manage: bool,
        first_id: Option<&str>,
        edit_request: &mut Option<(String, String)>,
    ) {
        ui.add_space(if show_header { 16.0 } else { 2.0 });
        let mut hits: Vec<media::Hit> = Vec::new();
        let mut profile_rects: Vec<egui::Rect> = Vec::new();
        let inner = ui.horizontal_top(|ui| {
            ui.add_space(20.0);
            if show_header {
                let response = theme::avatar(ui, &row.author, 40.0, None, row.avatar.as_deref());
                profile_rects.push(response.rect);
            } else {
                ui.allocate_space(egui::vec2(40.0, 1.0));
            }
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                if show_header {
                    ui.horizontal(|ui| {
                        let name = ui.add(egui::Label::new(
                            RichText::new(&row.author)
                                .family(theme::bold_family())
                                .size(15.5)
                                .color(theme::TEXT_BRIGHT),
                        ));
                        profile_rects.push(name.rect);
                        ui.label(theme::muted(theme::stamp(row.created_at, now_ms())).size(11.5));
                    });
                }
                if let Some((author, text)) = &row.reply {
                    super::body::show_reply_quote(ui, author, text);
                }
                if let Some(sticker) = media::sticker_of(&row.body) {
                    hits.extend(media::show_sticker(ui, sticker));
                } else if let Some(gif) = media::gif_of(&row.body) {
                    hits.extend(media::show_gif(ui, gif));
                } else if !row.body.is_empty() {
                    let names = &self.names;
                    let blocks = crate::markdown::parse(&row.body, &|id| names.get(id).cloned());
                    super::body::show_text(ui, &row.id, &blocks, row.edited);
                }
                for attachment in &row.attachments {
                    hits.extend(media::show_attachment(ui, attachment));
                }
                if !row.reactions.is_empty() {
                    match super::body::show_reactions(ui, &row.reactions) {
                        Some(super::body::ReactionClick::Toggle(emoji, mine)) => {
                            if mine {
                                bridge.remove_reaction(row.id.clone(), emoji);
                            } else {
                                bridge.add_reaction(row.id.clone(), emoji);
                            }
                        }
                        Some(super::body::ReactionClick::OpenPicker) => {
                            self.react_picker = Some((row.id.clone(), 0));
                        }
                        None => {}
                    }
                }
            });
        });
        let rect =
            egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), inner.response.rect.y_range());
        let response = ui.interact(rect, ui.auto_id_with(("msg", &row.id)), Sense::click());
        // После подгрузки возвращаем в поле зрения сообщение, которое читали:
        // новые страницы появляются выше него.
        if self.scroll_anchor.as_deref() == Some(row.id.as_str())
            && first_id != Some(row.id.as_str())
        {
            response.scroll_to_me(Some(egui::Align::TOP));
            self.scroll_anchor = None;
        }
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && let Some((_, value)) = hits.iter().find(|(rect, _)| rect.contains(pos))
        {
            self.viewer = Some(value.clone());
        }
        let message_id = row.id.clone();
        let open_profile = response.clicked()
            && response
                .interact_pointer_pos()
                .is_some_and(|pos| profile_rects.iter().any(|rect| rect.contains(pos)));
        if open_profile {
            self.profile = Some(super::profile::ProfileState::open(
                bridge,
                &row.author_id,
                &row.author,
            ));
        }
        if response.secondary_clicked() {
            self.menu_image = response.interact_pointer_pos().and_then(|pos| {
                hits.iter()
                    .find(|(rect, _)| rect.contains(pos))
                    .map(|(_, value)| (row.id.clone(), value.clone()))
            });
        }
        let image = self
            .menu_image
            .clone()
            .filter(|(id, _)| *id == row.id)
            .map(|(_, value)| value);
        let can_collect = image
            .as_deref()
            .is_some_and(|v| !v.starts_with("https://") || v.starts_with("sticker:"));
        let mut open_picker = false;
        let mut reply_request = false;
        response.context_menu(|ui| {
            if let Some(value) = &image {
                if ui.button("Копировать картинку").clicked() {
                    if let Some(hub) = crate::images::ImageHub::of(ui.ctx()) {
                        hub.request_raw(value, crate::images::RawPurpose::Copy);
                    }
                    ui.close();
                }
                if can_collect && ui.button("Сохранить в мои стикеры").clicked()
                {
                    if let Some(hub) = crate::images::ImageHub::of(ui.ctx()) {
                        hub.request_raw(
                            value,
                            crate::images::RawPurpose::Sticker("Стикер".to_owned()),
                        );
                    }
                    ui.close();
                }
                ui.separator();
            }
            if ui.button("Ответить").clicked() {
                reply_request = true;
                ui.close();
            }
            if ui.button("Добавить реакцию…").clicked() {
                open_picker = true;
                ui.close();
            }
            if !row.body.is_empty()
                && media::sticker_of(&row.body).is_none()
                && media::gif_of(&row.body).is_none()
                && ui.button("Копировать текст").clicked()
            {
                ui.ctx().copy_text(row.body.clone());
                ui.close();
            }
            if ui.button("Отметить непрочитанным").clicked() {
                bridge.send(bs_types::ClientEvent::MarkUnread {
                    channel_id: selected.channel.clone(),
                    message_id: message_id.clone(),
                });
                ui.close();
            }
            if manage && ui.button("Изменить").clicked() {
                *edit_request = Some((message_id.clone(), row.body.clone()));
                ui.close();
            }
            if manage && ui.button("Удалить").clicked() {
                bridge.delete_message(selected.kind, message_id.clone());
                ui.close();
            }
        });
        if reply_request {
            self.reply = Some(ReplyTarget {
                message_id: row.id.clone(),
                author: row.author.clone(),
                preview: row.body.clone(),
            });
            self.edit = None;
        }
        if open_picker {
            self.react_picker = Some((row.id.clone(), 0));
        }
    }
}

/// Выполняет действие из панели «Подключения».
fn route_connection(bridge: &mut Bridge, action: super::connections::ConnAction) {
    use super::connections::ConnAction;
    match action {
        ConnAction::None => {}
        ConnAction::Connect { address, password } => {
            bridge.connect_remote(&address, Some(&password));
        }
        ConnAction::Reconnect(origin) => bridge.reconnect(&origin),
        ConnAction::Reauthenticate { origin, password } => {
            bridge.reauthenticate(&origin, &password);
        }
        ConnAction::OwnLogin {
            origin,
            username,
            password,
        } => bridge.own_login(&origin, &username, &password),
        ConnAction::Disconnect(origin) => bridge.disconnect(&origin),
        ConnAction::ForceRemove(origin) => bridge.force_remove(&origin),
        ConnAction::Delete { origins, mode } => {
            let targets = if origins.is_empty() {
                bridge.rows().into_iter().map(|r| r.origin).collect()
            } else {
                origins
            };
            bridge.delete_identity(&targets, mode);
        }
    }
}

#[cfg(test)]
mod tests {
    use bs_core::Cache;
    use bs_types::ReadyPayload;

    use super::{
        MessageRow, dm_sidebar_items, member_rows, needs_header, sidebar_items, typing_text,
    };

    fn ready(
        space_id: &str,
        space_name: &str,
        channel_id: &str,
        channel_name: &str,
    ) -> ReadyPayload {
        serde_json::from_value(serde_json::json!({
            "user": {
                "id": "u1", "username": "alice", "displayName": null, "avatar": null,
                "banner": null, "accentColor": null, "avatarColor": "mint", "bio": null,
                "status": "online", "customStatus": null, "isAdmin": false,
                "createdAt": 1_750_000_000_000_i64, "homeInstance": null,
                "homeUserId": null, "replicatedInstances": []
            },
            "spaces": [{
                "id": space_id, "name": space_name, "icon": null,
                "ownerId": "u1", "visibility": "public",
                "directoryListed": false, "createdAt": 1_750_000_000_000_i64,
                "channels": [{
                    "id": channel_id, "name": channel_name, "spaceId": space_id,
                    "type": "text", "position": 0, "createdAt": 1_750_000_000_000_i64
                }],
                "categories": [], "members": [], "roles": []
            }]
        }))
        .unwrap_or_else(|err| panic!("готовый ready должен разбираться: {err}"))
    }

    #[test]
    fn sidebar_lists_space_and_channel() {
        let cache = Cache::from_ready(ready("s1", "Работа", "c1", "general"));
        let items = sidebar_items(&cache);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Работа");
        assert_eq!(items[0].channels().count(), 1);
        assert_eq!(items[0].channels().next().unwrap().name, "general");
        assert_eq!(items[0].id, "s1");
        assert!(!items[0].channels().next().unwrap().unread);
    }

    #[test]
    fn dm_sidebar_lists_private_channels_newest_first() {
        let mut payload = ready("s1", "Работа", "c1", "general");
        payload.dm_channels = serde_json::from_value(serde_json::json!([
            {"id": "100", "federatedId": null, "ownerId": null, "ownerHomeUserId": null,
             "ownerHomeInstance": null, "createdAt": 1, "metadataUpdatedAt": 1,
             "members": [], "name": "Старая группа"},
            {"id": "200", "federatedId": null, "ownerId": null, "ownerHomeUserId": null,
             "ownerHomeInstance": null, "createdAt": 1, "metadataUpdatedAt": 1,
             "members": [], "name": "Новая группа"}
        ]))
        .expect("dm_channels");
        let dms = dm_sidebar_items(&Cache::from_ready(payload));
        let labels: Vec<_> = dms.iter().map(|dm| dm.label.as_str()).collect();
        assert_eq!(labels, ["Новая группа", "Старая группа"]);
    }

    fn space_item(id: &str) -> super::SidebarSpace {
        super::SidebarSpace {
            origin: 0,
            id: id.to_owned(),
            name: id.to_owned(),
            color: None,
            icon: Some("i.png".to_owned()),
            groups: Vec::new(),
        }
    }

    fn dm_item(id: &str, key: Option<&str>, at: i64) -> super::SidebarDm {
        super::SidebarDm {
            origin: 0,
            key: key.map(str::to_owned),
            id: id.to_owned(),
            label: id.to_owned(),
            unread: false,
            status: None,
            preview: None,
            at: Some(at),
            avatar: Some("a.png".to_owned()),
        }
    }

    #[test]
    fn spaces_of_all_instances_are_listed_with_their_owner() {
        let parts = vec![
            (0, "https://home.example", vec![space_item("1")], Vec::new()),
            (
                2,
                "https://orbit.example",
                vec![space_item("1")],
                Vec::new(),
            ),
        ];
        let (spaces, _) = super::merge_origins(parts);
        // Одинаковые id на разных инстансах — разные пространства.
        assert_eq!(spaces.len(), 2);
        assert_eq!((spaces[0].origin, spaces[1].origin), (0, 2));
        assert_eq!(
            spaces[1].icon.as_deref(),
            Some("https://orbit.example/api/uploads/i.png")
        );
    }

    #[test]
    fn copies_of_one_dm_collapse_to_the_home_copy() {
        let parts = vec![
            (
                0,
                "https://home.example",
                Vec::new(),
                vec![dm_item("9", Some("k"), 10)],
            ),
            (
                1,
                "https://orbit.example",
                Vec::new(),
                vec![
                    dm_item("77", Some("k"), 10),
                    dm_item("78", None, 20),
                    dm_item("79", None, 5),
                ],
            ),
        ];
        let (_, dms) = super::merge_origins(parts);
        let ids: Vec<_> = dms.iter().map(|d| (d.origin, d.id.as_str())).collect();
        // Копия с ключом одна (домашняя); беседы без ключа не склеиваются;
        // порядок — по времени последнего сообщения.
        assert_eq!(ids, vec![(1, "78"), (0, "9"), (1, "79")]);
    }

    #[test]
    fn sidebar_and_members_carry_image_fields() {
        let mut payload = ready("s1", "Работа", "c1", "general");
        let mut value = serde_json::to_value(&payload).unwrap();
        value["spaces"][0]["icon"] = serde_json::json!("icon.png");
        payload = serde_json::from_value(value).unwrap();
        let cache = Cache::from_ready(payload);
        assert_eq!(sidebar_items(&cache)[0].icon.as_deref(), Some("icon.png"));
        assert!(member_rows(&cache, "s1").iter().all(|m| m.avatar.is_none()));
    }

    #[test]
    fn dm_sidebar_is_empty_without_private_channels() {
        let cache = Cache::from_ready(ready("s1", "A", "c1", "g"));
        assert!(dm_sidebar_items(&cache).is_empty());
    }

    fn row(author: &str, at: i64) -> MessageRow {
        MessageRow {
            id: format!("{at}"),
            author_id: author.to_owned(),
            author: author.to_owned(),
            body: String::new(),
            edited: false,
            created_at: at,
            avatar: None,
            attachments: Vec::new(),
            reactions: Vec::new(),
            reply: None,
        }
    }

    #[test]
    fn header_shown_for_first_and_new_author() {
        let a = row("a", 1_000_000);
        assert!(needs_header(None, &a));
        assert!(needs_header(Some(&a), &row("b", 1_000_100)));
    }

    #[test]
    fn same_author_within_window_is_grouped() {
        let a = row("a", 1_000_000);
        assert!(!needs_header(Some(&a), &row("a", 1_000_000 + 60_000)));
        assert!(needs_header(Some(&a), &row("a", 1_000_000 + 6 * 60_000)));
    }

    #[test]
    fn typing_text_variants() {
        assert_eq!(typing_text(&[]), None);
        assert_eq!(
            typing_text(&["Аня".into()]).as_deref(),
            Some("Аня печатает…")
        );
        assert_eq!(
            typing_text(&["Аня".into(), "Борис".into()]).as_deref(),
            Some("Аня и Борис печатают…")
        );
        assert_eq!(
            typing_text(&["a".into(), "b".into(), "c".into()]).as_deref(),
            Some("Несколько человек печатают…")
        );
    }

    #[test]
    fn members_are_sorted_online_first_then_by_name() {
        let mut payload = ready("s1", "Работа", "c1", "general");
        let user = |id: &str, name: &str, status: &str| {
            serde_json::json!({
                "spaceId": "s1", "userId": id, "nickname": null, "joinedAt": 1,
                "user": {
                    "id": id, "username": name, "displayName": null, "avatar": null,
                    "banner": null, "accentColor": null, "avatarColor": null, "bio": null,
                    "status": status, "customStatus": null, "isAdmin": false,
                    "createdAt": 1, "homeInstance": null, "homeUserId": null,
                    "replicatedInstances": []
                },
                "roles": []
            })
        };
        payload.spaces[0].members = serde_json::from_value(serde_json::json!([
            user("1", "zed", "offline"),
            user("2", "bob", "online"),
            user("3", "alice", "online"),
            user("4", "carl", "idle"),
        ]))
        .expect("members");
        let cache = Cache::from_ready(payload);
        let names: Vec<_> = member_rows(&cache, "s1")
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(names, ["alice", "bob", "carl", "zed"]);
        assert!(member_rows(&cache, "nope").is_empty());
    }

    #[test]
    fn owner_gets_own_section_first() {
        let mut payload = ready("s1", "Работа", "c1", "general");
        let user = |id: &str, name: &str, status: &str| {
            serde_json::json!({
                "spaceId": "s1", "userId": id, "nickname": null, "joinedAt": 1,
                "user": {
                    "id": id, "username": name, "displayName": null, "avatar": null,
                    "banner": null, "accentColor": null, "avatarColor": null, "bio": null,
                    "status": status, "customStatus": null, "isAdmin": false,
                    "createdAt": 1, "homeInstance": null, "homeUserId": null,
                    "replicatedInstances": []
                },
                "roles": []
            })
        };
        // Владелец в ready — «u1»; он офлайн, но всё равно первый.
        payload.spaces[0].members = serde_json::from_value(serde_json::json!([
            user("2", "bob", "online"),
            user("u1", "alice", "offline"),
        ]))
        .expect("members");
        let rows = member_rows(&Cache::from_ready(payload), "s1");
        assert_eq!(rows[0].name, "alice");
        assert_eq!(rows[0].section(), super::MemberSection::Owner);
        assert_eq!(rows[1].section(), super::MemberSection::Online);
    }
}
