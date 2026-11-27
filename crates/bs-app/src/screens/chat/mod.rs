//! Основной экран в стиле Discord: колонка пространств, колонка каналов,
//! шапка и лента сообщений, поле ввода, панель участников.

mod downloads;
mod forward;
mod gallery;
mod me_menu;
mod messages;
mod model;
mod panels;
mod saved;

// Части одного экрана, разнесённые по файлам: модель и панели нужны и
// снаружи (`crate::screens::chat::SidebarChannel` и т. п.).
#[allow(clippy::wildcard_imports, reason = "реэкспорт частей экрана")]
pub(crate) use self::model::*;
#[allow(clippy::wildcard_imports, reason = "реэкспорт частей экрана")]
pub(crate) use self::panels::*;

use std::collections::HashMap;

use bs_core::Cache;
use bs_types::{ChannelId, UserStatus};
use egui::{Align, Layout, Margin, RichText, Sense};

use crate::bridge::{BridgeRequest, ChannelKind, HistoryState};
use crate::fleet::Bridge;
use crate::i18n::tr;
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

/// Переход, который пользователь запросил кликом; применяется после отрисовки.
#[derive(Debug)]
pub(super) enum Nav {
    /// Главная: друзья.
    Friends,
    /// Обзор пространств.
    Explore,
    /// Страница «Backspace».
    Backspace,
    /// Открыть пространство.
    Space((usize, String)),
    /// Открыть канал или беседу.
    Channel(SelectedChannel),
    /// Открыть «Избранное».
    Saved,
    /// Окно закладок.
    Bookmarks,
    /// Окно «Создать пространство».
    CreateSpace,
    /// Экран настроек.
    Settings,
    /// Свой профиль (окно, как у других).
    OwnProfile,
    /// Перейти на сохранённый аккаунт или добавить новый (`None`).
    SwitchAccount(Option<usize>),
    /// Искать людей по строке (вкладка «Добавить»).
    SearchPeople(String),
    /// Войти в голосовой канал.
    VoiceJoin {
        origin: usize,
        channel_id: String,
        channel: String,
        space: String,
        space_id: String,
    },
    /// Открыть окно над пространством.
    SpaceDialog(super::space_dialogs::SpaceDialog),
    /// Свой профиль в пространстве.
    SpaceProfile(super::space_profile::SpaceProfileDialog),
    /// Настройки пространства.
    SpaceSettings {
        /// Инстанс.
        origin: usize,
        /// Пространство.
        space_id: String,
    },
    /// Окно «Создать группу».
    GroupCreate,
    /// Настройки уведомлений пространства или канала.
    Notifications {
        /// Инстанс.
        origin: usize,
        /// Пространство.
        space_id: String,
        /// Канал; `None` — всё пространство.
        channel_id: Option<String>,
    },
    /// Настройки канала или категории.
    ChannelSettings {
        /// Инстанс.
        origin: usize,
        /// Пространство.
        space_id: String,
        /// Канал или категория.
        target: super::channel_settings::Target,
    },
    /// Создать ссылку-приглашение и скопировать её.
    Invite { origin: usize, space_id: String },
    /// Пригласить собеседника беседы один на один в одно из своих пространств.
    InvitePerson { origin: usize, dm_id: String },
    /// Перечитать историю мостового чата.
    ResyncDm { origin: usize, dm_id: String },
    /// Удалить переписку у себя (после подтверждения).
    DeleteDm(super::delete_dm::DeleteDm),
    /// Действие панели голоса.
    Voice(crate::voice::DockAction),
    /// Локальная громкость участника.
    LocalAudio(crate::voice_media::LocalAudio),
    /// Модерация голоса.
    VoiceMod {
        origin: usize,
        op: crate::voice::ModOp,
    },
}

/// Сколько секунд подсвечивается сообщение после перехода из поиска.
pub(super) const FLASH_SECONDS: f64 = 2.5;

/// Состояние экрана чата.
#[derive(Debug, Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "флаги состояния экрана, не настройки"
)]
pub(crate) struct ChatScreen {
    /// Запрос на звонок из шапки: беседа, федеративный id и имя собеседника.
    pub(crate) pending_call: Option<(String, Option<String>, String)>,
    /// Голосовой звонок.
    pub(crate) voice: crate::voice::VoiceState,
    /// Рация (push-to-talk) между кадрами.
    ptt: crate::hotkeys::PushToTalk,
    /// Что пришло из трея и от глобальных клавиш в этом кадре.
    desktop_input: crate::desktop::DesktopInput,
    /// Привязки, перехваченные глобально.
    global_keys: std::collections::BTreeSet<crate::prefs::Action>,
    /// Нажата «Вступить» на приглашении.
    pending_join: Option<super::invite_card::InviteJoin>,
    /// Окно «Пригласить людей».
    invite_dialog: Option<super::invite_dialog::InviteDialog>,
    /// «Пригласить в пространство» для друга или собеседника.
    person_invite: Option<super::invite_dialog::PersonInvite>,
    /// Выбранный канал и его инстанс.
    pub(crate) selected_channel: Option<SelectedChannel>,
    pub(crate) draft: String,
    /// Вложения, эмодзи, GIF и стикеры.
    composer: super::composer::Composer,
    /// Что показано в центре.
    view: View,
    /// Состояние друзей и обзора.
    home: HomeState,
    /// Страница «Backspace».
    hub: super::hub::HubPage,
    /// Пространство, открытое в колонке каналов; `None` — главная.
    active_space: Option<(usize, String)>,
    /// Последний открытый канал в каждом пространстве.
    last_by_space: HashMap<(usize, String), String>,
    /// Строка поиска слева сверху: фильтр бесед и каналов.
    sidebar_filter: String,
    /// Открытое окно «Создать пространство».
    create_space: Option<CreateSpaceForm>,
    /// Окно над пространством (вход, новый канал, выход…).
    space_dialog: Option<super::space_dialogs::SpaceDialog>,
    space_settings: Option<super::space_settings::SpaceSettings>,
    channel_settings: Option<super::channel_settings::ChannelSettings>,
    notification_settings: Option<super::notification_settings::NotificationSettings>,
    group_dialog: Option<super::group_dm::GroupDialog>,
    /// Подтверждение «Удалить чат».
    delete_dm: Option<super::delete_dm::DeleteDm>,
    /// Открытое окно шифрования переписки.
    e2ee_dialog: Option<super::e2ee_dialog::E2eeDialog>,
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
    /// Нажали «к последним сообщениям»: прокрутить в самый низ.
    jump_bottom: bool,
    /// Канал, для которого относятся `scroll_anchor` и `was_scrolled_down`.
    scroll_key: Option<String>,
    /// Картинка под курсором при открытии меню: сообщение и значение поля.
    menu_image: Option<(String, String)>,
    /// Открытое окно профиля.
    profile: Option<super::profile::ProfileState>,
    /// Сообщение, на которое пишется ответ.
    reply: Option<ReplyTarget>,
    /// Открытое окно «Переслать».
    forward: Option<forward::ForwardDraft>,
    /// Выбор сообщений галочками для пересылки пачкой.
    picking: Option<forward::Picking>,
    /// Галерея вложений открытого чата.
    gallery: Option<gallery::Gallery>,
    /// Окно своего профиля в пространстве.
    space_profile: Option<super::space_profile::SpaceProfileDialog>,
    /// Сообщение, для которого открыт выбор реакции, и вкладка выбора.
    react_picker: Option<(String, usize)>,
    /// Имена пользователей для упоминаний (обновляется каждый кадр).
    names: HashMap<String, String>,
    /// Ожидаем выбор места для скачиваемого вложения.
    save_pending: Option<crate::download::Pending>,
    /// Поиск по сообщениям текущего канала.
    search: super::search_panel::SearchPanel,
    /// Сообщение, к которому нужно прокрутить ленту (из результатов поиска).
    jump_to: Option<String>,
    /// Подсвеченное после перехода сообщение и момент, когда подсветка гаснет.
    flash: Option<(String, f64)>,
    /// Нажали на цитату ответа: перейти к исходному сообщению.
    reply_jump: Option<String>,
    /// Закрепить, в закладки, в «Избранное» — из меню сообщения.
    extras_action: Option<saved::ExtrasAction>,
    /// Перейти к сообщению, когда откроется его чат.
    pending_jump: Option<(SelectedChannel, String)>,
    /// Открыть чат (из закладок): канал и его пространство.
    pending_nav: Option<(SelectedChannel, Option<String>)>,
    /// Какой закреп показан на панели.
    pin_index: usize,
    /// Открытое окно закладок.
    bookmarks: Option<saved::BookmarksUi>,
}

/// На что отвечаем.
#[derive(Debug, Clone)]
pub(super) struct ReplyTarget {
    message_id: String,
    author: String,
    preview: String,
}

/// Что именно правим.
#[derive(Debug, Clone)]
pub(super) struct EditState {
    message_id: String,
}

impl ChatScreen {
    /// Сбрасывает всё, что относилось к прошлой сессии.
    pub(crate) fn reset(&mut self) {
        *self = Self {
            show_members: true,
            initialized: true,
            ..Self::with_prefs()
        };
    }

    /// Пустой экран с голосовыми настройками из сохранённых.
    /// Принимает события трея и глобальных клавиш (обработаются в `show`).
    pub(crate) fn accept_desktop(
        &mut self,
        input: crate::desktop::DesktopInput,
        global: std::collections::BTreeSet<crate::prefs::Action>,
    ) {
        self.desktop_input.actions.extend(input.actions);
        self.desktop_input.keys.extend(input.keys);
        self.global_keys = global;
    }

    pub(crate) fn with_prefs() -> Self {
        let mut chat = Self::default();
        chat.voice.prefs = crate::prefs::get().voice;
        chat
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
    /// Открытая беседа ушла из списка (её инстанс без связи), а другая её
    /// копия на связи: переходим на копию — и обратно, когда связь вернётся.
    fn follow_dm_copy(&mut self, bridge: &Bridge, dms: &[SidebarDm]) {
        let Some(selected) = self.selected_channel.as_ref() else {
            return;
        };
        if selected.kind != ChannelKind::Dm
            || dms
                .iter()
                .any(|dm| (dm.origin, &dm.id) == (selected.origin, &selected.channel))
        {
            return;
        }
        let Some(key) = bridge
            .cache_at(selected.origin)
            .and_then(|cache| cache.dm_channels().find(|dm| dm.id == selected.channel))
            .and_then(|dm| dm.federated_id.clone())
        else {
            return;
        };
        let Some(copy) = dms
            .iter()
            .find(|dm| dm.key.as_deref() == Some(key.as_str()))
        else {
            return;
        };
        let target = SelectedChannel {
            origin: copy.origin,
            channel: copy.id.clone(),
            kind: ChannelKind::Dm,
        };
        self.open_channel(target);
    }

    pub(crate) fn open_channel(&mut self, selected: SelectedChannel) {
        // Правка относилась к прежнему каналу.
        if self.selected_channel.as_ref() != Some(&selected) {
            self.edit = None;
            self.reply = None;
            self.picking = None;
            self.gallery = None;
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
            View::Friends | View::Explore | View::Backspace => {
                self.active_space.as_ref().map(|(o, _)| *o)
            }
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
    /// «Пригласить в пространство» для собеседника беседы один на один.
    fn open_person_invite(&mut self, bridge: &Bridge, origin: usize, dm_id: &str) {
        let person = bridge.cache_at(origin).and_then(|cache| {
            let me = cache.me().id.clone();
            cache
                .dm_channel(dm_id)?
                .members
                .iter()
                .find(|m| m.id != me)
                .map(super::invite_dialog::friend_from_user)
        });
        if let Some(person) = person {
            self.person_invite = Some(super::invite_dialog::PersonInvite::open(person));
        }
    }

    /// Открыть свой профиль тем же окном, что и чужие.
    fn open_own_profile(&mut self, bridge: &Bridge) {
        // Профиль запрашивается у инстанса, открытого сейчас, — и под его id:
        // домашний id другой инстанс не знает.
        if let Some(me) = bridge.cache().map(|c| c.me().clone()) {
            let name = me
                .display_name
                .clone()
                .unwrap_or_else(|| me.username.clone());
            self.profile = Some(super::profile::ProfileState::open(bridge, &me.id, &name));
        }
    }

    /// Открывает настройки своего аккаунта.
    fn open_settings(&mut self, bridge: &Bridge) {
        self.settings = bridge
            .cache_at(0)
            .map(|cache| SettingsState::from_user(cache.me()));
    }

    fn apply_nav(&mut self, nav: Nav, items: &[SidebarSpace], bridge: &mut Bridge) {
        match nav {
            Nav::Saved => self.open_saved(items, bridge),
            Nav::Bookmarks => self.bookmarks = Some(saved::BookmarksUi::default()),
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
            Nav::Backspace => {
                self.active_space = None;
                self.view = View::Backspace;
                self.hub = super::hub::HubPage::default();
            }
            Nav::Space((origin, id)) => {
                if let Some(space) = items.iter().find(|s| s.origin == origin && s.id == id) {
                    self.open_space(space);
                }
            }
            Nav::Channel(selected) => self.open_channel(selected),
            Nav::CreateSpace => self.create_space = Some(CreateSpaceForm::default()),
            Nav::SpaceDialog(dialog) => self.space_dialog = Some(dialog),
            Nav::SpaceProfile(dialog) => self.space_profile = Some(dialog),
            Nav::SpaceSettings { origin, space_id } => {
                self.space_settings =
                    super::space_settings::SpaceSettings::open(bridge, origin, &space_id);
            }
            Nav::GroupCreate => {
                self.group_dialog =
                    Some(super::group_dm::GroupDialog::create(bridge.focus_index()));
            }
            Nav::Notifications {
                origin,
                space_id,
                channel_id,
            } => {
                self.notification_settings =
                    super::notification_settings::NotificationSettings::open(
                        bridge,
                        origin,
                        &space_id,
                        channel_id.as_deref(),
                    );
            }
            Nav::ChannelSettings {
                origin,
                space_id,
                target,
            } => {
                self.channel_settings = super::channel_settings::ChannelSettings::open(
                    bridge, origin, &space_id, target,
                );
            }
            Nav::Invite { origin, space_id } => {
                self.invite_dialog = Some(super::invite_dialog::InviteDialog::open(
                    bridge, origin, space_id,
                ));
            }
            Nav::InvitePerson { origin, dm_id } => self.open_person_invite(bridge, origin, &dm_id),
            Nav::DeleteDm(dialog) => self.delete_dm = Some(dialog),
            Nav::ResyncDm { origin, dm_id } => resync_dm(bridge, origin, dm_id),
            Nav::Settings => self.open_settings(bridge),
            Nav::OwnProfile => self.open_own_profile(bridge),
            // Обрабатывает приложение (см. конец `show`).
            Nav::SwitchAccount(_) => {}
            Nav::VoiceJoin {
                origin,
                channel_id,
                channel,
                space,
                space_id,
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
                        .join(bridge, origin, &channel_id, &channel, &space, &space_id);
                }
            }
            Nav::Voice(action) => self.voice.apply(action, bridge),
            Nav::VoiceMod { origin, op } => crate::voice::moderate(bridge, origin, op),
            Nav::LocalAudio(audio) => self.voice.set_local_audio(bridge, &audio),
            Nav::SearchPeople(query) => {
                self.active_space = None;
                self.view = View::Friends;
                self.home.start_search(&query, bridge);
                self.sidebar_filter.clear();
            }
        }
    }

    /// Голос: горячие клавиши, трей, звонки, уведомления о них.
    fn drive_voice(&mut self, ctx: &egui::Context, bridge: &mut Bridge, now: f64) {
        let binds = crate::prefs::get().keybinds;
        let muted = self.voice.flags().0;
        let mut actions =
            crate::hotkeys::poll(ctx, &binds, &self.global_keys, &mut self.ptt, muted);
        let desktop = std::mem::take(&mut self.desktop_input);
        for (action, pressed) in desktop.keys {
            crate::hotkeys::dispatch(action, pressed, &mut self.ptt, muted, &mut actions);
        }
        actions.extend(desktop.actions);
        for action in actions {
            self.voice.apply(action, bridge);
        }
        if let Some(info) = bridge.take_info() {
            self.toast = Some((info, now + 3.0));
        }

        self.route_actions(bridge, ctx);
        for (origin, event) in bridge.take_call_events() {
            self.voice.handle_call(bridge, origin, event);
        }
        if let Some((dm_id, federated, peer)) = self.pending_call.take() {
            let origin = bridge.focus_index();
            self.voice
                .start_dm(bridge, origin, &dm_id, federated, &peer);
        }
        self.voice.poll(bridge);
        self.voice.cue(bridge, ctx.input(|i| i.focused));
        if let Some(answer) = crate::dmcall::show_incoming(ctx, &self.voice) {
            self.voice.answer(bridge, answer);
        }
        if let Some(text) = self.voice.notice.take() {
            self.toast = Some((text, now + 3.0));
        }
    }

    /// Центральная область: друзья, обзор или канал.
    fn show_center(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        bridge: &mut Bridge,
        nav: &mut Option<Nav>,
    ) {
        match self.view {
            View::Friends => {
                home::show_active_now(ui, bridge);
                main_module(ui, |ui| home::show_friends(ui, &mut self.home, bridge));
            }
            View::Backspace => {
                let hub = &mut self.hub;
                main_module(ui, |ui| super::hub::show(ui, hub, bridge));
            }
            View::Explore => {
                let mut create = false;
                let target = main_module(ui, |ui| {
                    home::show_explore(ui, &mut self.home, bridge, &mut create)
                });
                if let Some(target) = target {
                    *nav = Some(Nav::Space(target));
                }
                if create {
                    *nav = Some(Nav::CreateSpace);
                }
            }
            View::Channel => {
                if self.show_members {
                    let rows = self.member_panel_rows(bridge);
                    if let Some(rows) = rows
                        && let Some((id, name)) = Self::show_member_panel(ui, &rows)
                    {
                        self.profile = if self.profile.as_ref().is_some_and(|p| p.is_of(&id)) {
                            None
                        } else {
                            Some(super::profile::ProfileState::open(bridge, &id, &name))
                        };
                    }
                }
                main_module(ui, |ui| {
                    self.show_header(ui, bridge);
                    if self.show_nsfw_gate(ui, bridge, nav) {
                        return;
                    }
                    self.show_pins_bar(ui, bridge);
                    self.show_plaintext_notice(ui, bridge);
                    self.show_composer(ui, bridge);
                    self.show_messages(ui, bridge);
                });
                match self.pending_join.take() {
                    Some(super::invite_card::InviteJoin::Slot(origin, code)) => {
                        let previous = bridge.focus_index();
                        bridge.focus_on(origin);
                        bridge.request(BridgeRequest::JoinByCode(code));
                        bridge.focus_on(previous);
                    }
                    Some(super::invite_card::InviteJoin::Connect(origin, code)) => {
                        bridge.connect_and_join(&origin, &code);
                    }
                    None => {}
                }
                self.show_search(ctx, bridge);
            }
        }
    }

    /// Свои профили участников открытого пространства (раз в две минуты) и
    /// окно своего профиля в пространстве.
    fn space_profiles(&mut self, ctx: &egui::Context, bridge: &mut Bridge) {
        if let Some((origin, space_id)) = &self.active_space
            && let Some(session) = bridge.e2ee_session(*origin)
        {
            session.want_space_profiles(space_id);
        }
        if let Some(dialog) = self.space_profile.as_mut()
            && !dialog.show(ctx, bridge)
        {
            self.space_profile = None;
        }
    }

    /// Окна поверх экрана: приглашение, просмотр, профиль, настройки пространства…
    fn show_overlays(&mut self, ctx: &egui::Context, bridge: &mut Bridge) {
        self.space_profiles(ctx, bridge);
        crate::stream_preview::show(ctx, bridge);
        if let Some(draft) = self.forward.as_mut()
            && !forward::show(ctx, draft, bridge)
        {
            self.forward = None;
        }
        if let Some(gallery) = self.gallery.as_mut() {
            match gallery.show(ctx, bridge) {
                gallery::Action::Keep => {}
                gallery::Action::Close => self.gallery = None,
                gallery::Action::Jump(id) => {
                    self.gallery = None;
                    if let Some(selected) = self.selected_channel.clone() {
                        bridge.jump_to(selected.kind, &selected.channel, &id);
                    }
                }
            }
        }
        if let Some(dialog) = self.invite_dialog.as_mut()
            && !super::invite_dialog::show(ctx, dialog, bridge)
        {
            self.invite_dialog = None;
        }
        if let Some(person) = self.home.invite_person.take() {
            self.person_invite = Some(super::invite_dialog::PersonInvite::open(person));
        }
        if let Some(dialog) = self.person_invite.as_mut()
            && !super::invite_dialog::show_person(ctx, dialog, bridge)
        {
            self.person_invite = None;
        }
        if let Some(value) = self.viewer.clone()
            && media::viewer(ctx, &value)
        {
            self.viewer = None;
        }
        let panel_open = self.show_members
            && self.view == View::Channel
            && self.member_panel_rows(bridge).is_some();
        if let Some(state) = self.profile.as_mut() {
            state.poll(bridge);
            state.set_space(
                self.active_space
                    .clone()
                    .filter(|_| self.view == View::Channel),
            );
            let me = bridge
                .cache()
                .map(|cache| cache.me().id.clone())
                .unwrap_or_default();
            let right_edge = ctx.content_rect().right()
                - if panel_open {
                    crate::columns::width(ctx, crate::columns::Side::Right)
                } else {
                    0.0
                };
            if super::profile::show(ctx, state, bridge, &me, right_edge)
                == super::profile::ProfileAction::Close
            {
                self.profile = None;
            }
        }
        if let Some(form) = self.create_space.as_mut()
            && !dialogs::show_create_space(ctx, form, bridge)
        {
            self.create_space = None;
        }
        if let Some(dialog) = self.space_dialog.as_mut()
            && !super::space_dialogs::show(ctx, dialog, bridge)
        {
            self.space_dialog = None;
        }
        if let Some(settings) = self.space_settings.as_mut()
            && !super::space_settings::show(ctx, settings, bridge)
        {
            self.space_settings = None;
        }
        if let Some(settings) = self.channel_settings.as_mut()
            && !super::channel_settings::show(ctx, settings, bridge)
        {
            self.channel_settings = None;
        }
        if let Some(settings) = self.notification_settings.as_mut()
            && !super::notification_settings::show(ctx, settings, bridge)
        {
            self.notification_settings = None;
        }
        self.show_dm_dialogs(ctx, bridge);
    }

    /// Окна личных бесед: группа, удаление, шифрование.
    fn show_dm_dialogs(&mut self, ctx: &egui::Context, bridge: &mut Bridge) {
        if let Some(dialog) = self.group_dialog.as_mut()
            && !super::group_dm::show(ctx, dialog, bridge)
        {
            self.group_dialog = None;
        }
        if let Some(dialog) = self.delete_dm.as_mut()
            && !super::delete_dm::show(ctx, dialog, bridge)
        {
            self.delete_dm = None;
        }
        if let Some(dialog) = self.e2ee_dialog.as_mut()
            && !super::e2ee_dialog::show(ctx, dialog, bridge)
        {
            self.e2ee_dialog = None;
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
        bridge.ensure_home_info();
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let extras_nav = self.drive_extras(bridge, now);
        self.drive_voice(&ctx, bridge, now);
        let (items, dms) = merged_sidebar(bridge);
        self.follow_dm_copy(bridge, &dms);

        self.take_space_events(bridge, &ctx, now);
        if let Some(opened) = bridge.take_opened_space() {
            self.create_space = None;
            self.space_dialog = None;
            self.space_settings = None;
            self.channel_settings = None;
            self.notification_settings = None;
            self.apply_nav(Nav::Space(opened), &items, bridge);
            self.route_actions(bridge, &ctx);
        }

        if let Some(state) = self.settings.as_mut() {
            // Профиль и настройки аккаунта живут на домашнем инстансе: на
            // остальных учётная запись — федеративная копия, и сервер там
            // отказывает в правке имени (`profile_managed_by_home`). Домашний
            // сервер сам разошлёт изменения копиям.
            let focus = bridge.focus_index();
            bridge.focus_on(0);
            let outcome = dialogs::show_settings(ui, state, bridge, &mut self.voice);
            bridge.focus_on(focus);
            match outcome {
                SettingsAction::None => {}
                SettingsAction::Close => self.settings = None,
                SettingsAction::Logout => {
                    self.settings = None;
                    action = ChatAction::Logout;
                }
                SettingsAction::AddAccount => {
                    self.settings = None;
                    action = ChatAction::SwitchAccount(None);
                }
                SettingsAction::Connections(chosen) => route_connection(bridge, chosen),
            }
            self.show_toast(&ctx, now);
            return action;
        }

        let mut nav: Option<Nav> = if let Some((target, space)) = self.pending_nav.take() {
            if let Some(space) = space {
                self.apply_nav(Nav::Space((target.origin, space)), &items, bridge);
            }
            Some(Nav::Channel(target))
        } else {
            extras_nav
        };
        let hosts: Vec<String> = (0..=bridge.live_origins().last().copied().unwrap_or(0))
            .map(|i| {
                bridge
                    .base_at(i)
                    .map(bs_identity::identity::host_of)
                    .unwrap_or_default()
            })
            .collect();
        theme::paint_backdrop(ui);
        self.show_rail(ui, &items, &dms, &hosts, &mut nav);
        self.show_left_column(ui, bridge, &items, &dms, &mut nav);
        crate::stage::show(ui, &mut self.voice, bridge);
        crate::stage::show_picker(&ctx, &mut self.voice);

        self.show_center(ui, &ctx, bridge, &mut nav);

        self.show_overlays(&ctx, bridge);
        self.show_bookmarks(&ctx, bridge);
        self.take_group_events(bridge);
        self.show_toast(&ctx, now);
        if let Some(Nav::SwitchAccount(index)) = nav {
            return ChatAction::SwitchAccount(index);
        }
        if let Some(nav) = nav {
            self.apply_nav(nav, &items, bridge);
        }
        action
    }

    /// Итоги действий над группами.
    fn take_group_events(&mut self, bridge: &mut Bridge) {
        use super::group_dm::Outcome;
        for (origin, event) in bridge.take_group_events() {
            if matches!(event.op, crate::bridge::GroupOp::Resync) {
                bridge.set_info(match &event.result {
                    Ok(()) => tr("История перечитывается — недостающие сообщения появятся в чате")
                        .to_owned(),
                    Err(err) => {
                        crate::tf!("Не удалось перечитать историю: {err}", err = err.clone())
                    }
                });
                continue;
            }
            if self
                .delete_dm
                .as_mut()
                .is_some_and(|dialog| dialog.apply(&event))
            {
                self.delete_dm = None;
            }
            let outcome = self
                .group_dialog
                .as_mut()
                .map_or(Outcome::Keep, |dialog| dialog.apply(&event));
            let left = matches!(
                (&event.op, &event.result),
                (
                    crate::bridge::GroupOp::Leave | crate::bridge::GroupOp::Delete,
                    Ok(())
                )
            );
            if left
                && self
                    .selected_channel
                    .as_ref()
                    .is_some_and(|c| c.origin == origin && c.channel == event.dm_id)
            {
                self.selected_channel = None;
                self.view = View::Friends;
            }
            match outcome {
                Outcome::Keep => {}
                Outcome::Close => self.group_dialog = None,
                Outcome::Open(dm_id) => {
                    self.group_dialog = None;
                    self.open_channel(SelectedChannel {
                        origin,
                        channel: dm_id,
                        kind: ChannelKind::Dm,
                    });
                }
            }
        }
    }

    /// Код приглашения и итог отправки; `true` — событие разобрано.
    fn invite_event(
        &mut self,
        bridge: &mut Bridge,
        ctx: &egui::Context,
        now: f64,
        origin: usize,
        event: &crate::bridge::SpaceEvent,
    ) -> bool {
        use crate::bridge::SpaceEvent;
        match event {
            SpaceEvent::InviteSent { friend_id, result } => {
                if self
                    .person_invite
                    .as_mut()
                    .is_some_and(|dialog| dialog.accept_sent(friend_id, result))
                {
                    return true;
                }
                if let Some(dialog) = self.invite_dialog.as_mut() {
                    dialog.accept_sent(friend_id, result);
                }
                true
            }
            SpaceEvent::Invite { space_id, result } => {
                if self
                    .person_invite
                    .as_mut()
                    .is_some_and(|dialog| dialog.accept_code(bridge, origin, space_id, result))
                {
                    return true;
                }
                if self
                    .invite_dialog
                    .as_mut()
                    .is_some_and(|dialog| dialog.accept_code(origin, result))
                {
                    return true;
                }
                let text = match result {
                    Ok(code) => {
                        let base = bridge.base_at(origin).unwrap_or_default();
                        ctx.copy_text(bs_api::invite_url(base, code));
                        tr("Ссылка-приглашение скопирована").to_owned()
                    }
                    Err(err) => err.clone(),
                };
                self.toast = Some((text, now + 3.0));
                true
            }
            _ => false,
        }
    }

    /// Итоги приглашений, создания каналов, входа и выхода.
    fn take_space_events(&mut self, bridge: &mut Bridge, ctx: &egui::Context, now: f64) {
        use crate::bridge::{SpaceEvent, SpaceOp};
        for (origin, event) in bridge.take_space_events() {
            if self.invite_event(bridge, ctx, now, origin, &event) {
                continue;
            }
            match &event {
                SpaceEvent::Left {
                    space_id,
                    result: Ok(()),
                }
                | SpaceEvent::Admin {
                    space_id,
                    op: SpaceOp::Delete,
                    result: Ok(()),
                    ..
                } => {
                    let text = if matches!(event, SpaceEvent::Left { .. }) {
                        tr("Вы покинули пространство")
                    } else {
                        tr("Пространство удалено")
                    };
                    self.toast = Some((text.to_owned(), now + 3.0));
                    if self
                        .active_space
                        .as_ref()
                        .is_some_and(|(o, id)| *o == origin && id == space_id)
                    {
                        self.active_space = None;
                        self.selected_channel = None;
                        self.view = View::Friends;
                        bridge.request(BridgeRequest::Friends);
                    }
                }
                SpaceEvent::Admin {
                    op: SpaceOp::DeleteChannel(id),
                    result: Ok(()),
                    ..
                } if self
                    .selected_channel
                    .as_ref()
                    .is_some_and(|c| c.origin == origin && c.channel.as_str() == id) =>
                {
                    self.selected_channel = None;
                    self.view = View::Friends;
                }
                _ => {}
            }
            if let Some(settings) = self.notification_settings.as_mut()
                && !settings.apply(&event)
            {
                self.notification_settings = None;
            }
            if let Some(settings) = self.channel_settings.as_mut()
                && !settings.apply(&event)
            {
                self.channel_settings = None;
            }
            if let Some(dialog) = self.space_dialog.as_mut()
                && !dialog.apply(&event)
            {
                self.space_dialog = None;
            }
            if let Some(settings) = self.space_settings.as_mut()
                && !settings.apply(&event)
            {
                self.space_settings = None;
            }
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

/// Главный модуль окна (лента, друзья, обзор) — скруглённая карточка
/// на подложке; вложенные панели рисуются уже без своей заливки.
fn main_module<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::CentralPanel::default()
        .frame(theme::module(theme::bg_chat()))
        .show(ui, add)
        .inner
}

/// Просит сервер перечитать историю мостового чата.
fn resync_dm(bridge: &mut Bridge, origin: usize, dm_id: String) {
    bridge.focus_on(origin);
    bridge.request(BridgeRequest::GroupAdmin {
        dm_id,
        op: crate::bridge::GroupOp::Resync,
    });
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
            rights: super::SpaceRights::default(),
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
            face: (id.to_owned(), None),
            bridged: false,
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
    fn dm_moves_to_a_connected_copy_when_home_is_down() {
        let parts = || {
            vec![
                (
                    0,
                    "https://home.example",
                    Vec::new(),
                    vec![dm_item("9", Some("k"), 10), dm_item("8", Some("only"), 5)],
                ),
                (
                    1,
                    "https://orbit.example",
                    Vec::new(),
                    vec![dm_item("77", Some("k"), 10)],
                ),
            ]
        };
        let ids = |dms: Vec<super::SidebarDm>| -> Vec<(usize, String)> {
            dms.into_iter().map(|d| (d.origin, d.id)).collect()
        };
        // Всё на связи — домашняя копия.
        let (_, dms) = super::merge_preferring(parts(), |_| true);
        assert_eq!(ids(dms), vec![(0, "9".into()), (0, "8".into())]);
        // Дом без связи — копия пира; беседа без копии остаётся домашней.
        let (_, dms) = super::merge_preferring(parts(), |origin| origin != 0);
        assert_eq!(ids(dms), vec![(1, "77".into()), (0, "8".into())]);
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
            face: (author.to_owned(), None),
            attachments: Vec::new(),
            embeds: Vec::new(),
            reactions: Vec::new(),
            reply: None,
            system: None,
            name_color: None,
            bot: false,
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

    #[test]
    fn voice_occupants_carry_avatars() {
        let mut payload = ready("s1", "Работа", "c1", "general");
        let user = |id: &str, name: &str, avatar: serde_json::Value| {
            serde_json::json!({
                "spaceId": "s1", "userId": id, "nickname": null, "joinedAt": 1,
                "user": {
                    "id": id, "username": name, "displayName": null, "avatar": avatar,
                    "banner": null, "accentColor": null, "avatarColor": null, "bio": null,
                    "status": "online", "customStatus": null, "isAdmin": false,
                    "createdAt": 1, "homeInstance": null, "homeUserId": null,
                    "replicatedInstances": []
                },
                "roles": []
            })
        };
        payload.spaces[0].members = serde_json::from_value(serde_json::json!([
            user("2", "bob", serde_json::json!("bob.png")),
            user("3", "carl", serde_json::Value::Null),
        ]))
        .expect("members");
        payload.voice_states = Some(
            std::iter::once(("c1".to_owned(), vec!["2".to_owned(), "3".to_owned()])).collect(),
        );
        let cache = Cache::from_ready(payload);
        let found = super::occupants(&cache, &super::name_map(&cache), "c1");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].avatar.as_deref(), Some("bob.png"));
        assert_eq!(found[1].avatar, None);
    }

    #[test]
    fn new_message_alert_wiring() {
        use crate::bridge::NewMessage;
        let cache = Cache::from_ready(ready("s1", "Работа", "c1", "general"));
        let me = cache.me().id.clone();
        let msg = |dm: bool, channel: &str, author: &str, content: &str| NewMessage {
            dm,
            channel_id: channel.to_owned(),
            author_id: author.to_owned(),
            content: content.to_owned(),
            mention_everyone: false,
            mention_roles: Vec::new(),
        };
        let alerts = |m: &NewMessage| crate::voice::message_alerts_user(&cache, m);
        // Личное от другого — да; своё — нет; неизвестный канал — нет.
        assert!(alerts(&msg(true, "d1", "2", "привет")));
        assert!(!alerts(&msg(true, "d1", &me, "привет")));
        assert!(!alerts(&msg(false, "нет-такого", "2", "привет")));
        // Прямое упоминание проходит при любом уровне, кроме «ничего»/mute;
        // свои сообщения не звучат никогда.
        assert!(alerts(&msg(false, "c1", "2", &format!("эй <@{me}>"))));
        assert!(!alerts(&msg(false, "c1", &me, &format!("<@{me}>"))));
    }
}
