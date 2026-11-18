//! Настройки → «Мосты»: свои аккаунты Matrix и Telegram, чаты которых
//! становятся личными переписками (личные мосты сервера Rustspace).

use std::collections::HashMap;

use bs_api::{MatrixLogin, PersonalAccount, PersonalBridges, RemoteChat};
use egui::RichText;

use super::dialogs::{card, page_title};
use super::settings_extra::{Note, fail, ok, show_note};
use crate::bridge::{AdminRequest, BridgeRequest, PersonalEvent, PersonalOp, Secret, TelegramNext};
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::{theme, widgets};

/// Чаты аккаунта на той стороне.
#[derive(Debug)]
enum Chats {
    Loading,
    Ready(Vec<RemoteChat>),
    Failed(String),
}

#[derive(Debug, Default)]
struct MatrixForm {
    homeserver: String,
    with_password: bool,
    token: String,
    login: String,
    password: String,
}

#[derive(Debug, Default)]
struct TelegramForm {
    phone: String,
    api_id: String,
    api_hash: String,
    /// Код отправлен: идентификатор входа.
    login: Option<String>,
    code: String,
    /// Нужен облачный пароль.
    need_password: bool,
    /// Подсказка к нему.
    hint: Option<String>,
    password: String,
}

#[derive(Debug)]
enum Adding {
    Matrix(MatrixForm),
    Telegram(TelegramForm),
}

/// Состояние раздела.
#[derive(Debug, Default)]
pub(crate) struct PersonalUi {
    asked: bool,
    loaded: Option<Result<PersonalBridges, Option<String>>>,
    busy: bool,
    note: Option<Note>,
    adding: Option<Adding>,
    chats: HashMap<String, Chats>,
    /// Аккаунт, у которого открыт выбор чатов.
    picking: Option<String>,
    filter: String,
    confirm_remove: Option<String>,
    /// Ждём ответа на подключение Matrix: форма закроется при успехе.
    matrix_pending: bool,
}

fn send(bridge: &Bridge, op: PersonalOp) {
    bridge.request(BridgeRequest::Admin(AdminRequest::Personal(op)));
}

impl PersonalUi {
    /// Ответ моста.
    pub(crate) fn apply(&mut self, event: PersonalEvent) {
        match event {
            PersonalEvent::Loaded(result) => {
                self.busy = false;
                if std::mem::take(&mut self.matrix_pending) && result.is_ok() {
                    self.adding = None;
                    self.note = Some(ok(tr("Matrix подключён — выберите чаты")));
                }
                self.loaded = Some(result);
            }
            PersonalEvent::Failed(message) => {
                self.busy = false;
                self.matrix_pending = false;
                self.note = Some(fail(message));
            }
            PersonalEvent::TelegramStarted(result) => {
                self.busy = false;
                match (result, &mut self.adding) {
                    (Ok(login), Some(Adding::Telegram(form))) => {
                        form.login = Some(login);
                        self.note = Some(ok(tr("Код отправлен в Telegram")));
                    }
                    (Err(message), _) => self.note = Some(fail(message)),
                    _ => {}
                }
            }
            PersonalEvent::Telegram(result) => {
                self.busy = false;
                match result {
                    Ok(TelegramNext::Password(hint)) => {
                        if let Some(Adding::Telegram(form)) = &mut self.adding {
                            form.need_password = true;
                            form.hint = hint;
                            form.password.clear();
                        }
                    }
                    Ok(TelegramNext::Done) => {
                        self.adding = None;
                        self.note = Some(ok(tr("Telegram подключён — выберите чаты")));
                        self.asked = false;
                    }
                    Err(message) => self.note = Some(fail(message)),
                }
            }
            PersonalEvent::RemoteChats { account, result } => {
                self.chats.insert(
                    account,
                    match result {
                        Ok(chats) => Chats::Ready(chats),
                        Err(message) => Chats::Failed(message),
                    },
                );
            }
        }
    }

    fn act(&mut self, bridge: &Bridge, op: PersonalOp) {
        self.busy = true;
        self.note = None;
        send(bridge, op);
    }
}

/// Раздел «Мосты».
pub(crate) fn page(ui: &mut egui::Ui, state: &mut PersonalUi, bridge: &Bridge) {
    page_title(ui, tr("Мосты"));
    ui.label(theme::muted(tr(
        "Подключите свои аккаунты Matrix и Telegram: выбранные чаты станут личными переписками \
         здесь. Сообщения собеседников приходят от них, ваши ответы уходят от вашего аккаунта.",
    )));
    ui.add_space(10.0);
    if !state.asked {
        state.asked = true;
        send(bridge, PersonalOp::Load);
    }
    let loaded = match &state.loaded {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::muted(tr("Загрузка…")));
            });
            return;
        }
        Some(Err(None)) => {
            card(ui, |ui| {
                ui.label(theme::muted(tr(
                    "Этот сервер не поддерживает личные мосты: нужен сервер Rustspace свежей \
                     версии, где они не выключены (PERSONAL_BRIDGES).",
                )));
            });
            return;
        }
        Some(Err(Some(message))) => {
            ui.label(RichText::new(message).color(theme::rose()));
            if ui.button(tr("Повторить")).clicked() {
                state.asked = false;
            }
            return;
        }
        Some(Ok(loaded)) => loaded.clone(),
    };
    show_note(ui, state.note.as_ref());
    for account in &loaded.accounts {
        account_card(ui, state, bridge, account);
        ui.add_space(8.0);
    }
    if loaded.accounts.is_empty() && state.adding.is_none() {
        card(ui, |ui| {
            ui.label(theme::muted(tr("Аккаунтов пока нет.")));
        });
        ui.add_space(8.0);
    }
    match &mut state.adding {
        None => {
            ui.horizontal(|ui| {
                if loaded.networks.matrix.available
                    && ui
                        .add(theme::primary_button(tr("Подключить Matrix")))
                        .clicked()
                {
                    state.adding = Some(Adding::Matrix(MatrixForm {
                        homeserver: "matrix.org".to_owned(),
                        ..MatrixForm::default()
                    }));
                    state.note = None;
                }
                if loaded.networks.telegram.available
                    && ui
                        .add(theme::primary_button(tr("Подключить Telegram")))
                        .clicked()
                {
                    state.adding = Some(Adding::Telegram(TelegramForm::default()));
                    state.note = None;
                }
            });
            ui.add_space(6.0);
            ui.label(
                theme::muted(tr(
                    "Discord не поддерживается: вход под личным аккаунтом нарушает его правила.",
                ))
                .size(12.0),
            );
        }
        Some(Adding::Matrix(_)) => matrix_form(ui, state, bridge),
        Some(Adding::Telegram(_)) => {
            telegram_form(ui, state, bridge, loaded.networks.telegram.server_keys);
        }
    }
    ui.add_space(10.0);
    ui.label(
        theme::muted(tr(
            "Ключи хранятся на сервере зашифрованными, но сервер видит подключённые чаты: \
             подключайте аккаунт только на инстансе, которому доверяете.",
        ))
        .size(12.0),
    );
}

fn network_title(kind: &str) -> &'static str {
    match kind {
        "telegram" => "Telegram",
        "matrix" => "Matrix",
        _ => "?",
    }
}

fn account_card(
    ui: &mut egui::Ui,
    state: &mut PersonalUi,
    bridge: &Bridge,
    account: &PersonalAccount,
) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        network_title(&account.kind),
                        account.display
                    ))
                    .family(theme::bold_family())
                    .color(theme::text_bright()),
                );
                let (text, color) = match account.state.as_str() {
                    "active" => (tr("Подключён").to_owned(), theme::green()),
                    "starting" => (tr("Подключается…").to_owned(), theme::amber()),
                    _ => (
                        account
                            .error
                            .clone()
                            .unwrap_or_else(|| tr("Ошибка").to_owned()),
                        theme::rose(),
                    ),
                };
                ui.label(RichText::new(text).color(color).size(12.5));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if state.confirm_remove.as_deref() == Some(account.id.as_str()) {
                    if ui
                        .button(RichText::new(tr("Да, отключить")).color(theme::rose()))
                        .clicked()
                    {
                        state.confirm_remove = None;
                        state.act(bridge, PersonalOp::Remove(account.id.clone()));
                    }
                    if ui.button(tr("Отмена")).clicked() {
                        state.confirm_remove = None;
                    }
                    return;
                }
                if ui
                    .add_enabled(!state.busy, egui::Button::new(tr("Отключить")))
                    .clicked()
                {
                    state.confirm_remove = Some(account.id.clone());
                }
                let open = state.picking.as_deref() == Some(account.id.as_str());
                let label = if open {
                    tr("Скрыть чаты")
                } else {
                    tr("Выбрать чаты")
                };
                if account.state == "active" && ui.button(label).clicked() {
                    if open {
                        state.picking = None;
                    } else {
                        state.picking = Some(account.id.clone());
                        state.filter.clear();
                        state.chats.insert(account.id.clone(), Chats::Loading);
                        send(bridge, PersonalOp::RemoteChats(account.id.clone()));
                    }
                }
            });
        });
        if !account.chats.is_empty() {
            ui.add_space(6.0);
            widgets::section_label(ui, tr("Связанные чаты"));
            for chat in &account.chats {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&chat.title).color(theme::text()));
                    if chat.group {
                        ui.label(theme::muted(tr("группа")).size(11.5));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!state.busy, egui::Button::new(tr("Отвязать")))
                            .on_hover_text(tr(
                                "Переписка останется, но новые сообщения перестанут ходить",
                            ))
                            .clicked()
                        {
                            state.act(
                                bridge,
                                PersonalOp::Unlink {
                                    account: account.id.clone(),
                                    chat: chat.id.clone(),
                                },
                            );
                        }
                    });
                });
            }
        }
        if state.picking.as_deref() == Some(account.id.as_str()) {
            ui.add_space(6.0);
            chat_picker(ui, state, bridge, &account.id);
        }
    });
}

fn chat_picker(ui: &mut egui::Ui, state: &mut PersonalUi, bridge: &Bridge, account: &str) {
    widgets::section_label(ui, tr("Чаты аккаунта"));
    let chats = match state.chats.get(account) {
        None | Some(Chats::Loading) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::muted(tr("Загружаю чаты…")));
            });
            return;
        }
        Some(Chats::Failed(message)) => {
            ui.label(RichText::new(message).color(theme::rose()));
            return;
        }
        Some(Chats::Ready(chats)) => chats.clone(),
    };
    widgets::search_box(ui, &mut state.filter, tr("Найти чат"));
    let needle = state.filter.trim().to_lowercase();
    let mut link = None;
    egui::ScrollArea::vertical()
        .id_salt(("personal_chats", account))
        .max_height(280.0)
        .show(ui, |ui| {
            for chat in chats
                .iter()
                .filter(|c| needle.is_empty() || c.title.to_lowercase().contains(&needle))
            {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&chat.title).color(theme::text()));
                    if chat.group {
                        ui.label(theme::muted(tr("группа")).size(11.5));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if chat.linked {
                            ui.label(RichText::new(tr("Связан")).color(theme::green()).size(12.5));
                        } else if ui
                            .add_enabled(!state.busy, egui::Button::new(tr("Связать")))
                            .clicked()
                        {
                            link = Some(chat.clone());
                        }
                    });
                });
            }
        });
    if let Some(chat) = link {
        if let Some(Chats::Ready(list)) = state.chats.get_mut(account)
            && let Some(item) = list.iter_mut().find(|c| c.id == chat.id)
        {
            item.linked = true;
        }
        state.act(
            bridge,
            PersonalOp::Link {
                account: account.to_owned(),
                chat,
            },
        );
    }
}

fn matrix_form(ui: &mut egui::Ui, state: &mut PersonalUi, bridge: &Bridge) {
    let Some(Adding::Matrix(form)) = &mut state.adding else {
        return;
    };
    let mut submit = None;
    let mut cancel = false;
    card(ui, |ui| {
        widgets::section_label(ui, tr("Подключить Matrix"));
        widgets::field_label(ui, tr("Домашний сервер"));
        widgets::text_field(ui, &mut form.homeserver, "matrix.org", 200);
        ui.horizontal(|ui| {
            ui.radio_value(&mut form.with_password, false, tr("Токен доступа"));
            ui.radio_value(&mut form.with_password, true, tr("Логин и пароль"));
        });
        if form.with_password {
            widgets::field_label(ui, tr("Логин"));
            widgets::text_field(ui, &mut form.login, "@me:matrix.org", 200);
            widgets::field_label(ui, tr("Пароль"));
            widgets::secret_field(ui, &mut form.password, tr("Пароль"), 256);
        } else {
            widgets::field_label(ui, tr("Токен доступа"));
            widgets::secret_field(ui, &mut form.token, "syt_…", 512);
            ui.label(
                theme::muted(tr(
                    "Element: Настройки → Помощь и о программе → Токен доступа.",
                ))
                .size(12.0),
            );
        }
        ui.label(theme::muted(tr("Зашифрованные комнаты не поддерживаются.")).size(12.0));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let ready = !form.homeserver.trim().is_empty()
                && if form.with_password {
                    !form.login.trim().is_empty() && !form.password.is_empty()
                } else {
                    !form.token.trim().is_empty()
                };
            if ui
                .add_enabled(
                    ready && !state.busy,
                    theme::primary_button(tr("Подключить")),
                )
                .clicked()
            {
                let login = if form.with_password {
                    MatrixLogin::Password(form.login.trim().to_owned(), form.password.clone())
                } else {
                    MatrixLogin::Token(form.token.trim().to_owned())
                };
                submit = Some(PersonalOp::AddMatrix {
                    homeserver: form.homeserver.trim().to_owned(),
                    login,
                });
            }
            if ui.button(tr("Отмена")).clicked() {
                cancel = true;
            }
            if state.busy {
                ui.spinner();
            }
        });
    });
    if let Some(op) = submit {
        state.act(bridge, op);
        // Форма закроется, когда придёт обновлённый список без ошибки.
        state.matrix_pending = true;
    }
    if cancel {
        state.adding = None;
    }
}

/// Кнопки шага: основная и «Отмена». Возвращает (основная нажата, отмена).
fn step_buttons(ui: &mut egui::Ui, label: &str, ready: bool, busy: bool) -> (bool, bool) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let go = ui
            .add_enabled(ready && !busy, theme::primary_button(label))
            .clicked();
        let cancel = ui.button(tr("Отмена")).clicked();
        if busy {
            ui.spinner();
        }
        (go, cancel)
    })
    .inner
}

/// Номер (и ключи приложения, если у сервера их нет).
fn telegram_phone(
    ui: &mut egui::Ui,
    form: &mut TelegramForm,
    busy: bool,
    server_keys: bool,
) -> (Option<PersonalOp>, bool) {
    widgets::field_label(ui, tr("Номер телефона"));
    widgets::text_field(ui, &mut form.phone, "+7 900 000-00-00", 32);
    if !server_keys {
        ui.label(
            theme::muted(tr(
                "Ключи приложения Telegram задаёт администратор сервера — один раз для всех \
                 (setup.sh --telegram-app). На этом сервере их пока нет: попросите \
                 администратора или впишите свои с my.telegram.org (API development tools).",
            ))
            .size(12.0),
        );
        widgets::field_label(ui, "API ID");
        widgets::text_field(ui, &mut form.api_id, "1234567", 12);
        widgets::field_label(ui, "API hash");
        widgets::secret_field(ui, &mut form.api_hash, "0123abcd…", 64);
    }
    let app = (!server_keys)
        .then(|| {
            form.api_id
                .trim()
                .parse::<i32>()
                .ok()
                .zip(Some(form.api_hash.trim().to_owned()).filter(|h| h.len() >= 16))
        })
        .flatten();
    let ready = form.phone.chars().filter(char::is_ascii_digit).count() >= 6
        && (server_keys || app.is_some());
    let (go, cancel) = step_buttons(ui, tr("Получить код"), ready, busy);
    let op = go.then(|| PersonalOp::TelegramLogin {
        phone: form.phone.trim().to_owned(),
        app,
    });
    (op, cancel)
}

/// Код или облачный пароль.
fn telegram_secret(
    ui: &mut egui::Ui,
    form: &mut TelegramForm,
    busy: bool,
) -> (Option<PersonalOp>, bool) {
    let login = form.login.clone().unwrap_or_default();
    if form.need_password {
        ui.label(theme::muted(tr(
            "Включена двухэтапная проверка: введите облачный пароль Telegram.",
        )));
        if let Some(hint) = form.hint.as_deref().filter(|h| !h.is_empty()) {
            ui.label(theme::muted(crate::tf!("Подсказка: {hint}", hint = hint)).size(12.0));
        }
        widgets::field_label(ui, tr("Облачный пароль"));
        widgets::secret_field(ui, &mut form.password, tr("Пароль"), 256);
        let (go, cancel) = step_buttons(ui, tr("Войти"), !form.password.is_empty(), busy);
        let op = go.then(|| PersonalOp::TelegramPassword {
            login,
            password: Secret::new(form.password.clone()),
        });
        return (op, cancel);
    }
    ui.label(theme::muted(tr(
        "Telegram прислал код — в приложение Telegram или по SMS.",
    )));
    widgets::field_label(ui, tr("Код"));
    widgets::text_field(ui, &mut form.code, "12345", 8);
    let (go, cancel) = step_buttons(ui, tr("Войти"), form.code.trim().len() >= 4, busy);
    let op = go.then(|| PersonalOp::TelegramCode {
        login,
        code: form.code.trim().to_owned(),
    });
    (op, cancel)
}

fn telegram_form(ui: &mut egui::Ui, state: &mut PersonalUi, bridge: &Bridge, server_keys: bool) {
    let busy = state.busy;
    let Some(Adding::Telegram(form)) = &mut state.adding else {
        return;
    };
    let mut step = (None, false);
    card(ui, |ui| {
        widgets::section_label(ui, tr("Подключить Telegram"));
        step = if form.login.is_none() {
            telegram_phone(ui, form, busy, server_keys)
        } else {
            telegram_secret(ui, form, busy)
        };
    });
    let (submit, cancel) = step;
    if let Some(op) = submit {
        state.act(bridge, op);
    }
    if cancel {
        state.adding = None;
        state.busy = false;
    }
}
