//! Корень GUI-приложения: роутинг между экранами и хранение состояния.

use std::time::Duration;

use tokio::runtime::Handle;

use crate::bridge::{Bridge, ChannelKind};
use crate::logic::{connection_label, next_screen, should_wake};
use crate::screens::chat::{ChatScreen, SelectedChannel, origin_label};
use crate::screens::login::LoginScreen;
use crate::screens::social::SocialScreen;

/// Как часто будить UI, пока идёт логин или есть живое соединение.
const WAKE_INTERVAL: Duration = Duration::from_millis(200);

/// Какой экран сейчас показан.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    /// Форма входа.
    Login,
    /// Основной интерфейс.
    Chat,
    /// Друзья, заявки, поиск, личные каналы.
    Social,
}

/// Форма добавления второго инстанса (открывается из строки статуса).
#[derive(Debug, Default)]
pub(crate) struct AddInstanceForm {
    /// Открыта ли форма.
    pub(crate) open: bool,
    /// Адрес инстанса.
    pub(crate) base: String,
    /// Логин на этом инстансе.
    pub(crate) username: String,
    /// Пароль на этом инстансе.
    pub(crate) password: String,
}

/// Состояние приложения целиком.
pub(crate) struct BackspaceApp {
    pub(crate) screen: Screen,
    pub(crate) bridge: Bridge,
    pub(crate) login: LoginScreen,
    pub(crate) chat: ChatScreen,
    pub(crate) social: SocialScreen,
    pub(crate) add_instance: AddInstanceForm,
}

impl BackspaceApp {
    /// Создаёт приложение; `runtime` — handle фонового tokio-runtime.
    pub(crate) fn new(runtime: Handle) -> Self {
        Self {
            screen: Screen::Login,
            bridge: Bridge::new(runtime),
            login: LoginScreen::default(),
            chat: ChatScreen::default(),
            social: SocialScreen::default(),
            add_instance: AddInstanceForm::default(),
        }
    }
}

impl BackspaceApp {
    /// Верхняя строка статуса: где мы, что с соединением и переключатель инстансов.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        // Список инстансов снимаем до замыкания: внутри него `self` уже занят.
        let origins = self.bridge.instances();
        let mut switch_to = None;
        let mut open_add = false;
        let mut drop_instance = None;
        egui::Panel::top("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Backspace RS").strong());
                ui.separator();
                ui.label(connection_label(self.bridge.connection()));
                if self.bridge.is_busy() {
                    ui.spinner();
                }
                {
                    ui.separator();
                    ui.label("Инстанс:");
                    let active = self.bridge.active().clone();
                    for origin in &origins {
                        let selected = *origin == active;
                        let label = format!(
                            "{} {}",
                            origin_label(origin),
                            connection_label(self.bridge.status_of(origin))
                        );
                        if ui
                            .selectable_label(selected, label)
                            .on_hover_text(origin_label(origin))
                            .clicked()
                        {
                            switch_to = Some(origin.clone());
                        }
                        // Домашний инстанс отключить нельзя: это выход из аккаунта.
                        if !origin.is_home()
                            && ui.small_button("×").on_hover_text("Отключить").clicked()
                        {
                            drop_instance = Some(origin.clone());
                        }
                    }
                    if ui.button("+").on_hover_text("Добавить инстанс").clicked() {
                        open_add = true;
                    }
                }
            });
        });
        if let Some(origin) = switch_to {
            self.bridge.set_active(origin);
        }
        if let Some(origin) = drop_instance {
            self.bridge.remove_instance(&origin);
        }
        if open_add {
            self.add_instance.open = true;
        }
        self.show_add_instance(ui);
    }

    /// Модальная форма подключения второго инстанса.
    fn show_add_instance(&mut self, ui: &egui::Ui) {
        if !self.add_instance.open {
            return;
        }
        let mut open = true;
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new("Добавить инстанс")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                egui::Grid::new("add_instance_form")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Адрес");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.add_instance.base)
                                .hint_text("example.com"),
                        );
                        ui.end_row();
                        ui.label("Логин");
                        ui.text_edit_singleline(&mut self.add_instance.username);
                        ui.end_row();
                        ui.label("Пароль");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.add_instance.password)
                                .password(true),
                        );
                        ui.end_row();
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Подключить").clicked() {
                        submit = true;
                    }
                    if ui.button("Отмена").clicked() {
                        cancel = true;
                    }
                });
            });
        if submit {
            let form = &self.add_instance;
            self.bridge
                .add_instance(&form.base, &form.username, &form.password);
            self.add_instance = AddInstanceForm::default();
        }
        self.add_instance.open = open && !cancel;
    }
}

impl eframe::App for BackspaceApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.bridge.poll();

        // Ctrl+[ / Ctrl+] листают инстансы, не открывая сайдбар.
        ui.input(|input| {
            if input.modifiers.command && input.key_pressed(egui::Key::CloseBracket) {
                self.bridge.next_instance();
            }
            if input.modifiers.command && input.key_pressed(egui::Key::OpenBracket) {
                self.bridge.prev_instance();
            }
        });

        let ready = self.bridge.take_ready();
        let lost = self.bridge.take_session_lost();
        if lost {
            self.bridge.reset_session();
            self.social.reset();
        }
        self.screen = next_screen(self.screen, ready, lost);

        // Только что открытый личный канал — сразу переключаемся в чат.
        if let Some(channel) = self.bridge.take_opened_dm() {
            // Личный канал открывается на активном инстансе: соцэкран
            // показывает друзей именно того origin, что выбран сейчас.
            self.chat.selected_channel = Some(SelectedChannel {
                origin: self.bridge.active().clone(),
                channel,
                kind: ChannelKind::Dm,
            });
            self.screen = Screen::Chat;
        }

        match self.screen {
            Screen::Login => self.login.show(ui, &mut self.bridge),
            Screen::Chat => {
                self.status_bar(ui);
                let open_social = self.chat.show(ui, &mut self.bridge);
                if open_social {
                    self.social.reset();
                    self.screen = Screen::Social;
                }
            }
            Screen::Social => {
                self.status_bar(ui);
                if self.social.show(ui, &self.bridge) {
                    self.screen = Screen::Chat;
                }
                if let Some(channel) = self.social.take_open_channel() {
                    self.chat.selected_channel = Some(SelectedChannel {
                        origin: self.bridge.active().clone(),
                        channel,
                        kind: ChannelKind::Dm,
                    });
                    self.screen = Screen::Chat;
                }
            }
        }

        // Фоновые события приходят из tokio; без этого окно спало бы вечно.
        if should_wake(self.bridge.is_busy(), self.bridge.cache().is_some()) {
            ui.ctx().request_repaint_after(WAKE_INTERVAL);
        }
    }
}
