//! Экран входа в инстанс Backspace.

use bs_types::TransportPolicy;
use zeroize::Zeroize as _;

use crate::bridge::Bridge;

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
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Backspace RS");
            ui.label("Вход в инстанс");
            ui.add_space(8.0);

            let busy = bridge.is_busy();
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let mut submit = false;

            ui.add_enabled_ui(!busy, |ui| {
                egui::Grid::new("login_form")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Адрес");
                        let base = ui.add(
                            egui::TextEdit::singleline(&mut self.base).hint_text("example.com"),
                        );
                        ui.end_row();

                        ui.label("Логин");
                        let username = ui.text_edit_singleline(&mut self.username);
                        ui.end_row();

                        ui.label("Пароль");
                        let password =
                            ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
                        ui.end_row();

                        // Enter в любом поле отправляет форму.
                        if [base, username, password]
                            .iter()
                            .any(|r| r.lost_focus() && enter)
                        {
                            submit = true;
                        }
                    });
            });

            ui.add_space(4.0);
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
            ui.add_space(8.0);
            let button = ui.add_enabled(
                !busy,
                egui::Button::new(if busy {
                    "Подключение…"
                } else {
                    "Войти"
                }),
            );
            if button.clicked() {
                submit = true;
            }

            if busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Подключение к инстансу…");
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
                ui.add_space(4.0);
                ui.colored_label(egui::Color32::LIGHT_RED, err);
            }
        });
    }
}
