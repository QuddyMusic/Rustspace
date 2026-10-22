//! Экран входа в инстанс Backspace.

use crate::bridge::Bridge;

/// Форма логина и её состояние.
#[derive(Debug, Default)]
pub(crate) struct LoginScreen {
    pub(crate) base: String,
    pub(crate) username: String,
    pub(crate) password: String,
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
                bridge.start_login(&self.base, &self.username, &self.password);
            }

            if let Some(err) = bridge.last_error() {
                ui.add_space(4.0);
                ui.colored_label(egui::Color32::LIGHT_RED, err);
            }
        });
    }
}
