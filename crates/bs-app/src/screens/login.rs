//! Экран входа в инстанс Backspace.

use bs_types::TransportPolicy;
use zeroize::Zeroize as _;

use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;

/// Форма логина и её состояние.
///
/// `Debug` написан руками: пароль не должен попасть в логи.
#[derive(Default)]
pub(crate) struct LoginScreen {
    pub(crate) base: String,
    pub(crate) username: String,
    pub(crate) password: String,
    /// Код двухфакторной защиты (если она включена у аккаунта).
    second_factor: String,
    /// Пользователь явно разрешил соединение по открытому `http://`.
    pub(crate) allow_insecure: bool,
    /// Показана форма регистрации вместо входа.
    registering: bool,
    /// Состояние формы регистрации.
    register: super::register::RegisterForm,
    /// Поставить курсор в поле адреса (после «Сменить инстанс»).
    focus_base: bool,
}

impl std::fmt::Debug for LoginScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginScreen")
            .field("base", &self.base)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("allow_insecure", &self.allow_insecure)
            .field("registering", &self.registering)
            .finish_non_exhaustive()
    }
}

impl Drop for LoginScreen {
    fn drop(&mut self) {
        self.password.zeroize();
        self.second_factor.zeroize();
    }
}

impl LoginScreen {
    /// Чистая форма для входа в другой инстанс: адрес, логин и пароль
    /// пусты, курсор в поле адреса.
    pub(crate) fn switch_instance(&mut self) {
        self.base.clear();
        self.username.clear();
        self.password.zeroize();
        self.second_factor.zeroize();
        self.allow_insecure = false;
        self.registering = false;
        self.focus_base = true;
    }

    /// Рисует экран и по нажатию кнопки (или Enter) запускает логин.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::bg_rail()))
            .show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                    let top = if self.registering { 0.06 } else { 0.18 };
                    ui.add_space((ui.available_height() * top).max(24.0));
                    egui::Frame::new()
                        .fill(theme::bg_chat())
                        .corner_radius(crate::theme::rounded(10))
                        .inner_margin(egui::Margin::same(28))
                        .show(ui, |ui| {
                            ui.set_width(380.0);
                            egui::ScrollArea::vertical()
                                .max_height(ui.ctx().content_rect().height() * 0.9)
                                .show(ui, |ui| {
                                    ui.with_layout(
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            if self.registering {
                                                let back = self.register.card(
                                                    ui,
                                                    bridge,
                                                    &mut self.base,
                                                    &mut self.allow_insecure,
                                                );
                                                if back {
                                                    self.registering = false;
                                                }
                                            } else {
                                                self.card(ui, bridge);
                                            }
                                        },
                                    );
                                });
                        });
                });
            });
    }

    /// Поле кода двухфакторной защиты.
    fn factor_field(&mut self, ui: &mut egui::Ui, width: f32) -> egui::Response {
        ui.add_space(8.0);
        ui.label(theme::section_title(tr("Код двухфакторной защиты")));
        ui.add(
            egui::TextEdit::singleline(&mut self.second_factor)
                .hint_text(tr("Если защита включена: код из приложения или запасной"))
                .char_limit(32)
                .desired_width(width)
                .margin(egui::vec2(10.0, 8.0)),
        )
    }

    /// Содержимое карточки входа.
    /// Поле адреса инстанса и недавние инстансы под ним.
    fn address_field(&mut self, ui: &mut egui::Ui, width: f32) -> egui::Response {
        ui.label(theme::section_title(tr("Адрес инстанса")));
        let base = ui.add(
            egui::TextEdit::singleline(&mut self.base)
                .hint_text("example.com")
                .desired_width(width)
                .margin(egui::vec2(10.0, 8.0)),
        );
        if std::mem::take(&mut self.focus_base) {
            base.request_focus();
        }
        self.recent(ui);
        base
    }

    /// Недавние инстансы: щелчок подставляет адрес, ✕ забывает его.
    fn recent(&mut self, ui: &mut egui::Ui) {
        let recent = crate::prefs::get().recent_instances;
        if recent.is_empty() {
            return;
        }
        ui.add_space(6.0);
        let mut forget = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            ui.label(theme::muted(tr("Недавние:")).size(12.5));
            for item in &recent {
                let shown = item
                    .base
                    .trim_start_matches("https://")
                    .trim_start_matches("http://");
                let chosen = self.base.trim().trim_end_matches('/') == item.base;
                if ui
                    .selectable_label(chosen, egui::RichText::new(shown).size(12.5))
                    .on_hover_text(&item.base)
                    .clicked()
                {
                    self.base.clone_from(&item.base);
                    self.allow_insecure = item.insecure;
                }
                if ui
                    .small_button("✕")
                    .on_hover_text(tr("Забыть этот инстанс"))
                    .clicked()
                {
                    forget = Some(item.base.clone());
                }
            }
        });
        if let Some(base) = forget {
            crate::prefs::update(|prefs| prefs.recent_instances.retain(|item| item.base != base));
        }
    }

    fn card(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge) {
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new(tr("С возвращением!"))
                    .size(24.0)
                    .family(theme::bold_family())
                    .color(theme::text_bright()),
            );
            ui.label(theme::muted(tr("Войдите в свой инстанс")));
        });
        ui.add_space(18.0);

        let busy = bridge.is_busy();
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let mut submit = false;

        ui.add_enabled_ui(!busy, |ui| {
            let width = ui.available_width();
            let base = self.address_field(ui, width);
            ui.add_space(8.0);
            ui.label(theme::section_title(tr("Логин")));
            let username = ui.add(
                egui::TextEdit::singleline(&mut self.username)
                    .desired_width(width)
                    .margin(egui::vec2(10.0, 8.0)),
            );
            ui.add_space(8.0);
            ui.label(theme::section_title(tr("Пароль")));
            let password = ui.add(
                egui::TextEdit::singleline(&mut self.password)
                    .password(true)
                    .desired_width(width)
                    .margin(egui::vec2(10.0, 8.0)),
            );
            let factor = self.factor_field(ui, width);
            // Enter в любом поле отправляет форму.
            if [base, username, password, factor]
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
                tr("Разрешить незащищённое соединение (http)"),
            ),
        )
        .on_hover_text(tr(
            "Пароль и токен пойдут открытым текстом. Включайте только для инстанса \
             в своей локальной сети. localhost разрешён и без этой галочки.",
        ));
        ui.add_space(14.0);

        let label = if busy {
            tr("Подключение…")
        } else {
            tr("Войти")
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
            bridge.set_second_factor(&self.second_factor);
            bridge.start_login(&self.base, &self.username, &self.password, transport);
            // Пароль и код нужны только на время отправки: не держим их в форме.
            self.password.zeroize();
            self.second_factor.zeroize();
        }

        if let Some(err) = bridge.last_error() {
            ui.add_space(10.0);
            ui.label(egui::RichText::new(err).color(theme::rose()));
        }
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            if ui.link(tr("Нет аккаунта? Зарегистрироваться")).clicked()
            {
                self.registering = true;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_instance_clears_the_form() {
        let mut login = LoginScreen::default();
        login.base = "https://old.example".to_owned();
        login.username = "ann".to_owned();
        login.password = "secret".to_owned();
        login.allow_insecure = true;
        login.switch_instance();
        assert!(login.base.is_empty() && login.username.is_empty() && login.password.is_empty());
        assert!(!login.allow_insecure);
        assert!(login.focus_base, "курсор встанет в поле адреса");
    }
}
