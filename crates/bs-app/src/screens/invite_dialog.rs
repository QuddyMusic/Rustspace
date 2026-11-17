//! «Пригласить людей»: ссылка-приглашение и отправка приглашения друзьям в
//! личные сообщения (`POST /api/dm/space-invite`), как в оригинале.

use std::collections::HashMap;

use bs_types::Friend;
use egui::{CornerRadius, Margin, RichText};

use crate::bridge::BridgeRequest;
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::social::user_display_name;
use crate::{theme, widgets};

/// Состояние отправки одному другу.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Sent {
    /// Запрос в пути.
    Pending,
    /// Код получен, карточка отправляется.
    Sending,
    /// Отправлено.
    Done,
    /// Сервер отказал.
    Failed(String),
}

/// Окно приглашения.
#[derive(Debug)]
pub(crate) struct InviteDialog {
    /// Инстанс пространства.
    origin: usize,
    /// Пространство.
    space_id: String,
    /// Название для заголовка.
    space_name: String,
    /// Код приглашения (пока не получен — `None`).
    code: Option<Result<String, String>>,
    /// Кому уже отправляли.
    sent: HashMap<String, Sent>,
    /// Фильтр по имени.
    filter: String,
    /// Ссылку только что скопировали.
    copied: bool,
}

impl InviteDialog {
    /// Открывает окно и запрашивает код приглашения.
    pub(crate) fn open(bridge: &mut Bridge, origin: usize, space_id: String) -> Self {
        let space_name = bridge
            .cache_at(origin)
            .and_then(|cache| cache.space(&space_id))
            .map(|space| space.space.name.clone())
            .unwrap_or_default();
        let previous = bridge.focus_index();
        bridge.focus_on(origin);
        bridge.request(BridgeRequest::SpaceInvite(space_id.clone()));
        bridge.focus_on(previous);
        Self {
            origin,
            space_id,
            space_name,
            code: None,
            sent: HashMap::new(),
            filter: String::new(),
            copied: false,
        }
    }

    /// Пришёл код приглашения для пространства этого окна.
    pub(crate) fn accept_code(&mut self, origin: usize, result: &Result<String, String>) -> bool {
        if origin != self.origin || self.code.is_some() {
            return false;
        }
        self.code = Some(result.clone());
        true
    }

    /// Итог отправки другу.
    pub(crate) fn accept_sent(&mut self, friend_id: &str, result: &Result<(), String>) {
        if let Some(slot) = self.sent.get_mut(friend_id) {
            *slot = match result {
                Ok(()) => Sent::Done,
                Err(err) => Sent::Failed(err.clone()),
            };
        }
    }
}

/// Рисует окно; `false` — закрыть.
pub(crate) fn show(ctx: &egui::Context, state: &mut InviteDialog, bridge: &mut Bridge) -> bool {
    let mut open = true;
    egui::Window::new(crate::tf!("Пригласить в «{a0}»", a0 = state.space_name))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .default_width(440.0)
        .max_width(440.0)
        .open(&mut open)
        .show(ctx, |ui| {
            link_row(ui, state, bridge);
            ui.add_space(12.0);
            friends(ui, state, bridge);
        });
    open
}

fn link_row(ui: &mut egui::Ui, state: &mut InviteDialog, bridge: &Bridge) {
    widgets::section_label(ui, tr("Ссылка-приглашение"));
    ui.add_space(4.0);
    match &state.code {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::muted(tr("Получаем код…")));
            });
        }
        Some(Err(err)) => {
            ui.label(RichText::new(err).color(theme::rose()));
        }
        Some(Ok(code)) => {
            let url = bs_api::invite_url(bridge.base_at(state.origin).unwrap_or_default(), code);
            ui.horizontal(|ui| {
                egui::Frame::new()
                    .fill(theme::bg_input())
                    .corner_radius(CornerRadius::same(theme::radius()))
                    .inner_margin(Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width() - 130.0);
                        ui.add(egui::Label::new(RichText::new(&url).monospace()).truncate());
                    });
                let label = if state.copied {
                    tr("Скопировано")
                } else {
                    tr("Копировать")
                };
                if ui.add(theme::primary_button(label)).clicked() {
                    ui.ctx().copy_text(url.clone());
                    state.copied = true;
                }
            });
        }
    }
}

fn friends(ui: &mut egui::Ui, state: &mut InviteDialog, bridge: &mut Bridge) {
    widgets::section_label(ui, tr("Друзья"));
    ui.add_space(4.0);
    let friends: Vec<Friend> = bridge.friends().to_vec();
    if friends.is_empty() {
        ui.label(theme::muted(tr(
            "Добавьте друзей, чтобы приглашать их в личных сообщениях.",
        )));
        return;
    }
    widgets::text_field(ui, &mut state.filter, tr("Найти друга"), 64);
    ui.add_space(6.0);
    let filter = state.filter.trim().to_lowercase();
    let code = state.code.as_ref().and_then(|c| c.as_ref().ok()).cloned();
    let mut send = None;
    egui::ScrollArea::vertical()
        .max_height(300.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for friend in &friends {
                let name = user_display_name(friend.display_name.as_deref(), &friend.username);
                if !filter.is_empty()
                    && !name.to_lowercase().contains(&filter)
                    && !friend.username.to_lowercase().contains(&filter)
                {
                    continue;
                }
                ui.horizontal(|ui| {
                    theme::avatar(
                        ui,
                        theme::Face {
                            name: &name,
                            key: Some(friend.home_user_id.as_deref().unwrap_or(&friend.id)),
                            color: friend.avatar_color.as_deref(),
                        },
                        32.0,
                        Some(friend.status),
                        friend.avatar.as_deref(),
                    );
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&name).color(theme::text_bright()));
                        ui.label(theme::muted(format!("@{}", friend.username)).size(12.0));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match state.sent.get(&friend.id) {
                            Some(Sent::Pending | Sent::Sending) => {
                                ui.spinner();
                            }
                            Some(Sent::Done) => {
                                ui.label(RichText::new(tr("Отправлено")).color(theme::mint()));
                            }
                            status => {
                                if let Some(Sent::Failed(err)) = status {
                                    ui.label(RichText::new(tr("Ошибка")).color(theme::rose()))
                                        .on_hover_text(err);
                                }
                                if ui
                                    .add_enabled(
                                        code.is_some(),
                                        theme::positive_button(tr("Пригласить")),
                                    )
                                    .clicked()
                                {
                                    send = Some(friend.clone());
                                }
                            }
                        }
                    });
                });
                ui.add_space(4.0);
            }
        });
    if let (Some(friend), Some(code)) = (send, code) {
        send_invite(state, bridge, &friend, code);
    }
}

/// Отправляет приглашение одному другу с домашнего инстанса.
fn send_invite(state: &mut InviteDialog, bridge: &mut Bridge, friend: &Friend, code: String) {
    state.sent.insert(friend.id.clone(), Sent::Pending);
    send_space_invite(bridge, state.origin, &state.space_id, friend, code);
}

/// Приглашение в пространство `space_id` инстанса `origin` уходит другу
/// карточкой в личные сообщения с домашнего инстанса.
fn send_space_invite(
    bridge: &mut Bridge,
    origin: usize,
    space_id: &str,
    friend: &Friend,
    code: String,
) {
    // Друзья живут на домашнем инстансе: приглашение уходит оттуда, а
    // для пространства с другого инстанса передаётся его адрес.
    let space_instance_origin = if origin == 0 {
        String::new()
    } else {
        bridge
            .base_at(origin)
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_owned()
    };
    let target = match (&friend.home_user_id, &friend.home_instance) {
        (Some(home_user_id), Some(home_instance)) => bs_api::InviteTarget::Remote {
            home_user_id: home_user_id.clone(),
            home_instance: home_instance.clone(),
        },
        _ => bs_api::InviteTarget::Local {
            user_id: friend.id.clone(),
        },
    };
    let previous = bridge.focus_index();
    bridge.focus_on(0);
    bridge.request(BridgeRequest::InviteFriend {
        friend_id: friend.id.clone(),
        request: bs_api::SpaceInviteRequest {
            target,
            space_id: space_id.to_owned(),
            space_instance_origin,
            invite_code: code,
        },
    });
    bridge.focus_on(previous);
}

/// Человек из профиля пользователя: собеседник в ЛС тоже подходит.
#[must_use]
pub(crate) fn friend_from_user(user: &bs_types::User) -> Friend {
    Friend {
        id: user.id.clone(),
        username: user.username.clone(),
        display_name: user.display_name.clone(),
        avatar: user.avatar.clone(),
        banner: None,
        accent_color: None,
        avatar_color: user.avatar_color.clone(),
        bio: None,
        status: user.status,
        custom_status: None,
        created_at: user.created_at,
        added_at: 0,
        home_user_id: user.home_user_id.clone(),
        home_instance: user.home_instance.clone(),
    }
}

/// «Пригласить в пространство» из меню друга или беседы (PR #485
/// оригинала, bonderaustria): список своих пространств; щелчок получает
/// код именно этого пространства и отправляет карточку в ЛС.
#[derive(Debug)]
pub(crate) struct PersonInvite {
    person: Friend,
    /// Состояние по пространствам (инстанс, пространство).
    sent: HashMap<(usize, String), Sent>,
    filter: String,
}

impl PersonInvite {
    /// Окно для этого человека.
    #[must_use]
    pub(crate) fn open(person: Friend) -> Self {
        Self {
            person,
            sent: HashMap::new(),
            filter: String::new(),
        }
    }

    /// Пришёл код пространства: если его ждали — отправить приглашение.
    pub(crate) fn accept_code(
        &mut self,
        bridge: &mut Bridge,
        origin: usize,
        space_id: &str,
        result: &Result<String, String>,
    ) -> bool {
        let key = (origin, space_id.to_owned());
        if self.sent.get(&key) != Some(&Sent::Pending) {
            return false;
        }
        match result {
            Ok(code) => {
                self.sent.insert(key, Sent::Sending);
                send_space_invite(bridge, origin, space_id, &self.person, code.clone());
            }
            Err(err) => {
                self.sent.insert(key, Sent::Failed(err.clone()));
            }
        }
        true
    }

    /// Итог отправки: отмечает пространство, которое ещё ждёт.
    pub(crate) fn accept_sent(&mut self, friend_id: &str, result: &Result<(), String>) -> bool {
        if friend_id != self.person.id {
            return false;
        }
        // Ответ на отправку не называет пространство: закрываем первое ожидающее.
        if let Some(slot) = self.sent.values_mut().find(|s| **s == Sent::Sending) {
            *slot = match result {
                Ok(()) => Sent::Done,
                Err(err) => Sent::Failed(err.clone()),
            };
        }
        true
    }
}

/// Свои пространства всех инстансов: (инстанс, id, название, значок, цвет).
type SpaceRow = (usize, String, String, Option<String>, Option<String>);

fn all_spaces(bridge: &Bridge) -> Vec<SpaceRow> {
    let mut spaces = Vec::new();
    for origin in bridge.live_origins() {
        if let Some(cache) = bridge.cache_at(origin) {
            for space in cache.spaces() {
                spaces.push((
                    origin,
                    space.space.id.clone(),
                    space.space.name.clone(),
                    space.space.icon.clone(),
                    space.space.avatar_color.clone(),
                ));
            }
        }
    }
    spaces
}

/// Значок пространства 32×32: картинка или буква на цвете.
fn space_icon(ui: &mut egui::Ui, bridge: &Bridge, row: &SpaceRow) {
    let (origin, _, title, icon, color) = row;
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(32.0), egui::Sense::hover());
    let base = bridge.base_at(*origin).unwrap_or_default();
    let drawn = icon.as_deref().is_some_and(|icon| {
        crate::images::paint_value(
            ui.painter(),
            rect,
            &crate::images::absolute(base, icon),
            theme::rounded(10),
            true,
            egui::Color32::WHITE,
        )
    });
    if !drawn {
        let fill = color
            .as_deref()
            .and_then(theme::palette)
            .unwrap_or_else(theme::accent);
        ui.painter().rect_filled(rect, theme::rounded(10), fill);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            title
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default(),
            egui::FontId::new(15.0, theme::bold_family()),
            egui::Color32::WHITE,
        );
    }
}

/// Строка пространства; `true` — нажали «Пригласить».
fn space_row(ui: &mut egui::Ui, bridge: &Bridge, state: &PersonInvite, row: &SpaceRow) -> bool {
    let mut chosen = false;
    ui.horizontal(|ui| {
        space_icon(ui, bridge, row);
        ui.label(RichText::new(&row.2).color(theme::text_bright()));
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| match state.sent.get(&(row.0, row.1.clone())) {
                Some(Sent::Pending | Sent::Sending) => {
                    ui.spinner();
                }
                Some(Sent::Done) => {
                    ui.label(RichText::new(tr("Отправлено")).color(theme::mint()));
                }
                status => {
                    if let Some(Sent::Failed(err)) = status {
                        ui.label(RichText::new(tr("Ошибка")).color(theme::rose()))
                            .on_hover_text(err);
                    }
                    chosen = ui.add(theme::positive_button(tr("Пригласить"))).clicked();
                }
            },
        );
    });
    chosen
}

/// Окно «Пригласить в пространство»; `false` — закрыть.
pub(crate) fn show_person(
    ctx: &egui::Context,
    state: &mut PersonInvite,
    bridge: &mut Bridge,
) -> bool {
    let mut open = true;
    let name = user_display_name(state.person.display_name.as_deref(), &state.person.username);
    let mut chosen = None;
    egui::Window::new(crate::tf!("Пригласить {a0} в пространство", a0 = name))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .default_width(420.0)
        .max_width(420.0)
        .open(&mut open)
        .show(ctx, |ui| {
            let spaces = all_spaces(bridge);
            if spaces.is_empty() {
                ui.label(theme::muted(tr("У вас пока нет пространств.")));
                return;
            }
            widgets::text_field(ui, &mut state.filter, tr("Найти пространство"), 64);
            ui.add_space(6.0);
            let filter = state.filter.trim().to_lowercase();
            egui::ScrollArea::vertical()
                .max_height(340.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for row in &spaces {
                        if !filter.is_empty() && !row.2.to_lowercase().contains(&filter) {
                            continue;
                        }
                        if space_row(ui, bridge, state, row) {
                            chosen = Some((row.0, row.1.clone()));
                        }
                        ui.add_space(4.0);
                    }
                });
        });
    if let Some((origin, space_id)) = chosen {
        state.sent.insert((origin, space_id.clone()), Sent::Pending);
        let previous = bridge.focus_index();
        bridge.focus_on(origin);
        bridge.request(BridgeRequest::SpaceInvite(space_id));
        bridge.focus_on(previous);
    }
    open
}
