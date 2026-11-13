//! Диалоги: «Создать пространство» и экран настроек пользователя.

mod account;
mod voice;

use self::account::{account, connections, privacy};
use self::voice::voice_settings;

use bs_api::{CreateSpaceRequest, UpdateProfileRequest};
use bs_types::{ChosenUserStatus, SpaceVisibility, User, UserStatus};
use egui::{Align, Color32, CornerRadius, Layout, Margin, RichText, Sense};

use crate::bridge::BridgeRequest;
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::social::user_display_name;
use crate::theme;
use crate::widgets;

/// Максимальная длина имени пространства (как у сервера).
const SPACE_NAME_MAX: usize = 100;
/// Максимальная длина описания пространства.
const SPACE_DESCRIPTION_MAX: usize = 200;

/// Форма «Создать пространство».
#[derive(Debug)]
pub(crate) struct CreateSpaceForm {
    name: String,
    description: String,
    visibility: SpaceVisibility,
    color: &'static str,
    /// Форма отправлена: ошибка сервера показывается только после этого.
    submitted: bool,
}

impl Default for CreateSpaceForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            visibility: SpaceVisibility::Private,
            color: "lavender",
            submitted: false,
        }
    }
}

impl CreateSpaceForm {
    /// Тело запроса, если форму можно отправлять.
    fn request(&self) -> Option<CreateSpaceRequest> {
        let name = self.name.trim();
        if name.is_empty() {
            return None;
        }
        let description = self.description.trim();
        Some(CreateSpaceRequest {
            name: name.to_owned(),
            visibility: Some(self.visibility),
            description: (!description.is_empty()).then(|| description.to_owned()),
            avatar_color: Some(self.color.to_owned()),
        })
    }
}

/// Значок будущего пространства и выбор его цвета.
fn space_icon_picker(ui: &mut egui::Ui, form: &mut CreateSpaceForm) {
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(76.0), Sense::hover());
        let color = theme::palette(form.color).unwrap_or_else(theme::accent);
        ui.painter().circle_filled(rect.center(), 38.0, color);
        let letter = form
            .name
            .trim()
            .chars()
            .next()
            .map_or_else(|| "S".to_owned(), |c| c.to_uppercase().collect());
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            letter,
            egui::FontId::new(30.0, theme::bold_family()),
            egui::Color32::WHITE,
        );
    });
    widgets::field_label(ui, tr("Цвет значка"));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for name in theme::PALETTE_NAMES {
            let color = theme::palette(name).unwrap_or_else(theme::accent);
            if widgets::color_swatch(ui, color, form.color == name, 30.0).clicked() {
                form.color = name;
            }
        }
    });
}

/// Приватное, по заявке или публичное.
fn visibility_picker(ui: &mut egui::Ui, visibility: &mut SpaceVisibility) {
    widgets::field_label(ui, tr("Видимость"));
    for (value, title, hint) in [
        (
            SpaceVisibility::Private,
            tr("Приватное"),
            tr("Только по ссылке-приглашению"),
        ),
        (
            SpaceVisibility::Request,
            tr("По заявке"),
            tr("Видно в обзоре, вступление по запросу"),
        ),
        (
            SpaceVisibility::Public,
            tr("Публичное"),
            tr("Видно в обзоре, вступить может любой"),
        ),
    ] {
        if widgets::radio_card(ui, title, hint, *visibility == value).clicked() {
            *visibility = value;
        }
    }
}

/// Рисует модальное окно создания. Возвращает `false`, когда его нужно закрыть.
pub(crate) fn show_create_space(
    ctx: &egui::Context,
    form: &mut CreateSpaceForm,
    bridge: &Bridge,
) -> bool {
    let mut keep_open = true;
    let busy = form.submitted && bridge.social_error().is_none();
    let response = egui::Modal::new(egui::Id::unique("create_space"))
        .frame(
            egui::Frame::new()
                .fill(theme::bg_bubble())
                .stroke(egui::Stroke::new(1.0, theme::border()))
                .corner_radius(crate::theme::rounded(16))
                .inner_margin(Margin::same(22)),
        )
        .show(ctx, |ui| {
            ui.set_width(400.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(tr("Создать пространство"))
                        .size(19.0)
                        .family(theme::bold_family())
                        .color(theme::text_bright()),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, Icon::Close, 30.0).clicked() {
                        keep_open = false;
                    }
                });
            });
            ui.add_space(12.0);
            space_icon_picker(ui, form);
            widgets::field_label(ui, tr("Название"));
            let name_response = widgets::text_field(
                ui,
                &mut form.name,
                tr("Моё замечательное пространство"),
                SPACE_NAME_MAX,
            );
            visibility_picker(ui, &mut form.visibility);
            widgets::field_label(ui, tr("Описание"));
            widgets::text_area(
                ui,
                &mut form.description,
                tr("Коротко о вашем пространстве…"),
                SPACE_DESCRIPTION_MAX,
                3,
            );
            if form.submitted
                && let Some(err) = bridge.social_error()
            {
                ui.add_space(6.0);
                ui.label(RichText::new(err).color(theme::rose()));
            }
            ui.add_space(14.0);
            let request = form.request();
            let enter = name_response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let create = ui.add_enabled(
                    request.is_some() && !busy,
                    theme::primary_button(if busy {
                        tr("Создаём…")
                    } else {
                        tr("Создать")
                    }),
                );
                if create.clicked() || (enter && !busy) {
                    if let Some(request) = request {
                        form.submitted = true;
                        bridge.request(BridgeRequest::CreateSpace(request));
                    }
                }
                if ui.button(tr("Отмена")).clicked() {
                    keep_open = false;
                }
            });
        });
    if response.should_close() {
        keep_open = false;
    }
    keep_open
}

/// Раздел настроек.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsSection {
    /// Профиль и статус.
    Account,
    /// Темы, масштаб, плотность.
    Appearance,
    /// Голос и видео.
    Voice,
    /// Горячие клавиши.
    Hotkeys,
    /// Системные уведомления.
    Notifications,
    /// Язык интерфейса.
    Language,
    /// Версия клиента и проверка его обновлений.
    About,
    /// Приватность.
    Privacy,
    /// Соединение с инстансом.
    Connections,
    /// Администратор: общие настройки инстанса.
    Instance,
    /// Администратор: пользователи.
    Users,
    /// Администратор: хранилище.
    Storage,
    /// Администратор: регистрация и инвайты.
    Registration,
    /// Администратор: федерация.
    Federation,
    /// Администратор: стриминг.
    Streaming,
    /// Администратор: телеметрия.
    Telemetry,
    /// Администратор: обновления.
    Updates,
}

impl SettingsSection {
    fn title(self) -> &'static str {
        match self {
            Self::Account => tr("Аккаунт"),
            Self::Appearance => tr("Внешний вид"),
            Self::Voice => tr("Голос и видео"),
            Self::Hotkeys => tr("Горячие клавиши"),
            Self::Notifications => tr("Уведомления"),
            Self::Language => tr("Язык"),
            Self::About => tr("О клиенте"),
            Self::Privacy => tr("Приватность"),
            Self::Connections => tr("Подключения"),
            Self::Instance => tr("Инстанс"),
            Self::Users => tr("Пользователи"),
            Self::Storage => tr("Хранилище"),
            Self::Registration => tr("Регистрация"),
            Self::Federation => tr("Федерация"),
            Self::Streaming => tr("Стриминг"),
            Self::Telemetry => tr("Телеметрия"),
            Self::Updates => tr("Обновления"),
        }
    }

    /// Раздел только для администратора.
    fn is_admin(self) -> bool {
        matches!(
            self,
            Self::Instance
                | Self::Users
                | Self::Storage
                | Self::Registration
                | Self::Federation
                | Self::Streaming
                | Self::Telemetry
                | Self::Updates
        )
    }
}

/// Что пользователь попросил в настройках.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingsAction {
    /// Ничего.
    None,
    /// Закрыть настройки.
    Close,
    /// Выйти из всех инстансов.
    Logout,
    /// Выйти и войти в другой инстанс.
    SwitchInstance,
    /// Действие в панели «Подключения».
    Connections(super::connections::ConnAction),
}

/// Редактируемая копия профиля и локальные настройки.
#[derive(Debug)]
pub(crate) struct SettingsState {
    section: SettingsSection,
    display_name: String,
    custom_status: String,
    bio: String,
    status: ChosenUserStatus,
    avatar_color: Option<String>,
    discoverable: bool,
    show_activity: bool,
    /// Страница «Внешний вид».
    appearance: super::appearance::AppearanceState,
    /// Страница «Горячие клавиши».
    hotkeys: super::settings_app::HotkeysState,
    /// До какого момента (`ui.input().time`) идёт проверка микрофона.
    mic_test_until: Option<f64>,
    /// Почему проверка не запустилась.
    mic_test_error: Option<String>,
    /// Состояние панели «Подключения».
    connections: super::connections::ConnectionsUi,
    /// Картинки профиля, пароль и админские разделы.
    extra: super::settings_extra::ExtraUi,
    /// Инстансы, где этот пользователь администратор: номер и адрес.
    admin_slots: Vec<(usize, String)>,
    /// Выбранный из них.
    admin_slot: Option<usize>,
    /// Устройства для раздела «Голос и видео»: микрофоны и воспроизведение.
    voice_devices: Option<(
        Vec<crate::voice_media::Device>,
        Vec<crate::voice_media::Device>,
    )>,
}

/// Выбранный статус из текущего присутствия (`offline` выбрать нельзя).
fn chosen_from(status: UserStatus) -> ChosenUserStatus {
    match status {
        UserStatus::Idle => ChosenUserStatus::Idle,
        UserStatus::DoNotDisturb => ChosenUserStatus::DoNotDisturb,
        UserStatus::Online | UserStatus::Offline => ChosenUserStatus::Online,
    }
}

impl SettingsState {
    /// Копия текущего профиля для редактирования.
    pub(crate) fn from_user(user: &User) -> Self {
        Self {
            section: SettingsSection::Account,
            display_name: user.display_name.clone().unwrap_or_default(),
            custom_status: user.custom_status.clone().unwrap_or_default(),
            bio: user.bio.clone().unwrap_or_default(),
            status: chosen_from(user.status),
            avatar_color: user.avatar_color.clone(),
            discoverable: user.discoverable.unwrap_or(true),
            show_activity: user.show_activity.unwrap_or(true),
            appearance: super::appearance::AppearanceState::default(),
            hotkeys: super::settings_app::HotkeysState::default(),
            mic_test_until: None,
            mic_test_error: None,
            connections: super::connections::ConnectionsUi::default(),
            extra: super::settings_extra::ExtraUi::default(),
            admin_slots: Vec::new(),
            admin_slot: None,
            voice_devices: None,
        }
    }

    /// Только изменённые относительно `user` поля; `None`, если менять нечего.
    fn diff(&self, user: &User) -> Option<UpdateProfileRequest> {
        let mut request = UpdateProfileRequest::default();
        let mut changed = false;
        let mut text = |new: &str, old: Option<&str>, slot: &mut Option<String>| {
            let new = new.trim();
            if new != old.unwrap_or("").trim() {
                *slot = Some(new.to_owned());
                changed = true;
            }
        };
        text(
            &self.display_name,
            user.display_name.as_deref(),
            &mut request.display_name,
        );
        text(&self.bio, user.bio.as_deref(), &mut request.bio);
        text(
            &self.custom_status,
            user.custom_status.as_deref(),
            &mut request.custom_status,
        );
        if self.status != chosen_from(user.status) {
            request.status = Some(self.status);
            changed = true;
        }
        if self.avatar_color != user.avatar_color {
            request.avatar_color.clone_from(&self.avatar_color);
            changed = true;
        }
        if self.discoverable != user.discoverable.unwrap_or(true) {
            request.discoverable = Some(self.discoverable);
            changed = true;
        }
        if self.show_activity != user.show_activity.unwrap_or(true) {
            request.show_activity = Some(self.show_activity);
            changed = true;
        }
        changed.then_some(request)
    }
}

/// Рисует экран настроек вместо основного интерфейса.
pub(crate) fn show_settings(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    bridge: &mut Bridge,
    voice: &mut crate::voice::VoiceState,
) -> SettingsAction {
    // Админские разделы относятся к инстансу, где пользователь администратор,
    // даже если в чате сейчас открыт другой (например, пиринговый).
    state.admin_slots = bridge
        .live_origins()
        .into_iter()
        .filter(|i| bridge.cache_at(*i).is_some_and(|c| c.me().is_admin))
        .map(|i| {
            (
                i,
                bridge
                    .base_at(i)
                    .unwrap_or_else(|| tr("домашний"))
                    .to_owned(),
            )
        })
        .collect();
    let previous = bridge.focus_index();
    if state.section.is_admin() {
        let target = state
            .admin_slot
            .filter(|i| state.admin_slots.iter().any(|(s, _)| s == i))
            .or_else(|| state.admin_slots.first().map(|(i, _)| *i));
        if let Some(target) = target {
            bridge.focus_on(target);
        }
    }
    let action = show_settings_inner(ui, state, bridge, voice);
    bridge.focus_on(previous);
    action
}

fn show_settings_inner(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    bridge: &mut Bridge,
    voice: &mut crate::voice::VoiceState,
) -> SettingsAction {
    let mut action = SettingsAction::None;
    state.extra.poll(bridge);
    if state.extra.account.logout {
        return SettingsAction::Logout;
    }
    let Some(me) = bridge.cache().map(|cache| cache.me().clone()) else {
        return SettingsAction::Close;
    };
    // Окно с отступом от краёв, как в оригинале: затемнённый фон и скруглённая рамка.
    let outer = ui.max_rect();
    let window = outer.shrink2(egui::vec2(
        (outer.width() * 0.05).clamp(8.0, 56.0),
        (outer.height() * 0.07).clamp(8.0, 50.0),
    ));
    ui.painter()
        .rect_filled(outer, 0.0, Color32::from_rgb(0x08, 0x08, 0x0c));
    ui.painter().rect(
        window,
        crate::theme::rounded(16),
        theme::bg_rail(),
        egui::Stroke::new(1.0, theme::border()),
        egui::StrokeKind::Inside,
    );
    let mut ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(window.shrink(1.0))
            .layout(Layout::top_down(Align::Min)),
    );
    ui.set_clip_rect(window.shrink(1.0));
    let ui = &mut ui;
    egui::Panel::left("settings_nav")
        .exact_size(280.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(Color32::TRANSPARENT)
                .inner_margin(Margin::symmetric(16, 16)),
        )
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("settings_nav_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| settings_nav(ui, state, &me, &mut action));
        });
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(Color32::TRANSPARENT)
                .inner_margin(Margin::symmetric(24, 22)),
        )
        .show(ui, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                if widgets::icon_button(ui, Icon::Close, 38.0)
                    .on_hover_text(tr("Закрыть (Esc)"))
                    .clicked()
                {
                    action = SettingsAction::Close;
                }
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_max_width(600.0);
                            section_content(ui, state, &me, bridge, voice, &mut action);
                        });
                });
            });
        });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = SettingsAction::Close;
    }
    action
}

/// Карточка профиля над разделами настроек.
fn nav_profile(ui: &mut egui::Ui, me: &User) {
    let name = user_display_name(me.display_name.as_deref(), &me.username);
    egui::Frame::new()
        .fill(theme::bg_sidebar())
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(crate::theme::rounded(10))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let pair = super::chat::face_of(me);
                widgets::big_avatar(
                    ui,
                    super::chat::face(&name, &pair),
                    38.0,
                    me.avatar.as_deref(),
                );
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(
                        RichText::new(theme::truncate_chars(&name, 18))
                            .family(theme::bold_family())
                            .color(theme::text_bright()),
                    );
                    ui.label(theme::muted(format!("@{}", me.username)).size(12.0));
                });
            });
        });
}

/// Выбор инстанса для администрирования, когда прав хватает на нескольких.
fn admin_slot_picker(ui: &mut egui::Ui, state: &mut SettingsState) {
    if state.admin_slots.len() > 1 {
        let current = state.admin_slot.unwrap_or(state.admin_slots[0].0);
        let label = state
            .admin_slots
            .iter()
            .find(|(i, _)| *i == current)
            .map_or("", |(_, base)| base.as_str())
            .to_owned();
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            egui::ComboBox::from_id_salt("admin_slot")
                .selected_text(theme::truncate_chars(&label, 26))
                .show_ui(ui, |ui| {
                    for (index, base) in &state.admin_slots {
                        if ui.selectable_label(*index == current, base).clicked()
                            && *index != current
                        {
                            state.admin_slot = Some(*index);
                            state.extra.admin = super::settings_extra::AdminUi::default();
                        }
                    }
                });
        });
        ui.add_space(4.0);
    }
}

/// Группа разделов в левой колонке настроек.
fn nav_group(
    ui: &mut egui::Ui,
    title: &str,
    items: &[SettingsSection],
    section: &mut SettingsSection,
) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        widgets::section_label(ui, title);
    });
    ui.add_space(2.0);
    for &item in items {
        if theme::list_row(ui, item.title(), *section == item, false).clicked() {
            *section = item;
        }
    }
    ui.add_space(8.0);
}

/// Левая колонка: карточка профиля и список разделов.
fn settings_nav(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    me: &User,
    action: &mut SettingsAction,
) {
    nav_profile(ui, me);
    ui.add_space(10.0);
    egui::Frame::new()
        .fill(theme::bg_sidebar())
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(crate::theme::rounded(10))
        .inner_margin(Margin::symmetric(8, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            nav_group(
                ui,
                tr("Пользователь"),
                &[
                    SettingsSection::Account,
                    SettingsSection::Appearance,
                    SettingsSection::Voice,
                    SettingsSection::Privacy,
                ],
                &mut state.section,
            );
            nav_group(
                ui,
                tr("Приложение"),
                &[
                    SettingsSection::Notifications,
                    SettingsSection::Hotkeys,
                    SettingsSection::Language,
                    SettingsSection::Connections,
                    SettingsSection::About,
                ],
                &mut state.section,
            );
            if !state.admin_slots.is_empty() {
                admin_slot_picker(ui, state);
                nav_group(
                    ui,
                    tr("Администрирование"),
                    &[
                        SettingsSection::Instance,
                        SettingsSection::Users,
                        SettingsSection::Storage,
                        SettingsSection::Registration,
                        SettingsSection::Federation,
                        SettingsSection::Streaming,
                        SettingsSection::Telemetry,
                        SettingsSection::Updates,
                    ],
                    &mut state.section,
                );
            }
            ui.add_space(6.0);
            if sidebar_item(ui, tr("Сменить инстанс"), theme::text())
                .on_hover_text(tr("Выйти и войти в другой инстанс Backspace"))
                .clicked()
            {
                *action = SettingsAction::SwitchInstance;
            }
            if sidebar_item(ui, tr("Выйти"), theme::rose()).clicked() {
                *action = SettingsAction::Logout;
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(theme::muted("Backspace RS · AGPL-3.0").size(11.5));
            });
        });
}

/// Строка-действие внизу боковой панели настроек.
fn sidebar_item(ui: &mut egui::Ui, label: &str, color: egui::Color32) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(
            rect.shrink2(egui::vec2(8.0, 1.0)),
            crate::theme::rounded(8),
            theme::bg_hover(),
        );
    }
    ui.painter().text(
        egui::pos2(rect.left() + 18.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(15.0),
        color,
    );
    response
}

/// Карточка раздела: подложка с тонкой рамкой.
pub(super) fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::bg_sidebar())
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(crate::theme::rounded(10))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// Заголовок страницы настроек.
pub(super) fn page_title(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(22.0)
            .family(theme::bold_family())
            .color(theme::text_bright()),
    );
    ui.add_space(14.0);
}

/// Содержимое выбранного раздела.
fn section_content(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    me: &User,
    bridge: &Bridge,
    voice: &mut crate::voice::VoiceState,
    action: &mut SettingsAction,
) {
    match state.section {
        SettingsSection::Account => account(ui, state, me, bridge),
        SettingsSection::Appearance => super::appearance::page(ui, &mut state.appearance),
        SettingsSection::Voice => voice_settings(ui, state, bridge, voice),
        SettingsSection::Hotkeys => super::settings_app::hotkeys(ui, &mut state.hotkeys),
        SettingsSection::Notifications => super::settings_app::notifications(ui),
        SettingsSection::Language => super::settings_app::language(ui),
        SettingsSection::About => super::settings_app::about(ui, bridge),
        SettingsSection::Privacy => privacy(ui, state, me, bridge),
        SettingsSection::Connections => connections(ui, state, bridge, action),
        SettingsSection::Instance if me.is_admin => {
            super::settings_extra::instance_panel(ui, &mut state.extra, bridge);
        }
        SettingsSection::Users if me.is_admin => {
            super::settings_extra::users_panel(ui, &mut state.extra, me, bridge);
        }
        SettingsSection::Storage if me.is_admin => {
            super::settings_extra::storage_panel(ui, &mut state.extra, bridge);
        }
        SettingsSection::Registration if me.is_admin => {
            super::settings_invites::registration_panel(ui, &mut state.extra, bridge);
        }
        SettingsSection::Federation if me.is_admin => {
            super::settings_more::federation_panel(ui, &mut state.extra, bridge);
        }
        SettingsSection::Streaming if me.is_admin => {
            super::settings_more::streaming_panel(ui, &mut state.extra.admin.more, bridge);
        }
        SettingsSection::Telemetry if me.is_admin => {
            super::settings_more::telemetry_panel(ui, &mut state.extra.admin.more, bridge);
        }
        SettingsSection::Updates if me.is_admin => {
            super::settings_more::updates_panel(ui, &mut state.extra.admin.more, bridge);
        }
        SettingsSection::Instance
        | SettingsSection::Users
        | SettingsSection::Storage
        | SettingsSection::Registration
        | SettingsSection::Federation
        | SettingsSection::Streaming
        | SettingsSection::Telemetry
        | SettingsSection::Updates => {
            page_title(ui, tr("Недоступно"));
            card(ui, |ui| {
                ui.label(tr("Этот раздел только для администратора инстанса."));
            });
        }
    }
}

/// Панель «Сохранить / Сбросить» под формой.
fn save_bar(ui: &mut egui::Ui, state: &mut SettingsState, me: &User, bridge: &Bridge) {
    let request = state.diff(me);
    ui.add_space(14.0);
    if let Some(err) = bridge.social_error() {
        ui.label(RichText::new(err).color(theme::rose()));
        ui.add_space(6.0);
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(request.is_some(), theme::primary_button(tr("Сохранить")))
            .clicked()
            && let Some(request) = request
        {
            bridge.request(BridgeRequest::UpdateProfile(request));
        }
        if ui
            .add_enabled(state.diff(me).is_some(), egui::Button::new(tr("Сбросить")))
            .clicked()
        {
            *state = SettingsState::from_user(me);
            state.section = SettingsSection::Account;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{CreateSpaceForm, SettingsState};
    use bs_types::SpaceVisibility;

    #[allow(clippy::expect_used)]
    fn user() -> bs_types::User {
        serde_json::from_value(serde_json::json!({
            "id": "1", "username": "alice", "displayName": "Alice", "avatar": null,
            "banner": null, "accentColor": null, "avatarColor": "mint", "bio": null,
            "status": "online", "customStatus": null, "isAdmin": false, "createdAt": 0,
            "homeInstance": null, "homeUserId": null, "replicatedInstances": []
        }))
        .expect("пользователь")
    }

    #[test]
    fn unchanged_settings_produce_no_request() {
        let me = user();
        assert!(SettingsState::from_user(&me).diff(&me).is_none());
    }

    #[test]
    fn only_changed_fields_are_sent() {
        let me = user();
        let mut state = SettingsState::from_user(&me);
        state.custom_status = "  busy  ".into();
        state.discoverable = false;
        let request = state.diff(&me).expect("есть изменения");
        assert_eq!(request.custom_status.as_deref(), Some("busy"));
        assert_eq!(request.discoverable, Some(false));
        assert!(request.display_name.is_none());
        assert!(request.status.is_none());
    }

    #[test]
    fn create_form_needs_a_name() {
        let mut form = CreateSpaceForm::default();
        assert!(form.request().is_none());
        form.name = "  Dev  ".into();
        form.visibility = SpaceVisibility::Public;
        let request = form.request().expect("имя задано");
        assert_eq!(request.name, "Dev");
        assert_eq!(request.visibility, Some(SpaceVisibility::Public));
        assert!(request.description.is_none());
    }
}
