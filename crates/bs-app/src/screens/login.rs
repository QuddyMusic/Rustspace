//! Экран входа в инстанс Backspace.

use bs_types::TransportPolicy;
use zeroize::Zeroize as _;

use crate::bridge::Bridge;
use crate::theme;

/// Форма логина и её состояние.
///
/// `Debug` написан руками: пароль не должен попасть в логи.
#[derive(Default)]
pub(crate) struct LoginScreen {
    pub(crate) base: String,
    pub(crate) username: String,
    pub(crate) password: String,
    /// Пользователь явно разрешил соединение по открытому `http://`.
    pub(crate) allow_insecure: bool,
}

impl std::fmt::Debug for LoginScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginScreen")
            .field("base", &self.base)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("allow_insecure", &self.allow_insecure)
            .finish()
    }
}

impl Drop for LoginScreen {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

impl LoginScreen {
    /// Рисует экран и по нажатию кнопки (или Enter) запускает логин.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG_RAIL))
            .show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                    ui.add_space((ui.available_height() * 0.18).max(24.0));
                    egui::Frame::new()
                        .fill(theme::BG_CHAT)
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Margin::same(28))
                        .show(ui, |ui| {
                            ui.set_width(380.0);
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                self.card(ui, bridge);
                            });
                        });
                });
            });
    }

    /// Содержимое карточки входа.
    fn card(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge) {
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("С возвращением!")
                    .size(24.0)
                    .strong()
                    .color(theme::TEXT_BRIGHT),
            );
            ui.label(theme::muted("Войдите в свой инстанс Backspace"));
        });
        ui.add_space(18.0);

        let busy = bridge.is_busy();
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let mut submit = false;

        ui.add_enabled_ui(!busy, |ui| {
            let width = ui.available_width();
            ui.label(theme::section_title("Адрес инстанса"));
            let base = ui.add(
                egui::TextEdit::singleline(&mut self.base)
                    .hint_text("example.com")
                    .desired_width(width)
                    .margin(egui::vec2(10.0, 8.0)),
            );
            ui.add_space(8.0);
            ui.label(theme::section_title("Логин"));
            let username = ui.add(
                egui::TextEdit::singleline(&mut self.username)
                    .desired_width(width)
                    .margin(egui::vec2(10.0, 8.0)),
            );
            ui.add_space(8.0);
            ui.label(theme::section_title("Пароль"));
            let password = ui.add(
                egui::TextEdit::singleline(&mut self.password)
                    .password(true)
                    .desired_width(width)
                    .margin(egui::vec2(10.0, 8.0)),
            );
            // Enter в любом поле отправляет форму.
            if [base, username, password]
                .iter()
                .any(|r| r.lost_focus() && enter)
            {
                submit = true;
            }
        });

        ui.add_space(10.0);
        ui.add_enabled(
            !busy,
            egui::Checkbox::new(
                &mut self.allow_insecure,
                "Разрешить незащищённое соединение (http)",
            ),
        )
        .on_hover_text(
            "Пароль и токен пойдут открытым текстом. Включайте только для инстанса \
             в своей локальной сети. localhost разрешён и без этой галочки.",
        );
        ui.add_space(14.0);

        let label = if busy {
            "Подключение…"
        } else {
            "Войти"
        };
        let button = ui.add_enabled(
            !busy,
            theme::primary_button(label).min_size(egui::vec2(ui.available_width(), 38.0)),
        );
        if button.clicked() {
            submit = true;
        }
        if busy {
            ui.add_space(6.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
            });
        }

        if submit && !busy {
            let transport = if self.allow_insecure {
                TransportPolicy::AllowInsecure
            } else {
                TransportPolicy::Strict
            };
            bridge.start_login(&self.base, &self.username, &self.password, transport);
            // Пароль нужен только на время отправки: не держим его в форме.
            self.password.zeroize();
        }

        if let Some(err) = bridge.last_error() {
            ui.add_space(10.0);
            ui.label(egui::RichText::new(err).color(theme::ROSE));
        }
    }
}
