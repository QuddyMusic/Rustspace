//! Корень GUI-приложения: роутинг между экранами и хранение состояния.

use std::time::Duration;

use tokio::runtime::Handle;

use crate::bridge::Bridge;
use crate::logic::{connection_label, next_screen, should_wake};
use crate::screens::chat::ChatScreen;
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
    /// Верхняя строка статуса: где мы и что с соединением.
    fn status_bar(&self, ui: &mut egui::Ui) {
        egui::Panel::top("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Backspace RS").strong());
                ui.separator();
                ui.label(connection_label(self.bridge.connection()));
                if self.bridge.is_busy() {
                    ui.spinner();
                }
            });
        });
    }
}

impl eframe::App for BackspaceApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.bridge.poll();

        let ready = self.bridge.take_ready();
        let lost = self.bridge.take_session_lost();
        if lost {
            self.bridge.reset_session();
            self.social.reset();
        }
        self.screen = next_screen(self.screen, ready, lost);

        // Только что открытый личный канал — сразу переключаемся в чат.
        if let Some(channel) = self.bridge.take_opened_dm() {
            self.chat.selected_channel = Some(channel);
            self.screen = Screen::Chat;
        }

        match self.screen {
            Screen::Login => self.login.show(ui, &mut self.bridge),
            Screen::Chat => {
                self.status_bar(ui);
                let open_social = self.chat.show(ui, &self.bridge);
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
                    self.chat.selected_channel = Some(channel);
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
