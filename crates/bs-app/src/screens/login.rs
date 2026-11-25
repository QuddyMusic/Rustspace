//! Экран входа: стеклянная карточка по центру окна.

use bs_types::TransportPolicy;
use zeroize::Zeroize as _;

use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;

/// Форма логина и её состояние.
///
/// `Debug` написан руками: пароль не должен попасть в логи.
#[derive(Default)]
#[allow(clippy::struct_excessive_bools, reason = "независимые флаги формы")]
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
    /// Пользователь открыл поле кода двухфакторной защиты.
    show_factor: bool,
    /// Высота карточки на прошлом кадре — чтобы держать её по центру.
    card_height: f32,
    /// Идёт вход по ключу доступа.
    passkey: Option<super::passkey_login::PasskeyLogin>,
    /// Выбран сохранённый аккаунт (номер в списке) — войти в него.
    pub(crate) picked: Option<usize>,
    /// Вход по сохранённой сессии повторится сам (сервер недоступен).
    pub(crate) retry_note: Option<String>,
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
            .frame(egui::Frame::new())
            .show(ui, |ui| {
                theme::paint_backdrop(ui);
                let available = ui.available_height();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                            // По центру окна; длинная форма прокручивается.
                            let top = ((available - self.card_height) / 2.0).max(24.0);
                            ui.add_space(top);
                            let card = theme::floating(theme::bg_sidebar(), 18)
                                .inner_margin(egui::Margin::symmetric(32, 28))
                                .show(ui, |ui| {
                                    ui.set_width(380.0);
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
                            let height = card.response.rect.height();
                            if (height - self.card_height).abs() > 0.5 {
                                self.card_height = height;
                                ui.ctx().request_repaint();
                            }
                            ui.add_space(24.0);
                        });
                    });
            });
    }

    /// Поле кода двухфакторной защиты.
    fn factor_field(&mut self, ui: &mut egui::Ui, width: f32) -> egui::Response {
        form_label(ui, tr("Код двухфакторной защиты"));
        ui.add(
            form_field(&mut self.second_factor, width)
                .hint_text(tr("Код из приложения или запасной"))
                .char_limit(32),
        )
    }

    /// Сохранённые аккаунты: вход в один щелчок, без пароля.
    fn saved_accounts(&mut self, ui: &mut egui::Ui, bridge: &Bridge, busy: bool) {
        let current = bridge.persist_path();
        let accounts: Vec<_> = crate::accounts::list()
            .into_iter()
            .enumerate()
            .filter(|(_, account)| crate::accounts::path_of(account) != current)
            .collect();
        if accounts.is_empty() {
            return;
        }
        form_label(ui, tr("Сохранённые аккаунты"));
        for (index, account) in accounts {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 44.0),
                if busy {
                    egui::Sense::hover()
                } else {
                    egui::Sense::click()
                },
            );
            if response.hovered() && !busy {
                ui.painter()
                    .rect_filled(rect, theme::rounded(8), theme::bg_hover());
            }
            let face = egui::Rect::from_center_size(
                rect.left_center() + egui::vec2(20.0, 0.0),
                egui::Vec2::splat(30.0),
            );
            theme::paint_avatar(ui.painter(), face, account.face(), None, busy, None);
            let text = rect.left_center() + egui::vec2(44.0, 0.0);
            ui.painter().text(
                text - egui::vec2(0.0, 8.0),
                egui::Align2::LEFT_CENTER,
                &account.name,
                egui::FontId::proportional(14.0),
                theme::text_bright(),
            );
            ui.painter().text(
                text + egui::vec2(0.0, 9.0),
                egui::Align2::LEFT_CENTER,
                account.subtitle(),
                egui::FontId::proportional(11.5),
                theme::text_muted(),
            );
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.picked = Some(index);
            }
        }
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
    }

    /// Содержимое карточки входа.
    /// Поле адреса инстанса и недавние инстансы под ним.
    fn address_field(&mut self, ui: &mut egui::Ui, width: f32) -> egui::Response {
        form_label(ui, tr("Адрес инстанса"));
        let base = ui.add(form_field(&mut self.base, width).hint_text("chat.example.org"));
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
        ui.add_space(8.0);
        let mut forget = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            for item in &recent {
                let shown = item
                    .base
                    .trim_start_matches("https://")
                    .trim_start_matches("http://");
                let chosen = self.base.trim().trim_end_matches('/') == item.base;
                match recent_chip(ui, shown, &item.base, chosen) {
                    Some(ChipClick::Pick) => {
                        self.base.clone_from(&item.base);
                        self.allow_insecure = item.insecure;
                    }
                    Some(ChipClick::Forget) => forget = Some(item.base.clone()),
                    None => {}
                }
            }
        });
        if let Some(base) = forget {
            crate::prefs::update(|prefs| prefs.recent_instances.retain(|item| item.base != base));
        }
    }

    fn card(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge) {
        ui.vertical_centered(|ui| {
            if let Some(icon) = crate::brand::icon_texture(ui.ctx(), 56.0) {
                ui.add(egui::Image::new(&icon).fit_to_exact_size(egui::vec2(56.0, 56.0)));
                ui.add_space(10.0);
            }
            ui.label(
                egui::RichText::new(tr("С возвращением!"))
                    .size(24.0)
                    .family(theme::bold_family())
                    .color(theme::text_bright()),
            );
            ui.label(theme::muted(tr("Войдите в свой инстанс")));
        });
        ui.add_space(14.0);
        let busy = bridge.is_busy();
        self.saved_accounts(ui, bridge, busy);

        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let mut submit = false;
        // Поле кода — по запросу или когда сервер сам попросил код.
        let asked = bridge.last_error().is_some_and(|err| {
            err == crate::logic::describe_server_error(Some("two_factor_required"), "")
                || err == crate::logic::describe_server_error(Some("two_factor_invalid"), "")
        });
        let factor_shown = self.show_factor || asked || !self.second_factor.is_empty();

        ui.add_enabled_ui(!busy, |ui| {
            let width = ui.available_width();
            let base = self.address_field(ui, width);
            form_label(ui, tr("Логин"));
            let username = ui.add(form_field(&mut self.username, width));
            form_label(ui, tr("Пароль"));
            let password = ui.add(form_field(&mut self.password, width).password(true));
            let mut fields = vec![base, username, password];
            if factor_shown {
                let factor = self.factor_field(ui, width);
                if asked && !factor.has_focus() && self.second_factor.is_empty() {
                    factor.request_focus();
                }
                fields.push(factor);
            }
            // Enter в любом поле отправляет форму.
            if fields.iter().any(|r| r.lost_focus() && enter) {
                submit = true;
            }
        });

        self.extras(ui, busy, factor_shown);
        self.footer(ui, bridge, busy, submit);
    }

    /// Ссылка на поле 2FA и галочка про открытый http.
    fn extras(&mut self, ui: &mut egui::Ui, busy: bool, factor_shown: bool) {
        if !factor_shown {
            ui.add_space(6.0);
            if ui
                .add(
                    egui::Label::new(
                        egui::RichText::new(tr("Есть код двухфакторной защиты?"))
                            .size(12.5)
                            .color(theme::text_muted()),
                    )
                    .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.show_factor = true;
            }
        }

        // Открытый http — только для адреса, который начинается с http://.
        if self.base.trim().to_ascii_lowercase().starts_with("http://") {
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
        }
        ui.add_space(18.0);
    }

    /// «Войти по ключу доступа»: страница инстанса в браузере, код для
    /// сверки, вход, когда браузер подтвердит ключ.
    fn passkey_part(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge, busy: bool) {
        ui.add_space(8.0);
        if let Some(flow) = self.passkey.as_mut() {
            if let super::passkey_login::Step::Token(token) = flow.poll(ui.ctx()) {
                let (base, transport) = (flow.base.clone(), flow.transport);
                self.passkey = None;
                bridge.start_token_login(&base, crate::bridge::Secret::new(token), transport);
                return;
            }
            let mut cancel = false;
            ui.vertical_centered(|ui| {
                match (&flow.code, &flow.error) {
                    (_, Some(err)) => error_note(ui, err),
                    (Some(code), None) => {
                        ui.label(theme::muted(tr(
                            "Подтвердите ключ в открывшемся браузере. Код там должен быть таким же:",
                        )));
                        ui.label(
                            egui::RichText::new(code)
                                .monospace()
                                .size(22.0)
                                .color(theme::text_bright()),
                        );
                        ui.spinner();
                    }
                    (None, None) => {
                        ui.spinner();
                    }
                }
                cancel = ui.button(tr("Отмена")).clicked();
            });
            if cancel {
                self.passkey = None;
            }
            return;
        }
        let ready = !busy && !self.base.trim().is_empty();
        let clicked = ui
            .vertical_centered(|ui| {
                ui.add_enabled(ready, egui::Button::new(tr("Войти по ключу доступа")))
                    .on_hover_text(tr(
                        "Passkey: Bitwarden, телефон, Windows Hello — подтверждение в браузере, без пароля.",
                    ))
                    .clicked()
            })
            .inner;
        if clicked && let Some(base) = crate::bridge::normalize_base(&self.base) {
            let transport = if self.allow_insecure {
                TransportPolicy::AllowInsecure
            } else {
                TransportPolicy::Strict
            };
            self.passkey = Some(super::passkey_login::PasskeyLogin::start(
                &bridge.runtime(),
                base,
                transport,
            ));
        }
    }

    /// Кнопка «Войти», отправка формы, ошибка и ссылка на регистрацию.
    fn footer(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge, busy: bool, mut submit: bool) {
        let label = if busy {
            tr("Подключение…")
        } else {
            tr("Войти")
        };
        if submit_button(ui, label, !busy).clicked() {
            submit = true;
        }
        if busy {
            ui.add_space(8.0);
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

        self.passkey_part(ui, bridge, busy);
        if let Some(err) = bridge.last_error() {
            ui.add_space(12.0);
            error_note(ui, err);
        }
        if let Some(note) = &self.retry_note {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::muted(note.as_str()).size(12.5));
            });
        }
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            // По центру: отступ на половину свободного места.
            let font = egui::FontId::proportional(13.5);
            let width = [tr("Нет аккаунта?"), tr("Зарегистрироваться")]
                .iter()
                .map(|text| {
                    ui.painter()
                        .layout_no_wrap((*text).to_owned(), font.clone(), theme::text())
                        .size()
                        .x
                })
                .sum::<f32>()
                + 4.0;
            ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
            ui.label(theme::muted(tr("Нет аккаунта?")).size(13.5));
            if ui
                .link(egui::RichText::new(tr("Зарегистрироваться")).size(13.5))
                .clicked()
            {
                self.registering = true;
            }
        });
    }
}

/// Подпись поля формы входа и регистрации.
pub(super) fn form_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(
        egui::RichText::new(text)
            .size(13.0)
            .color(theme::text_muted()),
    );
    ui.add_space(4.0);
}

/// Поле формы: повыше и с отступами, как на макете.
pub(super) fn form_field(text: &mut String, width: f32) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text)
        .desired_width(width)
        .min_size(egui::vec2(width, 40.0))
        .align(egui::Align2::LEFT_CENTER)
        .margin(egui::vec2(12.0, 10.0))
}

/// Главная кнопка формы во всю ширину, с текстом по центру.
pub(super) fn submit_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let width = ui.available_width();
    ui.vertical_centered(|ui| {
        ui.add_enabled(
            enabled,
            theme::primary_button(label)
                .corner_radius(crate::theme::rounded(12))
                .min_size(egui::vec2(width, 42.0)),
        )
    })
    .inner
}

/// Ошибка входа: плашка с текстом, а не голая красная строка.
pub(super) fn error_note(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(theme::rose().gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.0, theme::rose().gamma_multiply(0.35)))
        .corner_radius(crate::theme::rounded(10))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(text).size(13.0).color(theme::rose()));
        });
}

/// Что нажали на «таблетке» недавнего инстанса.
enum ChipClick {
    Pick,
    Forget,
}

/// «Таблетка» недавнего инстанса: щелчок подставляет адрес, ✕ внутри
/// забывает его.
fn recent_chip(ui: &mut egui::Ui, shown: &str, full: &str, chosen: bool) -> Option<ChipClick> {
    let galley = ui.painter().layout_no_wrap(
        shown.to_owned(),
        egui::FontId::proportional(12.5),
        theme::text(),
    );
    let size = egui::vec2(galley.size().x + 40.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let close = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 14.0, rect.center().y),
        egui::vec2(20.0, 20.0),
    );
    let close_response = ui.interact(close, response.id.with("forget"), egui::Sense::click());
    let painter = ui.painter();
    let background = if chosen {
        theme::accent().gamma_multiply(0.22)
    } else if response.hovered() {
        theme::bg_active()
    } else {
        theme::bg_hover()
    };
    painter.rect_filled(rect, egui::CornerRadius::same(14), background);
    if chosen {
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(14),
            egui::Stroke::new(1.0, theme::accent()),
            egui::StrokeKind::Inside,
        );
    }
    painter.galley(
        egui::pos2(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::text(),
    );
    let cross = if close_response.hovered() {
        theme::text_bright()
    } else {
        theme::text_faint()
    };
    let c = close.center();
    for (from, to) in [((-3.5, -3.5), (3.5, 3.5)), ((-3.5, 3.5), (3.5, -3.5))] {
        painter.line_segment(
            [c + egui::vec2(from.0, from.1), c + egui::vec2(to.0, to.1)],
            egui::Stroke::new(1.5, cross),
        );
    }
    if close_response
        .on_hover_text(tr("Забыть этот инстанс"))
        .clicked()
    {
        return Some(ChipClick::Forget);
    }
    response
        .on_hover_text(full)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
        .then_some(ChipClick::Pick)
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
