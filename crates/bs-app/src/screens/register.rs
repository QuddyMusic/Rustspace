//! Регистрация нового аккаунта на домашнем инстансе.
//!
//! Повторяет форму оригинального клиента: адрес инстанса, имя (с проверкой
//! занятости на лету), пароль с подтверждением, отображаемое имя, цвет
//! аватара и — если открытая регистрация выключена — приглашение (ссылка
//! или код, тоже проверяется на лету). После создания аккаунта сразу
//! выполняется вход.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use bs_api::{ApiClient, InstanceInfo, InviteCheck, RegisterRequest, UsernameCheck};
use bs_types::{Origin, TransportPolicy};
use zeroize::Zeroize as _;

use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::logic::{invite_token_from_input, username_problem};
use crate::theme;

/// Пауза после ввода, после которой идёт проверка на сервере.
const DEBOUNCE: Duration = Duration::from_millis(500);
/// Минимальная длина пароля (как `PASSWORD_MIN_LENGTH` оригинала).
const PASSWORD_MIN: usize = 8;

/// Отложенная проверка на сервере: ключ, срок и ответ.
#[derive(Debug)]
struct Probe<T> {
    /// Значение, для которого нужна проверка.
    key: String,
    /// Когда её запускать.
    due: Option<Instant>,
    /// Ключ, для которого запрос уже ушёл.
    sent: Option<String>,
    rx: Option<mpsc::Receiver<(String, Result<T, String>)>>,
    /// Последний ответ для текущего ключа.
    result: Option<Result<T, String>>,
}

impl<T> Default for Probe<T> {
    fn default() -> Self {
        Self {
            key: String::new(),
            due: None,
            sent: None,
            rx: None,
            result: None,
        }
    }
}

impl<T: Send + 'static> Probe<T> {
    /// Значение поменялось: сбросить ответ и запланировать проверку.
    fn touch(&mut self, key: &str) {
        if self.key != key {
            key.clone_into(&mut self.key);
            self.result = None;
            self.due = (!key.is_empty()).then(|| Instant::now() + DEBOUNCE);
        }
    }

    /// Проверка ещё в пути.
    fn pending(&self) -> bool {
        self.due.is_some()
            || (self.sent.as_deref() == Some(self.key.as_str()) && self.result.is_none())
    }

    /// Запускает запрос, когда подошёл срок, и принимает ответы.
    fn poll<F, Fut>(&mut self, runtime: &tokio::runtime::Handle, run: F)
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<Output = Result<T, String>> + Send + 'static,
    {
        if let Some(rx) = &self.rx {
            while let Ok((key, result)) = rx.try_recv() {
                if key == self.key {
                    self.result = Some(result);
                }
            }
        }
        if self.due.is_some_and(|due| Instant::now() >= due) {
            self.due = None;
            let key = self.key.clone();
            let (tx, rx) = mpsc::channel();
            self.rx = Some(rx);
            self.sent = Some(key.clone());
            let future = run(key.clone());
            runtime.spawn(async move {
                let _ = tx.send((key, future.await));
            });
        }
    }
}

/// Форма регистрации.
#[derive(Default)]
pub(crate) struct RegisterForm {
    username: String,
    password: String,
    confirm: String,
    display_name: String,
    avatar_color: Option<String>,
    invite: String,
    info: Probe<InstanceInfo>,
    name_check: Probe<UsernameCheck>,
    invite_check: Probe<InviteCheck>,
    /// Ошибка проверки формы до отправки.
    problem: Option<String>,
}

impl std::fmt::Debug for RegisterForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisterForm")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("invite", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl Drop for RegisterForm {
    fn drop(&mut self) {
        self.password.zeroize();
        self.confirm.zeroize();
    }
}

/// Клиент для коротких проверок без входа.
fn probe_client(base: &str, transport: TransportPolicy) -> Result<ApiClient, String> {
    let base = crate::bridge::normalize_base(base).ok_or_else(|| tr("нет адреса").to_owned())?;
    ApiClient::with_policy(Origin::Home, base, transport).map_err(|err| err.to_string())
}

impl RegisterForm {
    /// Рисует содержимое карточки; возвращает `true`, если пользователь
    /// хочет вернуться ко входу.
    pub(crate) fn card(
        &mut self,
        ui: &mut egui::Ui,
        bridge: &mut Bridge,
        base: &mut String,
        allow_insecure: &mut bool,
    ) -> bool {
        let transport = if *allow_insecure {
            TransportPolicy::AllowInsecure
        } else {
            TransportPolicy::Strict
        };
        self.poll(bridge, base, transport);
        if self.info.pending() || self.name_check.pending() || self.invite_check.pending() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }

        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new(tr("Создать аккаунт"))
                    .size(24.0)
                    .family(theme::bold_family())
                    .color(theme::text_bright()),
            );
            ui.label(theme::muted(tr("Новый аккаунт на инстансе")));
        });
        ui.add_space(14.0);

        let busy = bridge.is_busy();
        let mut back = false;
        ui.add_enabled_ui(!busy, |ui| {
            let width = ui.available_width();
            super::login::form_label(ui, tr("Адрес инстанса"));
            ui.add(field(base, width).hint_text("example.com"));
            self.instance_status(ui);
            ui.add_space(6.0);

            super::login::form_label(ui, tr("Имя пользователя"));
            ui.add(field(&mut self.username, width).hint_text(tr("латиница, цифры, _")));
            self.username_status(ui);
            ui.add_space(6.0);

            super::login::form_label(ui, tr("Отображаемое имя (необязательно)"));
            ui.add(field(&mut self.display_name, width));
            ui.add_space(6.0);

            super::login::form_label(ui, tr("Пароль"));
            ui.add(field(&mut self.password, width).password(true));
            ui.label(
                theme::muted(crate::tf!(
                    "Не меньше {PASSWORD_MIN} символов",
                    PASSWORD_MIN = PASSWORD_MIN
                ))
                .size(12.0),
            );
            ui.add_space(4.0);
            super::login::form_label(ui, tr("Повторите пароль"));
            ui.add(field(&mut self.confirm, width).password(true));
            if !self.confirm.is_empty() && self.confirm != self.password {
                ui.label(
                    egui::RichText::new(tr("Пароли не совпадают"))
                        .size(12.0)
                        .color(theme::rose()),
                );
            }
            ui.add_space(6.0);

            if self.needs_invite() || !self.invite.is_empty() {
                super::login::form_label(ui, tr("Приглашение"));
                ui.add(field(&mut self.invite, width).hint_text(tr("ссылка или код")));
                self.invite_status(ui);
                ui.add_space(6.0);
            }

            super::login::form_label(ui, tr("Цвет аватара"));
            self.color_picker(ui);
        });

        if base.trim().to_ascii_lowercase().starts_with("http://") {
            ui.add_space(10.0);
            ui.add_enabled(
                !busy,
                egui::Checkbox::new(
                    allow_insecure,
                    tr("Разрешить незащищённое соединение (http)"),
                ),
            );
        }
        ui.add_space(12.0);

        let label = if busy {
            tr("Создание…")
        } else {
            tr("Создать аккаунт")
        };
        let submit = super::login::submit_button(ui, label, !busy && self.ready()).clicked();
        if submit {
            self.submit(bridge, base, transport);
        }
        if busy {
            ui.add_space(6.0);
            ui.vertical_centered(egui::Ui::spinner);
        }
        self.show_problem(ui, bridge);
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            if ui.link(tr("Уже есть аккаунт? Войти")).clicked() {
                back = true;
            }
        });
        back
    }

    /// Ошибка формы или отказ сервера под кнопкой.
    fn show_problem(&self, ui: &mut egui::Ui, bridge: &Bridge) {
        let text = self.problem.as_deref().or_else(|| bridge.last_error());
        if let Some(text) = text {
            ui.add_space(10.0);
            super::login::error_note(ui, text);
        }
    }

    /// Запускает и принимает фоновые проверки.
    fn poll(&mut self, bridge: &Bridge, base: &str, transport: TransportPolicy) {
        let runtime = bridge.runtime();
        self.info.touch(base.trim());
        self.info.poll(&runtime, |key| async move {
            let client = probe_client(&key, transport)?;
            client.instance_info().await.map_err(|err| err.to_string())
        });

        let name = self.username.trim().to_lowercase();
        let name_key = if username_problem(&name).is_none() && !base.trim().is_empty() {
            format!("{}\n{name}", base.trim())
        } else {
            String::new()
        };
        self.name_check.touch(&name_key);
        self.name_check.poll(&runtime, |key| async move {
            let (base, name) = key.split_once('\n').unwrap_or_default();
            let client = probe_client(base, transport)?;
            client
                .check_username(name)
                .await
                .map_err(|err| err.to_string())
        });

        let invite_key = invite_token_from_input(&self.invite)
            .filter(|_| !base.trim().is_empty())
            .map(|token| format!("{}\n{token}", base.trim()))
            .unwrap_or_default();
        self.invite_check.touch(&invite_key);
        self.invite_check.poll(&runtime, |key| async move {
            let (base, token) = key.split_once('\n').unwrap_or_default();
            let client = probe_client(base, transport)?;
            client
                .check_invite(token)
                .await
                .map_err(|err| err.to_string())
        });
    }

    /// Открытая регистрация выключена: без приглашения не пустят.
    fn needs_invite(&self) -> bool {
        matches!(&self.info.result, Some(Ok(info)) if !info.registration_open)
    }

    fn instance_status(&self, ui: &mut egui::Ui) {
        let (text, color) = match &self.info.result {
            None if self.info.pending() => {
                (tr("Проверяем инстанс…").to_owned(), theme::text_faint())
            }
            None => return,
            Some(Err(err)) => (
                crate::tf!("Инстанс не отвечает: {err}", err = err),
                theme::rose(),
            ),
            Some(Ok(info)) if info.registration_open => (
                crate::tf!("{a0} · регистрация открыта", a0 = info.name),
                theme::mint(),
            ),
            Some(Ok(info)) => (
                crate::tf!("{a0} · регистрация только по приглашению", a0 = info.name),
                theme::amber(),
            ),
        };
        ui.label(egui::RichText::new(text).size(12.0).color(color));
    }

    fn username_status(&self, ui: &mut egui::Ui) {
        let name = self.username.trim();
        if name.is_empty() {
            return;
        }
        let (text, color) = username_problem(name).map_or_else(
            || self.name_verdict(),
            |problem| (problem.to_owned(), theme::amber()),
        );
        ui.label(egui::RichText::new(text).size(12.0).color(color));
    }

    /// Ответ сервера о занятости имени.
    fn name_verdict(&self) -> (String, egui::Color32) {
        match &self.name_check.result {
            None => (tr("Проверяем…").to_owned(), theme::text_faint()),
            Some(Ok(UsernameCheck::Available)) => (tr("Имя свободно").to_owned(), theme::mint()),
            Some(Ok(UsernameCheck::Taken)) => (tr("Имя уже занято").to_owned(), theme::rose()),
            Some(Ok(UsernameCheck::Invalid { code })) => (
                crate::logic::describe_server_error(Some(code), tr("Имя не подходит")),
                theme::rose(),
            ),
            // Закрытая регистрация не мешает имени: по приглашению можно.
            Some(Ok(UsernameCheck::RegistrationClosed)) => (
                tr("Проверка недоступна: нужна ссылка-приглашение").to_owned(),
                theme::text_faint(),
            ),
            Some(Err(_)) => (
                tr("Не удалось проверить имя").to_owned(),
                theme::text_faint(),
            ),
        }
    }

    fn invite_status(&self, ui: &mut egui::Ui) {
        if self.invite.trim().is_empty() {
            return;
        }
        let (text, color) = match &self.invite_check.result {
            None => (tr("Проверяем приглашение…").to_owned(), theme::text_faint()),
            Some(Ok(InviteCheck::Valid { name })) if name.is_empty() => {
                (tr("Приглашение действует").to_owned(), theme::mint())
            }
            Some(Ok(InviteCheck::Valid { name })) => (
                crate::tf!("Приглашение «{name}» действует", name = name),
                theme::mint(),
            ),
            Some(Ok(InviteCheck::Expired)) => {
                (tr("Срок приглашения истёк").to_owned(), theme::rose())
            }
            Some(Ok(InviteCheck::Exhausted)) => {
                (tr("Приглашение уже использовано").to_owned(), theme::rose())
            }
            Some(Ok(InviteCheck::Invalid)) => {
                (tr("Приглашение не найдено").to_owned(), theme::rose())
            }
            Some(Err(_)) => (
                tr("Не удалось проверить приглашение").to_owned(),
                theme::text_faint(),
            ),
        };
        ui.label(egui::RichText::new(text).size(12.0).color(color));
    }

    fn color_picker(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for name in theme::PALETTE_NAMES {
                let Some(color) = theme::palette(name) else {
                    continue;
                };
                let selected = self.avatar_color.as_deref() == Some(name);
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
                ui.painter().circle_filled(rect.center(), 11.0, color);
                if selected {
                    ui.painter().circle_stroke(
                        rect.center(),
                        12.5,
                        egui::Stroke::new(2.0, theme::text_bright()),
                    );
                }
                if response.on_hover_text(name).clicked() {
                    self.avatar_color = (!selected).then(|| name.to_owned());
                }
            }
        });
    }

    /// Всё ли заполнено, чтобы отправить форму.
    fn ready(&self) -> bool {
        username_problem(&self.username).is_none()
            && self.password.chars().count() >= PASSWORD_MIN
            && self.password == self.confirm
            && !matches!(self.name_check.result, Some(Ok(UsernameCheck::Taken)))
            && (!self.needs_invite() || invite_token_from_input(&self.invite).is_some())
    }

    fn submit(&mut self, bridge: &mut Bridge, base: &str, transport: TransportPolicy) {
        self.problem = None;
        if let Some(problem) = username_problem(&self.username) {
            self.problem = Some(problem.to_owned());
            return;
        }
        if self.password != self.confirm {
            self.problem = Some(tr("Пароли не совпадают").to_owned());
            return;
        }
        let display = self.display_name.trim();
        let request = RegisterRequest {
            username: self.username.trim().to_lowercase(),
            password: self.password.clone(),
            display_name: (!display.is_empty()).then(|| display.to_owned()),
            avatar_color: self.avatar_color.clone(),
            invite_token: invite_token_from_input(&self.invite),
            ..RegisterRequest::default()
        };
        bridge.start_register(base, request, transport);
        self.password.zeroize();
        self.confirm.zeroize();
    }
}

fn field(text: &mut String, width: f32) -> egui::TextEdit<'_> {
    super::login::form_field(text, width)
}
