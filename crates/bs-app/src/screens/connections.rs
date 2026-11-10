//! Панель «Подключения»: домашний сервер и подключённые инстансы.
//!
//! Повторяет `ConnectedInstances.tsx` оригинала: фильтры со счётчиками,
//! сортировка, раскрывающиеся карточки, статусы и действия по статусу
//! (отключить, переподключить, войти заново, удалить принудительно,
//! удалить учётную запись).

use std::collections::HashSet;

use bs_api::IdentityDeleteMode;
use chrono::{Datelike, TimeZone};
use egui::{Align, Color32, Layout, Margin, RichText, Sense, Stroke};

use crate::fleet::{Bridge, ConnectionRow, RowState};
use crate::i18n::tr;
use crate::registry::{now_ms, reason};
use crate::theme;
use crate::widgets;

/// Фильтр списка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Filter {
    #[default]
    All,
    Connected,
    Disconnected,
    Issues,
}

/// Порядок списка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Sort {
    #[default]
    DateAdded,
    LastConnected,
}

/// Встроенная форма в раскрытой карточке.
#[derive(Debug)]
enum Dialog {
    /// Вход заново с паролем домашнего аккаунта.
    Reauth { origin: String, password: String },
    /// Вход своим паролем на инстансе.
    OwnLogin {
        origin: String,
        username: String,
        password: String,
    },
    /// Подтверждение принудительного удаления записи.
    ForceRemove { origin: String },
    /// Удаление учётной записи на инстансе.
    Delete {
        origin: String,
        mode: IdentityDeleteMode,
        everywhere: bool,
    },
}

impl Dialog {
    fn origin(&self) -> &str {
        match self {
            Self::Reauth { origin, .. }
            | Self::OwnLogin { origin, .. }
            | Self::ForceRemove { origin }
            | Self::Delete { origin, .. } => origin,
        }
    }
}

/// Состояние панели между кадрами.
#[derive(Debug, Default)]
pub(crate) struct ConnectionsUi {
    filter: Filter,
    sort: Sort,
    expanded: HashSet<String>,
    add_open: bool,
    address: String,
    password: String,
    dialog: Option<Dialog>,
}

/// Что попросил пользователь.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnAction {
    /// Ничего.
    None,
    /// Подключить новый инстанс.
    Connect {
        /// Адрес.
        address: String,
        /// Пароль домашнего аккаунта (проверит дом).
        password: String,
    },
    /// Переподключить без пароля.
    Reconnect(String),
    /// Войти заново с паролем домашнего аккаунта.
    Reauthenticate {
        /// Инстанс.
        origin: String,
        /// Пароль домашнего аккаунта.
        password: String,
    },
    /// Войти своим паролем на инстансе.
    OwnLogin {
        /// Инстанс.
        origin: String,
        /// Имя на инстансе.
        username: String,
        /// Пароль на инстансе.
        password: String,
    },
    /// Отключить.
    Disconnect(String),
    /// Убрать запись, не уведомляя инстанс.
    ForceRemove(String),
    /// Удалить учётную запись.
    Delete {
        /// Инстансы.
        origins: Vec<String>,
        /// Что делать с данными.
        mode: IdentityDeleteMode,
    },
}

/// Показывает панель целиком.
pub(crate) fn show(ui: &mut egui::Ui, state: &mut ConnectionsUi, bridge: &Bridge) -> ConnAction {
    let mut action = ConnAction::None;
    ui.label(
        theme::muted(tr(
            "Свяжите учётные записи на разных серверах Backspace. Пространства и \
             личные сообщения всех подключённых серверов показываются в одном окне.",
        ))
        .size(13.0),
    );
    ui.add_space(12.0);

    if let Some(home) = bridge.home_row() {
        home_card(ui, &home);
        ui.add_space(10.0);
    }

    let rows = bridge.rows();
    let counts = Counts::of(&rows);
    filter_bar(ui, state, &counts);
    ui.add_space(8.0);

    let mut shown: Vec<&ConnectionRow> = rows.iter().filter(|r| state.filter.admits(r)).collect();
    match state.sort {
        Sort::DateAdded => shown.sort_by_key(|r| std::cmp::Reverse(r.added_at)),
        Sort::LastConnected => shown.sort_by_key(|r| std::cmp::Reverse(r.last_connected_at)),
    }
    if rows.is_empty() {
        ui.label(
            theme::muted(tr(
                "Другие серверы не подключены. Добавьте сервер, чтобы начать.",
            ))
            .size(13.0),
        );
    } else if shown.is_empty() {
        ui.label(theme::muted(tr("Нет серверов, подходящих под фильтр.")).size(13.0));
    }
    let now = now_ms();
    for row in shown {
        row_card(ui, state, row, now, &mut action);
        ui.add_space(6.0);
    }

    ui.add_space(6.0);
    add_form(ui, state, &mut action);
    action
}

struct Counts {
    all: usize,
    connected: usize,
    disconnected: usize,
    issues: usize,
}

impl Counts {
    fn of(rows: &[ConnectionRow]) -> Self {
        Self {
            all: rows.len(),
            connected: rows.iter().filter(|r| Filter::Connected.admits(r)).count(),
            disconnected: rows
                .iter()
                .filter(|r| Filter::Disconnected.admits(r))
                .count(),
            issues: rows.iter().filter(|r| Filter::Issues.admits(r)).count(),
        }
    }
}

impl Filter {
    const fn admits(self, row: &ConnectionRow) -> bool {
        match self {
            Self::All => true,
            Self::Connected => matches!(row.state, RowState::Connected),
            Self::Disconnected => matches!(row.state, RowState::Disconnected),
            Self::Issues => matches!(row.state, RowState::Unreachable | RowState::AuthExpired),
        }
    }
}

fn home_card(ui: &mut egui::Ui, home: &crate::fleet::HomeRow) {
    egui::Frame::new()
        .fill(theme::bg_sidebar())
        .stroke(Stroke::new(1.0, theme::border()))
        .corner_radius(crate::theme::rounded(10))
        .inner_margin(Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                dot(
                    ui,
                    if home.live {
                        theme::green()
                    } else {
                        theme::amber()
                    },
                );
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(
                        RichText::new(tr("Домашний сервер"))
                            .family(theme::bold_family())
                            .size(15.0)
                            .color(theme::text_bright()),
                    );
                    let host = bs_identity::identity::host_of(&home.base);
                    ui.label(
                        theme::muted(crate::tf!(
                            "{host} как {a0}",
                            host = host,
                            a0 = home.username
                        ))
                        .size(12.0),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(theme::muted(tr("Локальный")).size(12.5));
                });
            });
        });
}

fn filter_bar(ui: &mut egui::Ui, state: &mut ConnectionsUi, counts: &Counts) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (filter, title, count) in [
            (Filter::All, tr("Все"), counts.all),
            (Filter::Connected, tr("Подключённые"), counts.connected),
            (Filter::Disconnected, tr("Отключённые"), counts.disconnected),
            (Filter::Issues, tr("С проблемами"), counts.issues),
        ] {
            if widgets::pill_tab(ui, &format!("{title}  {count}"), state.filter == filter).clicked()
            {
                state.filter = filter;
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            egui::ComboBox::from_id_salt("connections_sort")
                .selected_text(tr("Сортировка"))
                .width(96.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut state.sort, Sort::DateAdded, tr("Дата добавления"));
                    ui.selectable_value(
                        &mut state.sort,
                        Sort::LastConnected,
                        tr("Последнее подключение"),
                    );
                });
        });
    });
}

fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(12.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, color);
}

/// Цвет и подпись значка статуса.
fn badge(state: RowState) -> (Color32, &'static str) {
    match state {
        RowState::Connected => (theme::green(), tr("Подключено")),
        RowState::Connecting => (theme::amber(), tr("Подключается…")),
        RowState::Disconnected => (theme::gray(), tr("Отключено")),
        RowState::Unreachable => (theme::amber(), tr("Недоступен")),
        RowState::AuthExpired => (theme::red(), tr("Нужен вход")),
    }
}

fn status_badge(ui: &mut egui::Ui, state: RowState) {
    let (color, text) = badge(state);
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(11.5), color);
    let size = galley.size() + egui::vec2(14.0, 6.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, crate::theme::rounded(5), color.gamma_multiply(0.16));
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
}

fn row_card(
    ui: &mut egui::Ui,
    state: &mut ConnectionsUi,
    row: &ConnectionRow,
    now: i64,
    action: &mut ConnAction,
) {
    let expanded = state.expanded.contains(&row.origin);
    let (dot_color, _) = badge(row.state);
    let alpha = if matches!(row.state, RowState::Disconnected) {
        0.75
    } else {
        1.0
    };
    let frame = egui::Frame::new()
        .fill(theme::bg_sidebar().gamma_multiply(alpha))
        .stroke(Stroke::new(
            1.0,
            if expanded {
                theme::border()
            } else {
                Color32::TRANSPARENT
            },
        ))
        .corner_radius(crate::theme::rounded(10))
        .inner_margin(Margin::symmetric(14, 11));
    let inner = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        let header = ui.horizontal(|ui| {
            dot(ui, dot_color);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&row.label)
                            .family(theme::bold_family())
                            .size(15.0)
                            .color(theme::text_bright()),
                    );
                    status_badge(ui, row.state);
                    if row.unread > 0 {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::Vec2::splat(8.0), Sense::hover());
                        ui.painter()
                            .circle_filled(rect.center(), 3.5, theme::rose());
                        ui.label(
                            RichText::new(row.unread.to_string())
                                .size(12.0)
                                .color(theme::rose()),
                        )
                        .on_hover_text(tr("Каналов и бесед с непрочитанным"));
                    }
                });
                let host = bs_identity::identity::host_of(&row.origin);
                let who = if row.username.is_empty() {
                    host
                } else {
                    crate::tf!("{host} как {a0}", host = host, a0 = row.username)
                };
                ui.label(theme::muted(who).size(12.0));
                ui.label(theme::muted(meta_line(row, now)).size(11.5));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                chevron(ui, expanded);
            });
        });
        if expanded {
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(8.0);
            details(ui, row);
            ui.add_space(8.0);
            actions(ui, state, row, action);
            ui.add_space(4.0);
            dialog(ui, state, row, action);
        }
        header.response.rect
    });
    let header_rect = egui::Rect::from_min_size(
        inner.response.rect.min,
        egui::vec2(inner.response.rect.width(), inner.inner.height() + 22.0),
    );
    let click = ui.interact(
        header_rect,
        ui.next_auto_id().with(("conn-row", &row.origin)),
        Sense::click(),
    );
    if click.clicked() {
        if expanded {
            state.expanded.remove(&row.origin);
        } else {
            state.expanded.insert(row.origin.clone());
        }
    }
    click.on_hover_cursor(egui::CursorIcon::PointingHand);
}

fn chevron(ui: &mut egui::Ui, open: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(14.0), Sense::hover());
    let c = rect.center();
    let points = if open {
        vec![
            c + egui::vec2(-4.0, -2.0),
            c + egui::vec2(4.0, -2.0),
            c + egui::vec2(0.0, 3.0),
        ]
    } else {
        vec![
            c + egui::vec2(-2.0, -4.0),
            c + egui::vec2(-2.0, 4.0),
            c + egui::vec2(3.0, 0.0),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        theme::text_muted(),
        Stroke::NONE,
    ));
}

fn meta_line(row: &ConnectionRow, now: i64) -> String {
    let mut parts = vec![crate::tf!("Добавлен {a0}", a0 = date(row.added_at))];
    match row.state {
        RowState::Connected => {
            if let Some(at) = row.last_connected_at {
                parts.push(crate::tf!("Подключён {a0}", a0 = relative(at, now)));
            }
        }
        RowState::Disconnected => {
            if let Some(at) = row.disconnected_at {
                parts.push(crate::tf!("Отключён {a0}", a0 = relative(at, now)));
            }
        }
        _ => {
            if let Some(at) = row.last_connected_at {
                parts.push(crate::tf!(
                    "Последнее подключение {a0}",
                    a0 = relative(at, now)
                ));
            }
        }
    }
    parts.join(" · ")
}

fn details(ui: &mut egui::Ui, row: &ConnectionRow) {
    ui.horizontal_top(|ui| {
        let width = (ui.available_width() / 3.0 - 8.0).max(80.0);
        let id = if row.remote_user_id.is_empty() {
            "—".to_owned()
        } else {
            theme::truncate_chars(&row.remote_user_id, 12)
        };
        detail_cell(ui, width, tr("ID УДАЛЁННОГО ПОЛЬЗОВАТЕЛЯ"), &id);
        detail_cell(ui, width, tr("ДОБАВЛЕН"), &date(row.added_at));
        if matches!(row.state, RowState::Disconnected) {
            detail_cell(
                ui,
                width,
                tr("ОТКЛЮЧЁН"),
                &row.disconnected_at.map_or_else(|| "—".to_owned(), date),
            );
        } else {
            detail_cell(
                ui,
                width,
                tr("ПОСЛЕДНЕЕ ПОДКЛЮЧЕНИЕ"),
                &row.last_connected_at.map_or_else(|| "—".to_owned(), date),
            );
        }
    });
    if let Some(status) = &row.peering {
        ui.add_space(4.0);
        ui.label(theme::muted(peering_label(status)).size(12.0));
    }
    if matches!(row.state, RowState::Unreachable | RowState::AuthExpired)
        && let Some(text) = row
            .reason
            .as_deref()
            .map(describe_reason)
            .or_else(|| row.detail.clone())
    {
        ui.add_space(8.0);
        egui::Frame::new()
            .fill(theme::red().gamma_multiply(0.12))
            .stroke(Stroke::new(1.0, theme::red().gamma_multiply(0.35)))
            .corner_radius(crate::theme::rounded(6))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(text).size(12.5).color(theme::rose()));
                if let Some(detail) = row.detail.as_deref() {
                    ui.label(theme::muted(detail).size(11.0));
                }
            });
    }
}

fn detail_cell(ui: &mut egui::Ui, width: f32, caption: &str, value: &str) {
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(theme::muted(caption).size(10.5));
        ui.label(RichText::new(value).size(13.0).color(theme::text()));
    });
}

/// Слова для кода причины из реестра.
fn describe_reason(stored: &str) -> String {
    match stored {
        reason::UNREACHABLE => tr("Этот сервер был недоступен.").to_owned(),
        reason::SESSION_EXPIRED => {
            tr("Сохранённая сессия истекла. Войдите заново, чтобы переподключиться.").to_owned()
        }
        reason::REAUTHENTICATE => tr("Войдите заново, чтобы подключиться.").to_owned(),
        reason::AUTHENTICATE_HOME => {
            tr("Войдите, чтобы подключиться к домашнему серверу.").to_owned()
        }
        other => other.to_owned(),
    }
}

fn peering_label(status: &str) -> &'static str {
    match status {
        "active" => tr("Пиринг установлен"),
        "pending" | "awaiting_approval" => tr("Пиринг ожидает подтверждения"),
        "admin_required" => tr("Нужно одобрение администратора"),
        "rejected" => tr("Пиринг отклонён"),
        "unreachable" => tr("Инстанс не отвечает на пиринг"),
        "revoked" => tr("Пиринг отозван"),
        _ => tr("Пиринг: статус неизвестен"),
    }
}

fn small_button(text: &str, tint: Color32) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).size(12.5).color(tint))
        .fill(tint.gamma_multiply(0.14))
        .corner_radius(crate::theme::rounded(6))
        .min_size(egui::vec2(0.0, 28.0))
}

fn actions(
    ui: &mut egui::Ui,
    state: &mut ConnectionsUi,
    row: &ConnectionRow,
    action: &mut ConnAction,
) {
    ui.horizontal_wrapped(|ui| {
        let origin = row.origin.clone();
        match row.state {
            RowState::Connected => {
                if ui
                    .add(small_button(tr("Отключиться"), theme::text_muted()))
                    .clicked()
                {
                    *action = ConnAction::Disconnect(origin.clone());
                }
                ui.add_enabled(
                    false,
                    small_button(tr("Удалить учётную запись"), theme::rose()),
                )
                .on_disabled_hover_text(tr("Сначала отключитесь, чтобы удалить учётную запись"));
            }
            RowState::Connecting => {
                ui.add_enabled(
                    false,
                    small_button(tr("Подключается…"), theme::text_muted()),
                );
            }
            RowState::Disconnected => {
                if ui
                    .add(small_button(tr("Переподключиться"), theme::accent()))
                    .clicked()
                {
                    *action = ConnAction::Reconnect(origin.clone());
                }
                reauth_button(ui, state, &origin);
                delete_button(ui, state, &origin);
            }
            RowState::Unreachable | RowState::AuthExpired => {
                if matches!(row.state, RowState::Unreachable)
                    && ui
                        .add(small_button(tr("Переподключиться"), theme::accent()))
                        .clicked()
                {
                    *action = ConnAction::Reconnect(origin.clone());
                }
                if matches!(row.state, RowState::AuthExpired) {
                    reauth_button(ui, state, &origin);
                }
                if ui
                    .add(small_button(
                        tr("Удалить принудительно"),
                        theme::text_muted(),
                    ))
                    .clicked()
                {
                    state.dialog = Some(Dialog::ForceRemove {
                        origin: origin.clone(),
                    });
                }
                delete_button(ui, state, &origin);
            }
        }
    });
}

fn reauth_button(ui: &mut egui::Ui, state: &mut ConnectionsUi, origin: &str) {
    if ui
        .add(small_button(tr("Войти заново"), theme::amber()))
        .clicked()
    {
        state.dialog = Some(Dialog::Reauth {
            origin: origin.to_owned(),
            password: String::new(),
        });
    }
}

fn delete_button(ui: &mut egui::Ui, state: &mut ConnectionsUi, origin: &str) {
    if ui
        .add(small_button(tr("Удалить учётную запись"), theme::rose()))
        .clicked()
    {
        state.dialog = Some(Dialog::Delete {
            origin: origin.to_owned(),
            mode: IdentityDeleteMode::Leave,
            everywhere: false,
        });
    }
}

/// Встроенная форма под кнопками карточки.
fn dialog(
    ui: &mut egui::Ui,
    state: &mut ConnectionsUi,
    row: &ConnectionRow,
    action: &mut ConnAction,
) {
    if state.dialog.as_ref().map(Dialog::origin) != Some(row.origin.as_str()) {
        if row.offer_own_login && state.dialog.is_none() {
            own_login_hint(ui, state, row);
        }
        return;
    }
    let mut close = false;
    let Some(current) = state.dialog.as_mut() else {
        return;
    };
    ui.add_space(6.0);
    egui::Frame::new()
        .fill(theme::bg_input())
        .corner_radius(crate::theme::rounded(8))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            match current {
                Dialog::Reauth { origin, password } => {
                    reauth_form(ui, origin, password, action, &mut close);
                }
                Dialog::OwnLogin {
                    origin,
                    username,
                    password,
                } => own_login_form(ui, origin, username, password, action, &mut close),
                Dialog::ForceRemove { origin } => force_remove_form(ui, origin, action, &mut close),
                Dialog::Delete {
                    origin,
                    mode,
                    everywhere,
                } => delete_form(ui, origin, mode, everywhere, action, &mut close),
            }
        });
    if close {
        state.dialog = None;
    }
}

/// Повторный вход паролем домашнего сервера.
fn reauth_form(
    ui: &mut egui::Ui,
    origin: &str,
    password: &mut String,
    action: &mut ConnAction,
    close: &mut bool,
) {
    widgets::field_label(ui, tr("Пароль учётной записи на домашнем сервере"));
    let field = widgets::secret_field(ui, password, tr("пароль"), 200);
    ui.label(
        theme::muted(tr(
            "Пароль проверяет ваш домашний сервер и никогда не отправляется \
             на этот сервер: для него создаются отдельные учётные данные.",
        ))
        .size(11.5),
    );
    ui.add_space(6.0);
    let ready = !password.is_empty();
    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.horizontal(|ui| {
        if ui
            .add_enabled(ready, theme::primary_button(tr("Войти")))
            .clicked()
            || (ready && enter)
        {
            *action = ConnAction::Reauthenticate {
                origin: origin.to_owned(),
                password: std::mem::take(password),
            };
            *close = true;
        }
        if ui.button(tr("Отмена")).clicked() {
            *close = true;
        }
    });
}

/// Вход собственным паролем аккаунта на удалённом сервере.
fn own_login_form(
    ui: &mut egui::Ui,
    origin: &str,
    username: &mut String,
    password: &mut String,
    action: &mut ConnAction,
    close: &mut bool,
) {
    ui.label(
        theme::muted(tr(
            "Сервер не принимает учётные данные, выданные вашим домашним \
             сервером. Войдите с паролем, который вы задали на том сервере: \
             после входа аккаунт будет переведён на выданные данные.",
        ))
        .size(12.0),
    );
    ui.add_space(6.0);
    widgets::field_label(ui, tr("Ваше имя на этом сервере"));
    widgets::text_field(ui, username, tr("имя пользователя"), 100);
    widgets::field_label(ui, tr("Пароль для этого сервера"));
    widgets::secret_field(ui, password, tr("пароль на удалённом сервере"), 200);
    ui.add_space(6.0);
    let ready = !username.trim().is_empty() && !password.is_empty();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(ready, theme::primary_button(tr("Войти и подключиться")))
            .clicked()
        {
            *action = ConnAction::OwnLogin {
                origin: origin.to_owned(),
                username: username.trim().to_owned(),
                password: std::mem::take(password),
            };
            *close = true;
        }
        if ui.button(tr("Отмена")).clicked() {
            *close = true;
        }
    });
}

/// Подтверждение принудительного удаления записи о сервере.
fn force_remove_form(ui: &mut egui::Ui, origin: &str, action: &mut ConnAction, close: &mut bool) {
    ui.label(
        RichText::new(tr("Принудительно удалить запись"))
            .family(theme::bold_family())
            .color(theme::text_bright()),
    );
    ui.label(
        theme::muted(tr(
            "Запись о сервере будет удалена из списка. Удалённый сервер не \
             получит уведомления. Используйте это только если сервер \
             недоступен навсегда.",
        ))
        .size(12.0),
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .add(small_button(tr("Удалить принудительно"), theme::rose()))
            .clicked()
        {
            *action = ConnAction::ForceRemove(origin.to_owned());
            *close = true;
        }
        if ui.button(tr("Отмена")).clicked() {
            *close = true;
        }
    });
}

fn own_login_hint(ui: &mut egui::Ui, state: &mut ConnectionsUi, row: &ConnectionRow) {
    ui.add_space(4.0);
    ui.label(
        theme::muted(tr(
            "На этом сервере уже есть ваша учётная запись, но он не принял выданные \
             домашним сервером данные.",
        ))
        .size(12.0),
    );
    if ui
        .add(small_button(tr("Войти своим паролем"), theme::amber()))
        .clicked()
    {
        state.dialog = Some(Dialog::OwnLogin {
            origin: row.origin.clone(),
            username: row
                .username
                .split('@')
                .next()
                .unwrap_or_default()
                .to_owned(),
            password: String::new(),
        });
    }
}

fn delete_form(
    ui: &mut egui::Ui,
    origin: &str,
    mode: &mut IdentityDeleteMode,
    everywhere: &mut bool,
    action: &mut ConnAction,
    close: &mut bool,
) {
    ui.label(
        RichText::new(tr("Удаление учётной записи"))
            .family(theme::bold_family())
            .color(theme::text_bright()),
    );
    ui.label(
        theme::muted(crate::tf!(
            "Удалить вашу федеративную учётную запись на сервере {a0}. Выберите, что \
             делать с вашими данными.",
            a0 = bs_identity::identity::host_of(origin)
        ))
        .size(12.0),
    );
    ui.add_space(6.0);
    for (value, title, text) in [
        (
            IdentityDeleteMode::Leave,
            tr("Тихо уйти"),
            tr("Отключиться от сервера. Ваша учётная запись и все данные останутся."),
        ),
        (
            IdentityDeleteMode::Soft,
            tr("Удалить пользователя"),
            tr(
                "Удалить учётную запись, но сохранить сообщения. Вы будете отображаться \
             как «Удалённый пользователь».",
            ),
        ),
        (
            IdentityDeleteMode::Full,
            tr("Удалить всё"),
            tr(
                "Удалить учётную запись и все ваши сообщения, личные сообщения, реакции и \
             файлы. Ничего не останется.",
            ),
        ),
    ] {
        if widgets::radio_card(ui, title, text, *mode == value).clicked() {
            *mode = value;
        }
        ui.add_space(4.0);
    }
    ui.checkbox(everywhere, tr("Применить ко всем подключённым серверам"));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let label = if *mode == IdentityDeleteMode::Leave {
            tr("Отключиться")
        } else {
            tr("Удалить учётную запись")
        };
        if ui.add(small_button(label, theme::rose())).clicked() {
            *action = ConnAction::Delete {
                origins: if *everywhere {
                    Vec::new()
                } else {
                    vec![origin.to_owned()]
                },
                mode: *mode,
            };
            *close = true;
        }
        if ui.button(tr("Отмена")).clicked() {
            *close = true;
        }
    });
}

fn add_form(ui: &mut egui::Ui, state: &mut ConnectionsUi, action: &mut ConnAction) {
    if !state.add_open {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), Sense::click());
        let hovered = response.hovered();
        ui.painter().rect_stroke(
            rect,
            crate::theme::rounded(10),
            Stroke::new(
                1.0,
                if hovered {
                    theme::accent()
                } else {
                    theme::border()
                },
            ),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            tr("+ Добавить сервер"),
            egui::FontId::proportional(14.0),
            if hovered {
                theme::text_bright()
            } else {
                theme::text_muted()
            },
        );
        if response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            state.add_open = true;
        }
        return;
    }
    egui::Frame::new()
        .fill(theme::bg_sidebar())
        .stroke(Stroke::new(1.0, theme::border()))
        .corner_radius(crate::theme::rounded(10))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(tr("Добавить удалённый сервер"))
                    .family(theme::bold_family())
                    .color(theme::text_bright()),
            );
            ui.add_space(6.0);
            widgets::field_label(ui, tr("Адрес сервера"));
            widgets::text_field(ui, &mut state.address, "https://instance.example.com", 200);
            widgets::field_label(ui, tr("Пароль учётной записи на домашнем сервере"));
            let field =
                widgets::secret_field(ui, &mut state.password, tr("тот же, что при входе"), 200);
            ui.add_space(4.0);
            ui.label(
                theme::muted(tr(
                    "Пароль проверяет ваш домашний сервер и никогда не отправляется на \
                     подключаемый сервер: для него создаются отдельные уникальные \
                     учётные данные.",
                ))
                .size(12.0),
            );
            ui.add_space(8.0);
            let ready = !state.address.trim().is_empty() && !state.password.is_empty();
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(ready, theme::primary_button(tr("Подключиться")))
                    .clicked()
                    || (ready && enter)
                {
                    *action = ConnAction::Connect {
                        address: std::mem::take(&mut state.address),
                        password: std::mem::take(&mut state.password),
                    };
                    state.add_open = false;
                }
                if ui.button(tr("Отмена")).clicked() {
                    state.add_open = false;
                    state.password.clear();
                }
            });
        });
}

const MONTHS: [&str; 12] = [
    "янв.",
    "февр.",
    "мар.",
    "апр.",
    "мая",
    "июн.",
    "июл.",
    "авг.",
    "сент.",
    "окт.",
    "нояб.",
    "дек.",
];

/// Дата вида «2 окт. 2026».
fn date(ms: i64) -> String {
    chrono::Local.timestamp_millis_opt(ms).single().map_or_else(
        || "—".to_owned(),
        |d| {
            format!(
                "{} {} {}",
                d.day(),
                tr(MONTHS[d.month0() as usize]),
                d.year()
            )
        },
    )
}

/// «только что», «5 мин назад», «вчера»…
fn relative(ms: i64, now: i64) -> String {
    let secs = ((now - ms) / 1000).max(0);
    let (minutes, hours, days) = (secs / 60, secs / 3600, secs / 86_400);
    if secs < 60 {
        tr("только что").to_owned()
    } else if minutes < 60 {
        crate::tf!("{minutes} мин назад", minutes = minutes)
    } else if hours < 24 {
        crate::tf!("{hours} ч назад", hours = hours)
    } else if days == 1 {
        tr("вчера").to_owned()
    } else {
        crate::tf!("{days} дн. назад", days = days)
    }
}

#[cfg(test)]
mod tests {
    use super::{Filter, describe_reason, relative};
    use crate::fleet::{ConnectionRow, RowState};

    fn row(state: RowState) -> ConnectionRow {
        ConnectionRow {
            origin: "https://a.example".into(),
            label: "A".into(),
            username: "q@h".into(),
            remote_user_id: String::new(),
            state,
            reason: None,
            detail: None,
            added_at: 0,
            last_connected_at: None,
            disconnected_at: None,
            unread: 0,
            peering: None,
            offer_own_login: false,
        }
    }

    #[test]
    fn issues_filter_takes_unreachable_and_expired_only() {
        assert!(Filter::Issues.admits(&row(RowState::Unreachable)));
        assert!(Filter::Issues.admits(&row(RowState::AuthExpired)));
        assert!(!Filter::Issues.admits(&row(RowState::Connected)));
        assert!(!Filter::Issues.admits(&row(RowState::Disconnected)));
    }

    #[test]
    fn relative_time_reads_naturally() {
        assert_eq!(relative(0, 30_000), "только что");
        assert_eq!(relative(0, 5 * 60_000), "5 мин назад");
        assert_eq!(relative(0, 3 * 3_600_000), "3 ч назад");
        assert_eq!(relative(0, 30 * 3_600_000), "вчера");
        assert_eq!(relative(0, 4 * 86_400_000), "4 дн. назад");
    }

    #[test]
    fn stored_codes_become_words_and_unknown_text_stays() {
        assert!(describe_reason("session_expired").contains("сессия"));
        assert_eq!(describe_reason("old english"), "old english");
    }
}
