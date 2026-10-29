//! Корень GUI-приложения: роутинг между экранами и хранение состояния.

use std::time::Duration;

use tokio::runtime::Handle;

use crate::bridge::{Bridge, ChannelKind};
use crate::logic::{connection_label, next_screen, should_wake};
use crate::screens::chat::{ChatScreen, SelectedChannel};
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

/// Состояние приложения целиком.
pub(crate) struct BackspaceApp {
    pub(crate) screen: Screen,
    pub(crate) bridge: Bridge,
    pub(crate) login: LoginScreen,
    pub(crate) chat: ChatScreen,
    pub(crate) social: SocialScreen,
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
        }
    }
}

impl BackspaceApp {
    /// Верхняя строка статуса: состояние соединения, выход и отказы сервера.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let mut logout = false;
        let mut dismiss = false;
        egui::Panel::top("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Backspace RS").strong());
                ui.separator();
                ui.label(connection_label(self.bridge.connection()));
                if self.bridge.is_busy() {
                    ui.spinner();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let button = ui
                        .add_enabled(!self.bridge.is_busy(), egui::Button::new("Выйти"))
                        .on_hover_text("Выйти из инстанса, чтобы войти в другой");
                    if button.clicked() {
                        logout = true;
                    }
                });
            });
            if let Some(notice) = self.bridge.notice() {
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::LIGHT_RED, notice);
                    if ui.small_button("×").on_hover_text("Скрыть").clicked() {
                        dismiss = true;
                    }
                });
            }
        });
        if logout {
            self.bridge.logout();
        }
        if dismiss {
            self.bridge.dismiss_notice();
        }
    }
}

impl eframe::App for BackspaceApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.bridge.poll();

        let ready = self.bridge.take_ready();
        let lost = self.bridge.take_session_lost();
        if lost {
            // Кэш и каналы мост уже сбросил сам, когда сессия закрылась.
            self.social.reset();
        }
        self.screen = next_screen(self.screen, ready, lost);

        // Только что открытый личный канал — сразу переключаемся в чат.
        if let Some(channel) = self.bridge.take_opened_dm() {
            self.chat.selected_channel = Some(SelectedChannel {
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
