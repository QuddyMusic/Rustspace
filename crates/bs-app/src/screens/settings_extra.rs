//! Настройки аккаунта (картинки, пароль, удаление) и разделы администратора
//! инстанса: настройки, пользователи, хранилище.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use bs_api::{AdminUserPage, CleanupResult, InstanceSettings, InstanceSettingsPatch, StorageStats};
use bs_types::User;
use egui::{Align, Layout, RichText};

use crate::bridge::{AdminDone, AdminEvent, AdminRequest, BridgeRequest, ImageKind, Secret};
use crate::fleet::Bridge;
use crate::media::human_size;
use crate::screens::dialogs::{card, page_title};
use crate::theme;
use crate::widgets;

/// Минимальная длина пароля (как на сервере).
const PASSWORD_MIN: usize = 8;
/// Самый большой файл картинки профиля.
const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;

type Picked = (ImageKind, String, Result<Vec<u8>, String>);

/// Итог последнего действия: успех или ошибка.
#[derive(Debug, Clone)]
pub(super) struct Note {
    text: String,
    ok: bool,
}

pub(super) fn show_note(ui: &mut egui::Ui, note: Option<&Note>) {
    if let Some(note) = note {
        let color = if note.ok { theme::GREEN } else { theme::ROSE };
        ui.label(RichText::new(&note.text).color(color).size(13.0));
        ui.add_space(6.0);
    }
}

/// Состояние дополнительных частей «Аккаунта».
#[derive(Debug, Default)]
pub(crate) struct AccountUi {
    current: String,
    new: String,
    repeat: String,
    delete_open: bool,
    delete_password: String,
    delete_username: String,
    picking: Option<ReceiverBox>,
    note: Option<Note>,
    password_note: Option<Note>,
    delete_note: Option<Note>,
    /// После успешной смены пароля или удаления нужно выйти.
    pub(crate) logout: bool,
}

struct ReceiverBox(Receiver<Picked>);

impl std::fmt::Debug for ReceiverBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Receiver")
    }
}

/// Состояние админских разделов.
#[allow(
    clippy::struct_excessive_bools,
    reason = "флаги «уже запрашивали» по разделам"
)]
#[derive(Debug, Default)]
pub(crate) struct AdminUi {
    pub(crate) more: super::settings_more::MoreUi,
    settings: Option<InstanceSettings>,
    edit: Option<InstanceSettings>,
    gif_key: String,
    settings_asked: bool,
    settings_note: Option<Note>,
    users: Option<AdminUserPage>,
    users_asked: bool,
    query: String,
    asked_query: String,
    page: u32,
    show_deleted: bool,
    confirm_delete: Option<String>,
    reset_for: Option<String>,
    temp_password: Option<(String, String)>,
    users_note: Option<Note>,
    storage: Option<StorageStats>,
    storage_asked: bool,
    cleanup: Option<CleanupResult>,
    confirm_cleanup: bool,
    storage_note: Option<Note>,
}

/// Всё, что хранят разделы этого модуля.
#[derive(Debug, Default)]
pub(crate) struct ExtraUi {
    pub(crate) account: AccountUi,
    pub(crate) admin: AdminUi,
}

impl ExtraUi {
    /// Разбирает ответы моста.
    pub(crate) fn poll(&mut self, bridge: &mut Bridge) {
        for event in bridge.take_admin_events() {
            let admin = &mut self.admin;
            let account = &mut self.account;
            let Some(event) = admin.more.accept(event) else {
                continue;
            };
            match event {
                AdminEvent::Settings(result) => match result {
                    Ok(settings) => {
                        admin.edit = Some(settings.clone());
                        admin.settings = Some(settings);
                        admin.gif_key.clear();
                    }
                    Err(message) => admin.settings_note = Some(fail(message)),
                },
                AdminEvent::SettingsSaved(result) => match result {
                    Ok(settings) => {
                        admin.edit = Some(settings.clone());
                        admin.settings = Some(settings);
                        admin.gif_key.clear();
                        admin.settings_note = Some(ok("Настройки сохранены"));
                    }
                    Err(message) => admin.settings_note = Some(fail(message)),
                },
                AdminEvent::Users(result) => match result {
                    Ok(page) => {
                        admin.users = Some(page);
                        admin.users_note = None;
                    }
                    Err(message) => admin.users_note = Some(fail(message)),
                },
                AdminEvent::Storage(result) => match result {
                    Ok(stats) => admin.storage = Some(stats),
                    Err(message) => admin.storage_note = Some(fail(message)),
                },
                AdminEvent::Streaming(_)
                | AdminEvent::Telemetry(_)
                | AdminEvent::TelemetryPreview(_)
                | AdminEvent::Updates(_) => {}
                AdminEvent::Cleaned(result) => match result {
                    Ok(done) => {
                        admin.cleanup = Some(done);
                        admin.storage_asked = false;
                    }
                    Err(message) => admin.storage_note = Some(fail(message)),
                },
                AdminEvent::Done { action, result } => match (action, result) {
                    (AdminDone::Role | AdminDone::Delete, Ok(text)) => {
                        admin.users_note = Some(ok(text));
                        admin.users_asked = false;
                        admin.confirm_delete = None;
                    }
                    (AdminDone::Role | AdminDone::Delete, Err(message)) => {
                        admin.users_note = Some(fail(message));
                    }
                    (AdminDone::Reset, Ok(password)) => {
                        let who = admin.reset_for.take().unwrap_or_default();
                        admin.temp_password = Some((who, password));
                    }
                    (AdminDone::Reset, Err(message)) => admin.users_note = Some(fail(message)),
                    (AdminDone::Password, Ok(_)) => {
                        account.password_note =
                            Some(ok("Пароль изменён. Войдите в аккаунт заново."));
                        account.current.clear();
                        account.new.clear();
                        account.repeat.clear();
                        account.logout = true;
                    }
                    (AdminDone::Password, Err(message)) => {
                        account.password_note = Some(fail(message));
                    }
                    (AdminDone::Account, Ok(_)) => account.logout = true,
                    (AdminDone::Account, Err(message)) => account.delete_note = Some(fail(message)),
                    (AdminDone::Image, Ok(_)) => account.note = Some(ok("Картинка обновлена")),
                    (AdminDone::Image, Err(message)) => account.note = Some(fail(message)),
                },
            }
        }
        if let Some(ReceiverBox(rx)) = &self.account.picking
            && let Ok((kind, name, bytes)) = rx.try_recv()
        {
            self.account.picking = None;
            match bytes {
                Ok(bytes) => {
                    bridge.request(BridgeRequest::SetImage {
                        kind,
                        name,
                        bytes: Arc::new(bytes),
                    });
                    self.account.note = Some(ok("Загружаю…"));
                }
                Err(message) => self.account.note = Some(fail(message)),
            }
        }
    }
}

pub(super) fn ok(text: impl Into<String>) -> Note {
    Note {
        text: text.into(),
        ok: true,
    }
}

pub(super) fn fail(text: impl Into<String>) -> Note {
    Note {
        text: text.into(),
        ok: false,
    }
}

impl AccountUi {
    /// Открывает диалог выбора картинки профиля (в отдельном потоке).
    fn pick(&mut self, ctx: &egui::Context, kind: ImageKind) {
        if self.picking.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let chosen = rfd::FileDialog::new()
                .add_filter("Изображения", &["png", "jpg", "jpeg", "gif", "webp"])
                .pick_file();
            if let Some(path) = chosen {
                let name = path
                    .file_name()
                    .map_or_else(|| "image".to_owned(), |n| n.to_string_lossy().into_owned());
                let bytes = match std::fs::metadata(&path) {
                    Ok(meta) if meta.len() > MAX_IMAGE_BYTES => {
                        Err("Картинка больше 16 МБ".to_owned())
                    }
                    Ok(_) => std::fs::read(&path).map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send((kind, name, bytes));
            }
            ctx.request_repaint();
        });
        self.picking = Some(ReceiverBox(rx));
    }
}

/// Аватар и баннер: загрузка и удаление.
pub(crate) fn images_card(ui: &mut egui::Ui, extra: &mut ExtraUi, me: &User, bridge: &Bridge) {
    let managed = me.home_instance.is_some() && me.detached_home_instance.is_none();
    widgets::section_label(ui, "Аватар и баннер");
    ui.add_space(6.0);
    if managed {
        ui.label(
            theme::muted("Картинки профиля ведёт ваш домашний инстанс: меняйте их там.").size(13.0),
        );
        return;
    }
    let ctx = ui.ctx().clone();
    for (kind, title, has) in [
        (ImageKind::Avatar, "Аватар", me.avatar.is_some()),
        (ImageKind::Banner, "Баннер", me.banner.is_some()),
    ] {
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).color(theme::TEXT_BRIGHT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(has, egui::Button::new("Убрать")).clicked() {
                    bridge.request(BridgeRequest::SetImage {
                        kind,
                        name: String::new(),
                        bytes: Arc::new(Vec::new()),
                    });
                }
                let busy = extra.account.picking.is_some();
                if ui
                    .add_enabled(!busy, theme::primary_button("Загрузить…"))
                    .clicked()
                {
                    extra.account.pick(&ctx, kind);
                }
            });
        });
        ui.add_space(4.0);
    }
    show_note(ui, extra.account.note.as_ref());
}

/// Смена пароля и удаление аккаунта.
pub(crate) fn security_cards(ui: &mut egui::Ui, extra: &mut ExtraUi, me: &User, bridge: &Bridge) {
    let state = &mut extra.account;
    let federated = me.home_instance.is_some() && me.detached_home_instance.is_none();
    page_title_small(ui, "Пароль");
    card(ui, |ui| {
        if federated {
            ui.label(theme::muted("Пароль этого аккаунта задаёт домашний инстанс.").size(13.0));
            return;
        }
        widgets::field_label(ui, "Текущий пароль");
        widgets::secret_field(ui, &mut state.current, "Текущий пароль", 128);
        widgets::field_label(ui, "Новый пароль");
        widgets::secret_field(ui, &mut state.new, "Не короче 8 символов", 128);
        widgets::field_label(ui, "Повторите новый пароль");
        widgets::secret_field(ui, &mut state.repeat, "Ещё раз", 128);
        ui.add_space(8.0);
        let problem = if state.new.chars().count() < PASSWORD_MIN {
            Some("Новый пароль короче 8 символов")
        } else if state.new != state.repeat {
            Some("Пароли не совпадают")
        } else if state.current.is_empty() {
            Some("Введите текущий пароль")
        } else {
            None
        };
        if let Some(problem) = problem
            && !(state.new.is_empty() && state.repeat.is_empty())
        {
            ui.label(RichText::new(problem).color(theme::AMBER).size(12.5));
        }
        show_note(ui, state.password_note.as_ref());
        if ui
            .add_enabled(problem.is_none(), theme::primary_button("Сменить пароль"))
            .clicked()
        {
            bridge.request(BridgeRequest::Admin(AdminRequest::ChangePassword {
                current: Some(Secret::new(state.current.clone())),
                new: Secret::new(state.new.clone()),
            }));
        }
    });
    ui.add_space(14.0);
    page_title_small(ui, "Опасная зона");
    card(ui, |ui| {
        if !state.delete_open {
            ui.label(theme::muted("Удаление аккаунта нельзя отменить.").size(13.0));
            ui.add_space(6.0);
            if ui
                .add(egui::Button::new(
                    RichText::new("Удалить аккаунт").color(theme::RED),
                ))
                .clicked()
            {
                state.delete_open = true;
            }
            return;
        }
        ui.label(
            RichText::new("Чтобы удалить аккаунт, введите свой логин и пароль.").color(theme::ROSE),
        );
        widgets::field_label(ui, &format!("Логин ({})", me.username));
        widgets::text_field(ui, &mut state.delete_username, &me.username, 64);
        widgets::field_label(ui, "Пароль");
        widgets::secret_field(ui, &mut state.delete_password, "Пароль", 128);
        ui.add_space(8.0);
        show_note(ui, state.delete_note.as_ref());
        ui.horizontal(|ui| {
            let ready = state.delete_username == me.username && !state.delete_password.is_empty();
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(RichText::new("Удалить навсегда").color(theme::RED)),
                )
                .clicked()
            {
                bridge.request(BridgeRequest::Admin(AdminRequest::DeleteAccount {
                    password: Secret::new(state.delete_password.clone()),
                    username: state.delete_username.clone(),
                }));
            }
            if ui.button("Отмена").clicked() {
                state.delete_open = false;
                state.delete_password.clear();
                state.delete_username.clear();
            }
        });
    });
}

fn page_title_small(ui: &mut egui::Ui, text: &str) {
    widgets::section_label(ui, text);
    ui.add_space(6.0);
}

pub(super) fn setting_row(ui: &mut egui::Ui, title: &str, hint: &str, value: &mut bool) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(ui.available_width() - 60.0);
            ui.label(RichText::new(title).color(theme::TEXT_BRIGHT));
            ui.label(theme::muted(hint).size(12.5));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            widgets::toggle(ui, value);
        });
    });
    ui.add_space(10.0);
}

pub(super) fn number_row(
    ui: &mut egui::Ui,
    title: &str,
    hint: &str,
    value: &mut u32,
    range: (u32, u32),
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(ui.available_width() - 120.0);
            ui.label(RichText::new(title).color(theme::TEXT_BRIGHT));
            ui.label(theme::muted(hint).size(12.5));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(egui::DragValue::new(value).range(range.0..=range.1));
        });
    });
    ui.add_space(10.0);
}

/// Что изменилось относительно сохранённых настроек.
fn settings_patch(
    old: &InstanceSettings,
    new: &InstanceSettings,
    gif_key: &str,
) -> InstanceSettingsPatch {
    let mut patch = InstanceSettingsPatch::default();
    if new.instance_name.trim() != old.instance_name {
        patch.instance_name = Some(new.instance_name.trim().to_owned());
    }
    macro_rules! diff {
        ($field:ident) => {
            if new.$field != old.$field {
                patch.$field = Some(new.$field);
            }
        };
    }
    diff!(registration_open);
    diff!(federated_registration_open);
    diff!(discovery_enabled);
    diff!(max_upload_size_mb);
    diff!(federation_relay_enabled);
    diff!(federation_relay_ttl_days);
    diff!(default_auto_rotate_interval_days);
    diff!(auto_accept_peering);
    if !gif_key.trim().is_empty() {
        patch.gif_api_key = Some(gif_key.trim().to_owned());
    }
    patch
}

/// «Инстанс»: общие настройки, регистрация, загрузки, GIF и федерация.
#[allow(clippy::too_many_lines, reason = "форма настроек одним блоком")]
pub(crate) fn instance_panel(ui: &mut egui::Ui, extra: &mut ExtraUi, bridge: &Bridge) {
    let admin = &mut extra.admin;
    page_title(ui, "Настройки инстанса");
    if !admin.settings_asked {
        admin.settings_asked = bridge.request(BridgeRequest::Admin(AdminRequest::LoadSettings));
    }
    let (Some(saved), Some(edit)) = (admin.settings.clone(), admin.edit.as_mut()) else {
        show_note(ui, admin.settings_note.as_ref());
        ui.label(theme::muted("Загрузка…"));
        return;
    };
    widgets::section_label(ui, "Общие");
    ui.add_space(6.0);
    card(ui, |ui| {
        widgets::field_label(ui, "Название инстанса");
        widgets::text_field(ui, &mut edit.instance_name, "Backspace", 32);
        ui.add_space(10.0);
        setting_row(
            ui,
            "Каталог пространств",
            "Публичные пространства видны в «Обзоре».",
            &mut edit.discovery_enabled,
        );
        number_row(
            ui,
            "Лимит загрузки, МБ",
            "Самый большой файл, который можно прикрепить.",
            &mut edit.max_upload_size_mb,
            (1, 10_240),
        );
        widgets::field_label(ui, "Ключ Klipy для поиска GIF");
        widgets::secret_field(
            ui,
            &mut admin.gif_key,
            saved.gif_api_key.as_deref().unwrap_or("Не задан"),
            128,
        );
    });
    ui.add_space(14.0);
    widgets::section_label(ui, "Регистрация");
    ui.add_space(6.0);
    card(ui, |ui| {
        setting_row(
            ui,
            "Регистрация открыта",
            "Новые люди могут создать аккаунт на этом инстансе.",
            &mut edit.registration_open,
        );
        setting_row(
            ui,
            "Федеративная регистрация",
            "Аккаунты с других инстансов могут заводить здесь профиль.",
            &mut edit.federated_registration_open,
        );
    });
    ui.add_space(14.0);
    widgets::section_label(ui, "Федерация");
    ui.add_space(6.0);
    card(ui, |ui| {
        setting_row(
            ui,
            "Пересылка сообщений",
            "Личные сообщения доставляются другим инстансам.",
            &mut edit.federation_relay_enabled,
        );
        number_row(
            ui,
            "Хранить очередь, дней",
            "Сколько недоставленное ждёт получателя.",
            &mut edit.federation_relay_ttl_days,
            (1, 365),
        );
        number_row(
            ui,
            "Смена ключей, дней",
            "Как часто ключи пиринга обновляются сами.",
            &mut edit.default_auto_rotate_interval_days,
            (1, 365),
        );
        setting_row(
            ui,
            "Принимать пиринг автоматически",
            "Без ручного подтверждения заявок от других инстансов.",
            &mut edit.auto_accept_peering,
        );
    });
    let patch = settings_patch(&saved, edit, &admin.gif_key);
    let changed = patch != InstanceSettingsPatch::default();
    ui.add_space(14.0);
    show_note(ui, admin.settings_note.as_ref());
    ui.horizontal(|ui| {
        if ui
            .add_enabled(changed, theme::primary_button("Сохранить"))
            .clicked()
        {
            bridge.request(BridgeRequest::Admin(AdminRequest::SaveSettings(patch)));
        }
        if ui
            .add_enabled(changed, egui::Button::new("Сбросить"))
            .clicked()
        {
            admin.edit.clone_from(&admin.settings);
            admin.gif_key.clear();
        }
    });
}

/// «Пользователи»: поиск, роли, сброс пароля, удаление.
#[allow(clippy::too_many_lines, reason = "список с действиями в одном месте")]
pub(crate) fn users_panel(ui: &mut egui::Ui, extra: &mut ExtraUi, me: &User, bridge: &Bridge) {
    let admin = &mut extra.admin;
    page_title(ui, "Пользователи");
    ui.horizontal(|ui| {
        widgets::search_box(ui, &mut admin.query, "Найти по имени");
    });
    if admin.query != admin.asked_query {
        admin.asked_query.clone_from(&admin.query);
        admin.page = 1;
        admin.users_asked = false;
    }
    ui.horizontal(|ui| {
        if widgets::toggle(ui, &mut admin.show_deleted) {
            admin.users_asked = false;
        }
        ui.label(theme::muted("Показывать удалённых"));
    });
    if admin.page == 0 {
        admin.page = 1;
    }
    if !admin.users_asked {
        admin.users_asked = bridge.request(BridgeRequest::Admin(AdminRequest::LoadUsers {
            query: admin.query.clone(),
            page: admin.page,
            show_deleted: admin.show_deleted,
        }));
    }
    ui.add_space(8.0);
    show_note(ui, admin.users_note.as_ref());
    if let Some((who, password)) = admin.temp_password.clone() {
        card(ui, |ui| {
            ui.label(format!("Временный пароль для @{who} (покажется один раз):"));
            ui.label(
                RichText::new(&password)
                    .monospace()
                    .size(16.0)
                    .color(theme::MINT),
            );
            if ui.button("Скопировать и скрыть").clicked() {
                ui.ctx().copy_text(password);
                admin.temp_password = None;
            }
        });
        ui.add_space(8.0);
    }
    let Some(page) = admin.users.clone() else {
        ui.label(theme::muted("Загрузка…"));
        return;
    };
    ui.label(theme::muted(format!("Всего: {}", page.total)).size(12.5));
    ui.add_space(4.0);
    for user in &page.users {
        let is_me = user.id == me.id;
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let name = user.display_name.as_deref().unwrap_or(&user.username);
                    ui.label(RichText::new(name).family(theme::bold_family()).color(
                        if user.is_deleted {
                            theme::TEXT_FAINT
                        } else {
                            theme::TEXT_BRIGHT
                        },
                    ));
                    let mut meta = format!("@{}", user.username);
                    if let Some(host) = &user.home_instance {
                        meta.push_str(" · ");
                        meta.push_str(host);
                    }
                    if user.is_admin {
                        meta.push_str(" · администратор");
                    }
                    if user.is_deleted {
                        meta.push_str(" · удалён");
                    }
                    ui.label(theme::muted(meta).size(12.5));
                });
            });
            if user.is_deleted || is_me {
                return;
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let label = if user.is_admin {
                    "Снять админа"
                } else {
                    "Сделать админом"
                };
                if ui.button(label).clicked() {
                    bridge.request(BridgeRequest::Admin(AdminRequest::SetRole {
                        id: user.id.clone(),
                        admin: !user.is_admin,
                    }));
                }
                if user.home_instance.is_none() && ui.button("Сбросить пароль").clicked()
                {
                    admin.reset_for = Some(user.username.clone());
                    bridge.request(BridgeRequest::Admin(AdminRequest::ResetPassword(
                        user.id.clone(),
                    )));
                }
                if admin.confirm_delete.as_deref() == Some(user.id.as_str()) {
                    if ui
                        .add(egui::Button::new(
                            RichText::new("Точно удалить?").color(theme::RED),
                        ))
                        .clicked()
                    {
                        bridge.request(BridgeRequest::Admin(AdminRequest::DeleteUser(
                            user.id.clone(),
                        )));
                    }
                    if ui.button("Отмена").clicked() {
                        admin.confirm_delete = None;
                    }
                } else if ui
                    .add(egui::Button::new(
                        RichText::new("Удалить").color(theme::ROSE),
                    ))
                    .clicked()
                {
                    admin.confirm_delete = Some(user.id.clone());
                }
            });
        });
        ui.add_space(6.0);
    }
    let pages = page.total.div_ceil(u64::from(page.page_size.max(1)));
    if pages > 1 {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(admin.page > 1, egui::Button::new("Назад"))
                .clicked()
            {
                admin.page -= 1;
                admin.users_asked = false;
            }
            ui.label(theme::muted(format!("{} из {pages}", admin.page)));
            if ui
                .add_enabled(u64::from(admin.page) < pages, egui::Button::new("Вперёд"))
                .clicked()
            {
                admin.page += 1;
                admin.users_asked = false;
            }
        });
    }
}

/// «Хранилище»: сводка и очистка осиротевших файлов.
#[allow(clippy::too_many_lines, reason = "сводка и очистка одним блоком")]
pub(crate) fn storage_panel(ui: &mut egui::Ui, extra: &mut ExtraUi, bridge: &Bridge) {
    let admin = &mut extra.admin;
    page_title(ui, "Хранилище");
    if !admin.storage_asked {
        admin.storage_asked = bridge.request(BridgeRequest::Admin(AdminRequest::LoadStorage));
    }
    show_note(ui, admin.storage_note.as_ref());
    let Some(stats) = admin.storage.clone() else {
        ui.label(theme::muted("Загрузка…"));
        return;
    };
    let size = |bytes: u64| human_size(i64::try_from(bytes).unwrap_or(i64::MAX));
    card(ui, |ui| {
        for (label, files, bytes) in [
            ("Всего", stats.total_files, stats.total_size),
            (
                "Используется",
                stats.referenced_files,
                stats.referenced_size,
            ),
            ("Осиротевшие", stats.orphaned_files, stats.orphaned_size),
            (
                "Недогруженные загрузки",
                stats.stale_tus_sessions,
                stats.stale_tus_size,
            ),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).color(theme::TEXT_BRIGHT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(theme::muted(format!("{files} файлов · {}", size(bytes))));
                });
            });
        }
    });
    if !stats.breakdown.is_empty() {
        ui.add_space(10.0);
        widgets::section_label(ui, "По типам");
        ui.add_space(6.0);
        card(ui, |ui| {
            for row in &stats.breakdown {
                let name = match row.kind.as_str() {
                    "image" => "Изображения",
                    "video" => "Видео",
                    "audio" => "Аудио",
                    "document" => "Документы",
                    _ => "Прочее",
                };
                ui.horizontal(|ui| {
                    ui.label(name);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(theme::muted(format!("{} · {}", row.count, size(row.size))));
                    });
                });
            }
        });
    }
    ui.add_space(14.0);
    if let Some(done) = &admin.cleanup {
        let text = if done.dry_run {
            format!(
                "Можно удалить {} файлов ({})",
                done.deleted_files,
                size(done.freed_bytes)
            )
        } else {
            format!(
                "Удалено {} файлов, освобождено {}",
                done.deleted_files,
                size(done.freed_bytes)
            )
        };
        ui.label(RichText::new(text).color(theme::GREEN));
        for error in done.errors.iter().take(3) {
            ui.label(RichText::new(error).color(theme::ROSE).size(12.0));
        }
        ui.add_space(6.0);
    }
    ui.horizontal(|ui| {
        if ui.button("Проверить без удаления").clicked() {
            bridge.request(BridgeRequest::Admin(AdminRequest::Cleanup {
                dry_run: true,
            }));
        }
        if admin.confirm_cleanup {
            if ui
                .add(egui::Button::new(
                    RichText::new("Удалить сирот").color(theme::RED),
                ))
                .clicked()
            {
                bridge.request(BridgeRequest::Admin(AdminRequest::Cleanup {
                    dry_run: false,
                }));
                admin.confirm_cleanup = false;
            }
            if ui.button("Отмена").clicked() {
                admin.confirm_cleanup = false;
            }
        } else if ui
            .add_enabled(
                stats.orphaned_files > 0,
                egui::Button::new("Очистить сирот…"),
            )
            .clicked()
        {
            admin.confirm_cleanup = true;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::settings_patch;
    use bs_api::InstanceSettings;

    #[test]
    fn patch_contains_only_changes() {
        let old = InstanceSettings {
            instance_name: "A".into(),
            registration_open: true,
            max_upload_size_mb: 100,
            ..InstanceSettings::default()
        };
        let mut new = old.clone();
        assert_eq!(
            settings_patch(&old, &new, ""),
            bs_api::InstanceSettingsPatch::default()
        );
        new.registration_open = false;
        new.instance_name = " B ".into();
        let patch = settings_patch(&old, &new, "  key ");
        assert_eq!(patch.registration_open, Some(false));
        assert_eq!(patch.instance_name.as_deref(), Some("B"));
        assert_eq!(patch.gif_api_key.as_deref(), Some("key"));
        assert!(patch.max_upload_size_mb.is_none());
    }
}
