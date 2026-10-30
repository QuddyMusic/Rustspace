//! Корень GUI-приложения: роутинг между экранами и хранение состояния.

use std::time::Duration;

use tokio::runtime::Handle;

use crate::bridge::{Bridge, ChannelKind};
use crate::logic::{connection_label, next_screen, should_wake};
use crate::screens::chat::{ChatAction, ChatScreen, SelectedChannel};
use crate::screens::login::LoginScreen;
use crate::theme;
use bs_instances::ConnectionState;

/// Как часто будить UI, пока идёт логин или есть живое соединение.
const WAKE_INTERVAL: Duration = Duration::from_millis(200);

/// Какой экран сейчас показан.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    /// Форма входа.
    Login,
    /// Основной интерфейс.
    Chat,
}

/// Состояние приложения целиком.
pub(crate) struct BackspaceApp {
    pub(crate) screen: Screen,
    pub(crate) bridge: Bridge,
    pub(crate) login: LoginScreen,
    pub(crate) chat: ChatScreen,
}

impl BackspaceApp {
    /// Создаёт приложение; `runtime` — handle фонового tokio-runtime.
    pub(crate) fn new(runtime: Handle) -> Self {
        Self {
            screen: Screen::Login,
            bridge: Bridge::new(runtime),
            login: LoginScreen::default(),
            chat: ChatScreen::default(),
        }
    }
}

impl BackspaceApp {
    /// Тонкая полоса над интерфейсом: показывается только если есть что
    /// сказать — отказ сервера или нестабильное соединение.
    fn notice_strip(&mut self, ui: &mut egui::Ui) {
        let notice = self.bridge.notice().map(str::to_owned);
        let unstable = self.bridge.cache().is_some()
            && *self.bridge.connection() != ConnectionState::Connected;
        if notice.is_none() && !unstable {
            return;
        }
        let mut dismiss = false;
        egui::Panel::top("notice")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(if notice.is_some() {
                        theme::ROSE
                    } else {
                        theme::AMBER
                    })
                    .inner_margin(egui::Margin::symmetric(16, 6)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let text = notice.unwrap_or_else(|| {
                        format!("Соединение: {}", connection_label(self.bridge.connection()))
                    });
                    ui.label(
                        egui::RichText::new(text)
                            .color(egui::Color32::BLACK)
                            .family(theme::bold_family()),
                    );
                    if self.bridge.notice().is_some() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Скрыть").clicked() {
                                dismiss = true;
                            }
                        });
                    }
                });
            });
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
            self.chat.reset();
        }
        self.screen = next_screen(self.screen, ready, lost);

        // Только что открытый личный канал — сразу переключаемся в чат.
        if let Some(channel) = self.bridge.take_opened_dm() {
            self.chat.open_channel(SelectedChannel {
                channel,
                kind: ChannelKind::Dm,
            });
            self.screen = Screen::Chat;
        }

        match self.screen {
            Screen::Login => self.login.show(ui, &mut self.bridge),
            Screen::Chat => {
                self.notice_strip(ui);
                if self.chat.show(ui, &mut self.bridge) == ChatAction::Logout {
                    self.bridge.logout();
                }
            }
        }

        // Фоновые события приходят из tokio; без этого окно спало бы вечно.
        if should_wake(self.bridge.is_busy(), self.bridge.cache().is_some()) {
            ui.ctx().request_repaint_after(WAKE_INTERVAL);
        }
    }
}
