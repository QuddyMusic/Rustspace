//! Основной экран в стиле Discord: колонка пространств, колонка каналов,
//! шапка и лента сообщений, поле ввода, панель участников.

use std::collections::HashMap;

use bs_core::Cache;
use bs_types::{ChannelId, UserStatus};
use egui::{Align, CornerRadius, Layout, Margin, RichText, Sense};

use crate::bridge::{Bridge, ChannelKind, HistoryState};
use crate::logic::{
    DEFAULT_REACTION, author_name, can_send, connection_label, message_actions, outgoing_text,
    should_ack, should_load_older,
};
use crate::social::dm_display_name;
use crate::theme;

/// Что пользователь попросил сделать за пределами чата.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ChatAction {
    /// Ничего.
    #[default]
    None,
    /// Открыть экран друзей.
    OpenSocial,
    /// Выйти из инстанса.
    Logout,
}

/// Выбранный канал.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedChannel {
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
    /// Голосовой канал: в GUI пока недоступен.
    pub(crate) voice: bool,
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
    /// Идентификатор пространства.
    pub(crate) id: String,
    /// Имя пространства.
    pub(crate) name: String,
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
    /// Идентификатор личного канала.
    pub(crate) id: String,
    /// Имя собеседника или название группы.
    pub(crate) label: String,
    /// Есть непрочитанные.
    pub(crate) unread: bool,
}

/// Собирает сайдбар: пространства, в них категории и каналы в порядке сервера.
#[must_use]
pub(crate) fn sidebar_items(cache: &Cache) -> Vec<SidebarSpace> {
    let mut spaces: Vec<_> = cache.spaces().collect();
    spaces.sort_by(|a, b| a.space.id.cmp(&b.space.id));
    spaces
        .into_iter()
        .map(|space| {
            let to_row = |channel: &bs_types::Channel| SidebarChannel {
                id: channel.id.clone(),
                name: channel.name.clone(),
                unread: cache.unread_count(&channel.id) > 0,
                voice: channel.channel_type == bs_types::ChannelType::Voice,
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
                id: space.space.id.clone(),
                name: space.space.name.clone(),
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
            id: dm.id.clone(),
            label: dm_display_name(dm, &me),
            unread: cache.unread_count(&dm.id) > 0,
        })
        .collect()
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

/// Строки ленты канала `channel_id` из кэша (старые → новые).
fn message_rows(cache: &Cache, kind: ChannelKind, channel_id: &str) -> Vec<MessageRow> {
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

/// Состояние экрана чата.
#[derive(Debug, Default)]
pub(crate) struct ChatScreen {
    /// Выбранный канал и его инстанс.
    pub(crate) selected_channel: Option<SelectedChannel>,
    pub(crate) draft: String,
    /// Пространство, открытое в колонке каналов; `None` — личные сообщения.
    active_space: Option<String>,
    /// Последний открытый канал в каждом пространстве.
    last_by_space: HashMap<String, String>,
    /// Последняя открытая личная беседа.
    last_dm: Option<String>,
    /// Первичный выбор канала уже сделан.
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

    /// Открывает канал.
    pub(crate) fn open_channel(&mut self, selected: SelectedChannel) {
        // Правка относилась к прежнему каналу.
        if self.selected_channel.as_ref() != Some(&selected) {
            self.edit = None;
        }
        match selected.kind {
            ChannelKind::Dm => {
                self.last_dm = Some(selected.channel.clone());
                self.active_space = None;
            }
            ChannelKind::Space => {
                if let Some(space) = &self.active_space {
                    self.last_by_space
                        .insert(space.clone(), selected.channel.clone());
                }
            }
        }
        self.selected_channel = Some(selected);
    }

    /// Первичный выбор и согласование колонки каналов с выбранным каналом.
    fn sync_selection(&mut self, spaces: &[SidebarSpace]) {
        if !self.initialized {
            self.initialized = true;
            self.show_members = true;
            if self.selected_channel.is_none()
                && let Some(space) = spaces.first()
            {
                self.active_space = Some(space.id.clone());
                if let Some(channel) = space.first_text() {
                    self.open_channel(SelectedChannel {
                        channel: channel.id.clone(),
                        kind: ChannelKind::Space,
                    });
                }
            }
        }
        // Личная беседа, открытая снаружи (из «Друзей»), переключает на ЛС.
        if let Some(selected) = &self.selected_channel {
            match selected.kind {
                ChannelKind::Dm => {
                    self.last_dm = Some(selected.channel.clone());
                    self.active_space = None;
                }
                ChannelKind::Space => {
                    if let Some(space) = spaces
                        .iter()
                        .find(|s| s.channels().any(|c| c.id == selected.channel))
                        && self.active_space.as_deref() != Some(space.id.as_str())
                    {
                        self.active_space = Some(space.id.clone());
                    }
                }
            }
        }
    }

    /// Рисует экран. Возвращает действие, которое нужно выполнить снаружи.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge) -> ChatAction {
        let mut action = ChatAction::None;
        let (items, dms) = bridge.cache().map_or_else(Default::default, |cache| {
            (sidebar_items(cache), dm_sidebar_items(cache))
        });
        self.sync_selection(&items);

        let mut requested: Option<SelectedChannel> = None;
        self.show_rail(ui, &items, &dms, &mut requested, &mut action);
        self.show_channel_column(ui, bridge, &items, &dms, &mut requested, &mut action);

        if let Some(selected) = requested {
            self.open_channel(selected);
        }

        if self.show_members
            && let Some(space_id) = self.active_space.clone()
        {
            Self::show_member_panel(ui, bridge, &space_id);
        }

        self.show_header(ui, bridge);
        self.show_composer(ui, bridge);
        self.show_messages(ui, bridge);
        action
    }

    /// Левая колонка: «Личные сообщения», пространства, «Друзья».
    fn show_rail(
        &mut self,
        ui: &mut egui::Ui,
        items: &[SidebarSpace],
        dms: &[SidebarDm],
        requested: &mut Option<SelectedChannel>,
        action: &mut ChatAction,
    ) {
        let dm_unread = dms.iter().any(|d| d.unread);
        egui::Panel::left("rail")
            .exact_size(72.0)
            .resizable(false)
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
                        let home = self.active_space.is_none();
                        if theme::rail_home(ui, home, dm_unread)
                            .on_hover_text("Личные сообщения")
                            .clicked()
                        {
                            self.active_space = None;
                            if let Some(dm) = self
                                .last_dm
                                .clone()
                                .or_else(|| dms.first().map(|d| d.id.clone()))
                            {
                                *requested = Some(SelectedChannel {
                                    channel: dm,
                                    kind: ChannelKind::Dm,
                                });
                            } else {
                                self.selected_channel = None;
                            }
                        }
                        if theme::rail_button(ui, "👥", theme::BG_ACTIVE, false, false)
                            .on_hover_text("Друзья и поиск")
                            .clicked()
                        {
                            *action = ChatAction::OpenSocial;
                        }
                        theme::rail_separator(ui);
                        for space in items {
                            let selected = self.active_space.as_deref() == Some(space.id.as_str());
                            if theme::rail_button(
                                ui,
                                &theme::initials(&space.name),
                                theme::color_for(&space.name),
                                selected,
                                space.has_unread(),
                            )
                            .on_hover_text(&space.name)
                            .clicked()
                                && !selected
                            {
                                self.active_space = Some(space.id.clone());
                                let remembered = self
                                    .last_by_space
                                    .get(&space.id)
                                    .filter(|id| space.channels().any(|c| &c.id == *id))
                                    .cloned()
                                    .or_else(|| space.first_text().map(|c| c.id.clone()));
                                match remembered {
                                    Some(channel) => {
                                        *requested = Some(SelectedChannel {
                                            channel,
                                            kind: ChannelKind::Space,
                                        });
                                    }
                                    None => self.selected_channel = None,
                                }
                            }
                        }
                    });
            });
    }

    /// Вторая колонка: каналы пространства или список бесед, плюс панель пользователя.
    fn show_channel_column(
        &self,
        ui: &mut egui::Ui,
        bridge: &Bridge,
        items: &[SidebarSpace],
        dms: &[SidebarDm],
        requested: &mut Option<SelectedChannel>,
        action: &mut ChatAction,
    ) {
        egui::Panel::left("channels")
            .resizable(true)
            .default_size(236.0)
            .size_range(180.0..=360.0)
            .frame(egui::Frame::new().fill(theme::BG_SIDEBAR))
            .show(ui, |ui| {
                let title =
                    self.active_space.as_ref().map_or("Личные сообщения", |id| {
                        items
                            .iter()
                            .find(|s| &s.id == id)
                            .map_or("Пространство", |s| s.name.as_str())
                    });
                egui::Panel::top("channels_header")
                    .exact_size(52.0)
                    .resizable(false)
                    .frame(
                        egui::Frame::new()
                            .fill(theme::BG_SIDEBAR)
                            .inner_margin(Margin::symmetric(18, 0)),
                    )
                    .show(ui, |ui| {
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.label(
                                RichText::new(title)
                                    .strong()
                                    .size(16.0)
                                    .color(theme::TEXT_BRIGHT),
                            );
                        });
                    });

                Self::show_user_bar(ui, bridge, action);

                egui::ScrollArea::vertical()
                    .id_salt("channel_list")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if let Some(space) = self
                            .active_space
                            .as_ref()
                            .and_then(|id| items.iter().find(|s| &s.id == id))
                        {
                            self.channel_list(ui, space, requested);
                        } else {
                            self.dm_list(ui, dms, requested, action);
                        }
                    });
            });
    }

    /// Каналы пространства по категориям.
    fn channel_list(
        &self,
        ui: &mut egui::Ui,
        space: &SidebarSpace,
        requested: &mut Option<SelectedChannel>,
    ) {
        ui.add_space(6.0);
        for group in &space.groups {
            if let Some(name) = &group.name {
                theme::category_header(ui, name);
            }
            for channel in &group.channels {
                if channel.voice {
                    theme::disabled_row(ui, &format!("🔊  {}", channel.name))
                        .on_hover_text("Голосовые каналы в этом клиенте пока недоступны");
                    continue;
                }
                let selected = self.selected_channel.as_ref().is_some_and(|current| {
                    current.kind == ChannelKind::Space && current.channel.as_str() == channel.id
                });
                if theme::list_row(
                    ui,
                    &format!("#   {}", channel.name),
                    selected,
                    channel.unread,
                )
                .clicked()
                {
                    *requested = Some(SelectedChannel {
                        channel: channel.id.clone(),
                        kind: ChannelKind::Space,
                    });
                }
            }
        }
    }

    /// Личные беседы.
    fn dm_list(
        &self,
        ui: &mut egui::Ui,
        dms: &[SidebarDm],
        requested: &mut Option<SelectedChannel>,
        action: &mut ChatAction,
    ) {
        ui.add_space(8.0);
        if dms.is_empty() {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.label(theme::muted("Бесед пока нет"));
                if ui.button("Найти друзей").clicked() {
                    *action = ChatAction::OpenSocial;
                }
            });
        }
        for dm in dms {
            let selected = self.selected_channel.as_ref().is_some_and(|current| {
                current.kind == ChannelKind::Dm && current.channel.as_str() == dm.id
            });
            if theme::dm_row(ui, &dm.label, selected, dm.unread).clicked() {
                *requested = Some(SelectedChannel {
                    channel: dm.id.clone(),
                    kind: ChannelKind::Dm,
                });
            }
        }
    }

    /// Нижняя панель колонки каналов: «я», статус соединения и выход.
    fn show_user_bar(ui: &mut egui::Ui, bridge: &Bridge, action: &mut ChatAction) {
        let (name, handle, status) = bridge.cache().map_or_else(
            || (String::new(), String::new(), UserStatus::Online),
            |cache| {
                let me = cache.me();
                (
                    author_name(me.display_name.as_deref(), &me.username).to_owned(),
                    format!("@{}", me.username),
                    cache.user_status(&me.id).unwrap_or(me.status),
                )
            },
        );
        let connected = *bridge.connection() == bs_instances::ConnectionState::Connected;
        egui::Panel::bottom("user_bar")
            .exact_size(58.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_RAIL)
                    .inner_margin(Margin::symmetric(12, 0)),
            )
            .show(ui, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    theme::avatar(ui, &name, 36.0, Some(status));
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(theme::truncate_chars(&name, 14))
                                .strong()
                                .color(theme::TEXT_BRIGHT),
                        );
                        let sub = if connected {
                            handle
                        } else {
                            connection_label(bridge.connection())
                        };
                        ui.label(theme::muted(theme::truncate_chars(&sub, 18)).size(11.5));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let logout = ui
                            .add_enabled(!bridge.is_busy(), egui::Button::new("Выйти").small())
                            .on_hover_text("Выйти из инстанса, чтобы войти в другой");
                        if logout.clicked() {
                            *action = ChatAction::Logout;
                        }
                    });
                });
            });
    }

    /// Правая панель: участники пространства по секциям.
    fn show_member_panel(ui: &mut egui::Ui, bridge: &Bridge, space_id: &str) {
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
                            theme::member_row(ui, &row.name, row.status);
                        }
                    });
            });
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
                            .strong()
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
                            let button =
                                egui::Button::new(RichText::new("👥").size(18.0).color(color))
                                    .fill(egui::Color32::TRANSPARENT);
                            if ui
                                .add(button)
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
                    .strong()
                    .color(theme::TEXT_BRIGHT),
            );
            ui.label(theme::muted("Здесь пока всё, что было написано раньше."));
            ui.add_space(16.0);
        });
    }
    false
}

impl ChatScreen {
    fn show_composer(&mut self, ui: &mut egui::Ui, bridge: &Bridge) {
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
                    if self.is_editing() {
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
                let can_send_now = can_send(
                    self.selected_channel.as_ref().map(|s| &s.channel),
                    &self.draft,
                );
                let mut submitted = false;
                let mut response = None;
                egui::Frame::new()
                    .fill(theme::BG_BUBBLE)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .corner_radius(CornerRadius::same(16))
                    .inner_margin(Margin::symmetric(16, 11))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("+").size(22.0).color(theme::TEXT_FAINT))
                                .on_hover_text("Вложения появятся в одном из следующих этапов");
                            let send_width = if can_send_now { 110.0 } else { 0.0 };
                            let width = (ui.available_width() - send_width).max(80.0);
                            let edit = ui.add_enabled(
                                enabled,
                                egui::TextEdit::singleline(&mut self.draft)
                                    .hint_text(hint)
                                    .frame(egui::Frame::NONE)
                                    .align(egui::Align2::LEFT_CENTER)
                                    .desired_width(width),
                            );
                            submitted =
                                edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if can_send_now && ui.add(theme::primary_button("Отправить")).clicked()
                            {
                                submitted = true;
                            }
                            response = Some(edit);
                        });
                    });
                if self.is_editing() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.cancel_edit();
                }
                if submitted {
                    self.submit(bridge);
                    if let Some(edit) = response {
                        edit.request_focus();
                    }
                }
            });
    }

    /// Отправляет черновик: новое сообщение или правку.
    fn submit(&mut self, bridge: &Bridge) {
        let Some(content) = outgoing_text(&self.draft) else {
            return;
        };
        let Some(selected) = self.selected_channel.clone() else {
            return;
        };
        if let Some(edit) = self.edit.take() {
            bridge.edit_message(selected.kind, edit.message_id, content);
        } else {
            bridge.send_message(selected.kind, selected.channel, content);
        }
        self.draft.clear();
    }

    /// Сбрасывает состояние прокрутки, если открыли другой канал.
    fn sync_scroll_key(&mut self, selected: &SelectedChannel) {
        let key = selected.channel.clone();
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
                    (
                        cache.me().id.clone(),
                        message_rows(cache, selected.kind, &channel_id),
                    )
                };
                let ack_key = channel_id.clone();
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
            });
    }

    /// Одно сообщение: аватар и имя у первого в группе, тело, контекстное меню.
    #[allow(clippy::too_many_arguments)]
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
        let inner = ui.horizontal_top(|ui| {
            ui.add_space(20.0);
            if show_header {
                theme::avatar(ui, &row.author, 40.0, None);
            } else {
                ui.allocate_space(egui::vec2(40.0, 1.0));
            }
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                if show_header {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&row.author)
                                .strong()
                                .size(15.5)
                                .color(theme::TEXT_BRIGHT),
                        );
                        ui.label(theme::muted(theme::stamp(row.created_at, now_ms())).size(11.5));
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&row.body).color(theme::TEXT));
                    if row.edited {
                        ui.label(theme::muted("(изменено)").size(11.0));
                    }
                });
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
        let message_id = row.id.clone();
        response.context_menu(|ui| {
            if ui.button("Реакция 👍").clicked() {
                bridge.add_reaction(message_id.clone(), DEFAULT_REACTION.to_owned());
                ui.close();
            }
            if ui.button("Убрать реакцию").clicked() {
                bridge.remove_reaction(message_id.clone(), DEFAULT_REACTION.to_owned());
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
