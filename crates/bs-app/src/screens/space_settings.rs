//! Настройки пространства: «Основное», «Участники», «Блокировки».
//!
//! Тексты — как в `spaceSettingsPanels` и `TransferOwnershipModal`
//! оригинального веб-клиента. Права лишь решают, что показать; решает сервер.

mod roles;

use bs_api::{Ban, SpaceUpdate};
use bs_permissions::{Permissions, can_manage_role_at, has_permission_bit, parse_permission_string};
use bs_types::SpaceVisibility;
use egui::{Align, CornerRadius, Layout, Margin, RichText};

use crate::bridge::{BridgeRequest, SpaceEvent, SpaceOp};
use crate::fleet::Bridge;
use crate::icons::Icon;
use crate::{theme, widgets};

const NAME_MAX: usize = 100;
const DESCRIPTION_MAX: usize = 200;

/// Вкладка окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Members,
    Roles,
    Bans,
}

/// Что ждёт подтверждения.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Confirm {
    Kick(String),
    Ban(String),
    Transfer(String),
}

/// Снимок редактируемых полей.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fields {
    name: String,
    color: String,
    description: String,
    visibility: SpaceVisibility,
}

/// Состояние окна настроек пространства.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools, reason = "независимые флаги интерфейса")]
pub(crate) struct SpaceSettings {
    origin: usize,
    space_id: String,
    tab: Tab,
    fields: Fields,
    saved: Fields,
    bans: Option<Vec<Ban>>,
    bans_requested: bool,
    confirm: Option<Confirm>,
    reason: String,
    search: String,
    transfer_open: bool,
    delete_armed: bool,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    roles: roles::RolesState,
}

/// Права текущего пользователя в пространстве.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::struct_excessive_bools, reason = "независимые права")]
struct Rights {
    owner: bool,
    manage_space: bool,
    kick: bool,
    ban: bool,
    manage_roles: bool,
    /// Все права текущего пользователя (владелец — все).
    perms: Permissions,
    /// Позиция высшей роли (владелец — наивысшая).
    top: i64,
}

impl SpaceSettings {
    /// Открывает окно для пространства из кэша `origin`.
    pub(crate) fn open(bridge: &Bridge, origin: usize, space_id: &str) -> Option<Self> {
        let space = bridge.cache_at(origin)?.space(space_id)?;
        let fields = Fields {
            name: space.space.name.clone(),
            color: space.space.avatar_color.clone().unwrap_or_else(|| "mint".to_owned()),
            description: space.space.description.clone().unwrap_or_default(),
            visibility: space.space.visibility,
        };
        Some(Self {
            origin,
            space_id: space_id.to_owned(),
            tab: Tab::Overview,
            saved: fields.clone(),
            fields,
            bans: None,
            bans_requested: false,
            confirm: None,
            reason: String::new(),
            search: String::new(),
            transfer_open: false,
            delete_armed: false,
            busy: false,
            error: None,
            notice: None,
            roles: roles::RolesState::default(),
        })
    }

    /// Учитывает итог действия. `false` — окно надо закрыть.
    pub(crate) fn apply(&mut self, event: &SpaceEvent) -> bool {
        match event {
            SpaceEvent::Admin {
                space_id,
                op,
                result,
                ..
            } if *space_id == self.space_id => {
                self.busy = false;
                match (op, result) {
                    (SpaceOp::Delete, Ok(())) => return false,
                    (op, Err(err)) => {
                        if matches!(op, SpaceOp::UpdateRole(_, _)) && err.starts_with("Кто-то другой") {
                            self.roles.reload();
                        }
                        self.error = Some(err.clone());
                    }
                    (SpaceOp::Update(_), Ok(())) => {
                        self.saved = self.fields.clone();
                        self.notice = Some("Настройки сохранены".to_owned());
                    }
                    (SpaceOp::Transfer(_), Ok(())) => {
                        self.transfer_open = false;
                        self.confirm = None;
                        self.notice = Some("Права владельца переданы".to_owned());
                    }
                    (SpaceOp::Kick(_) | SpaceOp::Ban(_, _), Ok(())) => {
                        self.confirm = None;
                        self.reason.clear();
                        self.bans = None;
                        self.bans_requested = false;
                    }
                    (SpaceOp::Unban(user), Ok(())) => {
                        if let Some(bans) = self.bans.as_mut() {
                            bans.retain(|b| b.user_id != *user);
                        }
                    }
                    (SpaceOp::UpdateRole(_, _), Ok(())) => {
                        self.roles.reload();
                        self.notice = Some("Роль сохранена".to_owned());
                    }
                    (SpaceOp::DeleteRole(_), Ok(())) => self.roles.closed(),
                    (SpaceOp::CreateRole(_), Ok(())) => {
                        self.notice = Some("Роль создана. Настройте её ниже.".to_owned());
                    }
                    (
                        SpaceOp::ListBans
                        | SpaceOp::AddMemberRole(_, _)
                        | SpaceOp::RemoveMemberRole(_, _),
                        Ok(()),
                    ) => {}
                }
            }
            SpaceEvent::Bans { space_id, result } if *space_id == self.space_id => match result {
                Ok(list) => self.bans = Some(list.clone()),
                Err(err) => {
                    self.bans = Some(Vec::new());
                    self.error = Some(err.clone());
                }
            },
            SpaceEvent::Left {
                space_id,
                result: Ok(()),
            } if *space_id == self.space_id => return false,
            _ => {}
        }
        true
    }

    fn rights(&self, bridge: &Bridge) -> Option<Rights> {
        let cache = bridge.cache_at(self.origin)?;
        let space = cache.space(&self.space_id)?;
        let owner = space.space.owner_id == cache.me().id;
        let bits = space
            .my_permissions
            .as_deref()
            .and_then(|raw| parse_permission_string(raw).ok())
            .unwrap_or(Permissions::empty());
        let has = |flag| owner || has_permission_bit(bits, flag);
        Some(Rights {
            owner,
            manage_space: has(Permissions::MANAGE_SPACE),
            kick: has(Permissions::KICK_MEMBERS),
            ban: has(Permissions::BAN_MEMBERS),
            manage_roles: has(Permissions::MANAGE_ROLES),
            perms: if owner { Permissions::all() } else { bits },
            top: if owner {
                i64::MAX
            } else {
                space
                    .members
                    .iter()
                    .find(|m| m.user.id == cache.me().id)
                    .and_then(|m| m.roles.iter().map(|r| r.position).max())
                    .unwrap_or(0)
            },
        })
    }

    fn request(&mut self, bridge: &mut Bridge, op: SpaceOp) {
        self.busy = true;
        self.error = None;
        self.notice = None;
        bridge.focus_on(self.origin);
        bridge.request(BridgeRequest::SpaceAdmin {
            space_id: self.space_id.clone(),
            op,
        });
    }

    fn update_patch(&self) -> SpaceUpdate {
        SpaceUpdate {
            name: Some(self.fields.name.trim().to_owned()),
            avatar_color: Some(self.fields.color.clone()),
            description: Some(self.fields.description.trim().to_owned()),
            visibility: Some(self.fields.visibility),
        }
    }
}

/// Рисует окно. Возвращает `false`, когда его надо закрыть.
pub(crate) fn show(ctx: &egui::Context, state: &mut SpaceSettings, bridge: &mut Bridge) -> bool {
    let Some(rights) = state.rights(bridge) else {
        return false;
    };
    if state.tab == Tab::Bans && !state.bans_requested && rights.ban {
        state.bans_requested = true;
        bridge.focus_on(state.origin);
        bridge.request(BridgeRequest::SpaceAdmin {
            space_id: state.space_id.clone(),
            op: SpaceOp::ListBans,
        });
    }
    let mut keep_open = true;
    let response = egui::Modal::new(egui::Id::unique("space_settings"))
        .frame(
            egui::Frame::new()
                .fill(theme::BG_BUBBLE)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(16))
                .inner_margin(Margin::same(0)),
        )
        .show(ctx, |ui| {
            ui.set_width(760.0);
            ui.set_height(520.0);
            ui.horizontal_top(|ui| {
                nav(ui, state, rights, &mut keep_open);
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() - 18.0);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add_space(18.0);
                            match state.tab {
                                Tab::Overview => overview(ui, state, rights, bridge),
                                Tab::Members => members(ui, state, rights, bridge),
                                Tab::Roles => roles::show(ui, state, rights, bridge),
                                Tab::Bans => bans(ui, state, bridge),
                            }
                            ui.add_space(16.0);
                        });
                });
            });
        });
    keep_open && !response.should_close()
}

fn nav(ui: &mut egui::Ui, state: &mut SpaceSettings, rights: Rights, keep_open: &mut bool) {
    ui.vertical(|ui| {
        ui.set_width(190.0);
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            widgets::section_label(ui, "Настройки пространства");
        });
        ui.add_space(8.0);
        let mut tabs = vec![(Tab::Overview, "Основное", Icon::Gear)];
        tabs.push((Tab::Members, "Участники", Icon::Users));
        if rights.manage_roles {
            tabs.push((Tab::Roles, "Роли", Icon::Gear));
        }
        if rights.ban {
            tabs.push((Tab::Bans, "Блокировки", Icon::User));
        }
        for (tab, label, icon) in tabs {
            if widgets::nav_row(ui, icon, label, state.tab == tab).clicked() {
                state.tab = tab;
                state.error = None;
                state.notice = None;
                state.confirm = None;
            }
        }
        ui.add_space(ui.available_height() - 36.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            if ui.button("Закрыть").clicked() {
                *keep_open = false;
            }
        });
    });
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(19.0)
            .family(theme::bold_family())
            .color(theme::TEXT_BRIGHT),
    );
    ui.add_space(8.0);
}

fn status_lines(ui: &mut egui::Ui, state: &SpaceSettings) {
    if let Some(err) = &state.error {
        ui.add_space(6.0);
        ui.label(RichText::new(err).color(theme::ROSE));
    }
    if let Some(note) = &state.notice {
        ui.add_space(6.0);
        ui.label(RichText::new(note).color(theme::TEXT_MUTED));
    }
}

fn overview(ui: &mut egui::Ui, state: &mut SpaceSettings, rights: Rights, bridge: &mut Bridge) {
    heading(ui, "Основное");
    ui.add_enabled_ui(rights.manage_space, |ui| {
        widgets::field_label(ui, "Цвет значка");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for name in theme::PALETTE_NAMES {
                let color = theme::palette(name).unwrap_or(theme::ACCENT);
                if widgets::color_swatch(ui, color, state.fields.color == name, 30.0).clicked() {
                    name.clone_into(&mut state.fields.color);
                }
            }
        });
        widgets::field_label(ui, "Название пространства");
        widgets::text_field(ui, &mut state.fields.name, "Название", NAME_MAX);
        widgets::field_label(ui, "Видимость");
        for (value, title, hint) in [
            (SpaceVisibility::Private, "Приватное", "Только по ссылке-приглашению"),
            (SpaceVisibility::Request, "По заявке", "Видно в обзоре, вступление по запросу"),
            (SpaceVisibility::Public, "Публичное", "Видно в обзоре, вступить может любой"),
        ] {
            if widgets::radio_card(ui, title, hint, state.fields.visibility == value).clicked() {
                state.fields.visibility = value;
            }
        }
        widgets::field_label(ui, "Описание");
        widgets::text_area(
            ui,
            &mut state.fields.description,
            "Коротко о вашем пространстве…",
            DESCRIPTION_MAX,
            3,
        );
    });
    status_lines(ui, state);
    let dirty = state.fields != state.saved;
    if rights.manage_space && dirty {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let valid = !state.fields.name.trim().is_empty();
            if ui
                .add_enabled(
                    valid && !state.busy,
                    theme::primary_button(if state.busy { "Сохранение…" } else { "Сохранить" }),
                )
                .clicked()
            {
                let op = SpaceOp::Update(state.update_patch());
                state.request(bridge, op);
            }
            if ui.button("Отменить изменения").clicked() {
                state.fields = state.saved.clone();
            }
        });
    }
    if rights.owner {
        danger_zone(ui, state, bridge);
    }
}

fn danger_zone(ui: &mut egui::Ui, state: &mut SpaceSettings, bridge: &mut Bridge) {
    ui.add_space(22.0);
    ui.label(
        RichText::new("ОПАСНАЯ ЗОНА")
            .size(11.0)
            .family(theme::bold_family())
            .color(theme::ROSE),
    );
    ui.add_space(6.0);
    ui.label(
        RichText::new(
            "Передать права владельца другому участнику. Вы станете обычным участником.",
        )
        .color(theme::TEXT_MUTED),
    );
    ui.add_space(6.0);
    if state.transfer_open {
        transfer_picker(ui, state, bridge);
    } else if ui.button("Передать права владельца").clicked() {
        state.transfer_open = true;
        state.confirm = None;
    }
    ui.add_space(14.0);
    ui.label(
        RichText::new(
            "Удалить это пространство и все его данные навсегда. Это нельзя отменить.",
        )
        .color(theme::TEXT_MUTED),
    );
    ui.add_space(6.0);
    let label = if state.delete_armed {
        "Нажмите ещё раз, чтобы подтвердить удаление"
    } else {
        "Удалить пространство"
    };
    let delete = ui.add_enabled(
        !state.busy,
        egui::Button::new(RichText::new(label).color(theme::BG_RAIL))
            .fill(theme::ROSE)
            .corner_radius(CornerRadius::same(8)),
    );
    if delete.clicked() {
        if state.delete_armed {
            state.request(bridge, SpaceOp::Delete);
        } else {
            state.delete_armed = true;
        }
    }
}

fn display_name(user: &bs_types::User) -> String {
    user.display_name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| user.username.clone())
}

fn transfer_picker(ui: &mut egui::Ui, state: &mut SpaceSettings, bridge: &mut Bridge) {
    if let Some(Confirm::Transfer(target)) = state.confirm.clone() {
        let (space_name, member) = bridge
            .cache_at(state.origin)
            .and_then(|c| c.space(&state.space_id))
            .map(|s| {
                let member = s
                    .members
                    .iter()
                    .find(|m| m.user.id == target)
                    .map(|m| display_name(&m.user))
                    .unwrap_or_default();
                (s.space.name.clone(), member)
            })
            .unwrap_or_default();
        ui.label(format!(
            "Передать права владельца «{space_name}» пользователю {member}? Вы станете обычным участником."
        ));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !state.busy,
                    theme::primary_button(if state.busy { "Передаём права…" } else { "Передать" }),
                )
                .clicked()
            {
                state.request(bridge, SpaceOp::Transfer(target));
            }
            if ui.button("Назад").clicked() {
                state.confirm = None;
            }
        });
        return;
    }
    widgets::text_field(ui, &mut state.search, "Поиск участников…", 64);
    ui.add_space(4.0);
    let needle = state.search.trim().to_lowercase();
    let candidates: Vec<(String, String)> = bridge
        .cache_at(state.origin)
        .and_then(|c| c.space(&state.space_id).map(|s| (c.me().id.clone(), s)))
        .map(|(me, s)| {
            s.members
                .iter()
                .filter(|m| m.user.id != me)
                .map(|m| (m.user.id.clone(), display_name(&m.user)))
                .filter(|(_, name)| needle.is_empty() || name.to_lowercase().contains(&needle))
                .collect()
        })
        .unwrap_or_default();
    if candidates.is_empty() {
        ui.label(RichText::new("Участники не найдены").color(theme::TEXT_MUTED));
    }
    for (id, name) in candidates {
        if ui.button(name).clicked() {
            state.confirm = Some(Confirm::Transfer(id));
        }
    }
    if ui.button("Отмена").clicked() {
        state.transfer_open = false;
    }
}

struct Row {
    id: String,
    name: String,
    username: String,
    roles: Vec<String>,
    role_ids: Vec<String>,
    top: i64,
    owner: bool,
    me: bool,
}

fn members(ui: &mut egui::Ui, state: &mut SpaceSettings, rights: Rights, bridge: &mut Bridge) {
    heading(ui, "Участники");
    let rows: Vec<Row> = bridge
        .cache_at(state.origin)
        .and_then(|c| c.space(&state.space_id).map(|s| (c.me().id.clone(), s)))
        .map(|(me, s)| {
            s.members
                .iter()
                .map(|m| Row {
                    id: m.user.id.clone(),
                    name: display_name(&m.user),
                    username: m.user.username.clone(),
                    roles: m
                        .roles
                        .iter()
                        .filter(|r| r.is_everyone != Some(true))
                        .map(|r| r.name.clone())
                        .collect(),
                    role_ids: m.roles.iter().map(|r| r.id.clone()).collect(),
                    top: m.roles.iter().map(|r| r.position).max().unwrap_or(0),
                    owner: m.user.id == s.space.owner_id,
                    me: m.user.id == me,
                })
                .collect()
        })
        .unwrap_or_default();
    ui.label(
        RichText::new("Управляйте участниками этого пространства.").color(theme::TEXT_MUTED),
    );
    ui.add_space(8.0);
    widgets::section_label(ui, &format!("Участники ({})", rows.len()));
    ui.add_space(6.0);
    status_lines(ui, state);
    let assignable: Vec<(String, String, i64)> = bridge
        .cache_at(state.origin)
        .and_then(|c| c.space(&state.space_id))
        .map(|s| {
            let mut list: Vec<_> = s
                .roles
                .iter()
                .filter(|r| r.is_everyone != Some(true))
                .map(|r| (r.id.clone(), r.name.clone(), r.position))
                .collect();
            list.sort_by_key(|(_, _, position)| std::cmp::Reverse(*position));
            list
        })
        .unwrap_or_default();
    for row in &rows {
        member_row(ui, state, rights, bridge, row, &assignable);
        ui.add_space(4.0);
    }
}

fn member_row(
    ui: &mut egui::Ui,
    state: &mut SpaceSettings,
    rights: Rights,
    bridge: &mut Bridge,
    row: &Row,
    assignable: &[(String, String, i64)],
) {
    egui::Frame::new()
        .fill(theme::BG_RAIL)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::avatar(ui, &row.name, 32.0, None, None);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&row.name).color(theme::TEXT_BRIGHT));
                    let roles = if row.owner {
                        "Владелец".to_owned()
                    } else if row.roles.is_empty() {
                        "Нет ролей".to_owned()
                    } else {
                        row.roles.join(", ")
                    };
                    ui.label(
                        RichText::new(format!("@{} · {roles}", row.username))
                            .size(12.0)
                            .color(theme::TEXT_MUTED),
                    );
                });
                if row.owner || row.me {
                    return;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if rights.ban && ui.button("Заблокировать").clicked() {
                        state.confirm = Some(Confirm::Ban(row.id.clone()));
                        state.reason.clear();
                    }
                    if rights.kick && ui.button("Исключить").clicked() {
                        state.confirm = Some(Confirm::Kick(row.id.clone()));
                    }
                    let outranks = rights.top > row.top || rights.perms.contains(Permissions::ADMINISTRATOR);
                    if rights.manage_roles && outranks {
                        ui.menu_button("Роли", |ui| {
                            member_roles_menu(ui, state, rights, bridge, row, assignable);
                        });
                    }
                });
            });
            let pending = state.confirm.clone();
            match pending {
                Some(Confirm::Kick(id)) if id == row.id => {
                    confirm_box(ui, state, bridge, row, false);
                }
                Some(Confirm::Ban(id)) if id == row.id => {
                    confirm_box(ui, state, bridge, row, true);
                }
                _ => {}
            }
        });
}

fn member_roles_menu(
    ui: &mut egui::Ui,
    state: &mut SpaceSettings,
    rights: Rights,
    bridge: &mut Bridge,
    row: &Row,
    assignable: &[(String, String, i64)],
) {
    if assignable.is_empty() {
        ui.label(RichText::new("Ролей пока нет").color(theme::TEXT_MUTED));
    }
    for (id, name, position) in assignable {
        let allowed = can_manage_role_at(rights.top, *position, rights.perms);
        let mut has = row.role_ids.contains(id);
        let was = has;
        if ui
            .add_enabled(allowed && !state.busy, egui::Checkbox::new(&mut has, name.as_str()))
            .changed()
            && has != was
        {
            let op = if has {
                SpaceOp::AddMemberRole(row.id.clone(), id.clone())
            } else {
                SpaceOp::RemoveMemberRole(row.id.clone(), id.clone())
            };
            state.request(bridge, op);
        }
    }
}

fn confirm_box(
    ui: &mut egui::Ui,
    state: &mut SpaceSettings,
    bridge: &mut Bridge,
    row: &Row,
    ban: bool,
) {
    ui.add_space(8.0);
    ui.separator();
    ui.label(
        RichText::new(if ban {
            format!("Заблокировать {}", row.name)
        } else {
            format!("Исключить {}", row.name)
        })
        .family(theme::bold_family())
        .color(theme::TEXT_BRIGHT),
    );
    ui.label(
        RichText::new(if ban {
            "Пользователь не сможет вернуться в это пространство, пока его не разблокируют."
        } else {
            "Пользователь сможет вернуться по ссылке для приглашения."
        })
        .color(theme::TEXT_MUTED),
    );
    if ban {
        ui.add_space(4.0);
        widgets::text_field(ui, &mut state.reason, "Причина (необязательно)", 512);
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let label = match (ban, state.busy) {
            (true, false) => "Заблокировать",
            (false, false) => "Исключить",
            (_, true) => "Подождите…",
        };
        let go = ui.add_enabled(
            !state.busy,
            egui::Button::new(RichText::new(label).color(theme::BG_RAIL))
                .fill(theme::ROSE)
                .corner_radius(CornerRadius::same(8)),
        );
        if go.clicked() {
            let op = if ban {
                let reason = state.reason.trim();
                SpaceOp::Ban(
                    row.id.clone(),
                    (!reason.is_empty()).then(|| reason.to_owned()),
                )
            } else {
                SpaceOp::Kick(row.id.clone())
            };
            state.request(bridge, op);
        }
        if ui.button("Отмена").clicked() {
            state.confirm = None;
        }
    });
}

fn bans(ui: &mut egui::Ui, state: &mut SpaceSettings, bridge: &mut Bridge) {
    heading(ui, "Блокировки");
    ui.label(
        RichText::new(
            "Заблокированные пользователи не могут вернуться в это пространство, пока их не разблокируют.",
        )
        .color(theme::TEXT_MUTED),
    );
    ui.add_space(8.0);
    status_lines(ui, state);
    let Some(list) = state.bans.clone() else {
        ui.label(RichText::new("Загрузка блокировок…").color(theme::TEXT_MUTED));
        return;
    };
    if list.is_empty() {
        ui.label(RichText::new("Нет заблокированных пользователей").color(theme::TEXT_MUTED));
        return;
    }
    widgets::section_label(ui, &format!("Блокировки ({})", list.len()));
    ui.add_space(6.0);
    for ban in &list {
        let name = ban
            .user
            .as_ref()
            .map_or_else(|| ban.user_id.clone(), display_name);
        egui::Frame::new()
            .fill(theme::BG_RAIL)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::avatar(ui, &name, 32.0, None, None);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&name).color(theme::TEXT_BRIGHT));
                        let by = ban.moderator.as_ref().map_or_else(
                            || "неизвестно".to_owned(),
                            display_name,
                        );
                        let text = ban
                            .reason
                            .as_deref()
                            .filter(|r| !r.is_empty())
                            .map_or_else(
                                || format!("Заблокировано пользователем {by}"),
                                |reason| format!("Заблокировано пользователем {by}: {reason}"),
                            );
                        ui.label(RichText::new(text).size(12.0).color(theme::TEXT_MUTED));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(!state.busy, egui::Button::new("Разблокировать"))
                            .clicked()
                        {
                            state.request(bridge, SpaceOp::Unban(ban.user_id.clone()));
                        }
                    });
                });
            });
        ui.add_space(4.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> SpaceSettings {
        let fields = Fields {
            name: "Игры".to_owned(),
            color: "mint".to_owned(),
            description: String::new(),
            visibility: SpaceVisibility::Private,
        };
        SpaceSettings {
            origin: 0,
            space_id: "s1".to_owned(),
            tab: Tab::Overview,
            saved: fields.clone(),
            fields,
            bans: None,
            bans_requested: false,
            confirm: Some(Confirm::Ban("u2".to_owned())),
            reason: "спам".to_owned(),
            search: String::new(),
            transfer_open: false,
            delete_armed: false,
            busy: true,
            error: None,
            notice: None,
            roles: roles::RolesState::default(),
        }
    }

    fn admin(op: SpaceOp, result: Result<(), String>, space: &str) -> SpaceEvent {
        SpaceEvent::Admin {
            space_id: space.to_owned(),
            op,
            result,
            space: None,
        }
    }

    #[test]
    fn delete_closes_and_foreign_space_is_ignored() {
        let mut s = state();
        assert!(s.apply(&admin(SpaceOp::Delete, Ok(()), "other")));
        assert!(s.busy);
        assert!(!s.apply(&admin(SpaceOp::Delete, Ok(()), "s1")));
    }

    #[test]
    fn ban_success_clears_confirm_and_error_keeps_it() {
        let mut s = state();
        assert!(s.apply(&admin(SpaceOp::Ban("u2".to_owned(), None), Err("нет".to_owned()), "s1")));
        assert_eq!(s.error.as_deref(), Some("нет"));
        assert!(s.confirm.is_some());
        assert!(s.apply(&admin(SpaceOp::Ban("u2".to_owned(), None), Ok(()), "s1")));
        assert!(s.confirm.is_none());
        assert!(s.reason.is_empty());
    }

    #[test]
    fn update_success_resets_dirty_state() {
        let mut s = state();
        s.fields.name = "Новое".to_owned();
        assert!(s.apply(&admin(SpaceOp::Update(s.update_patch()), Ok(()), "s1")));
        assert_eq!(s.saved.name, "Новое");
    }

    #[test]
    fn unban_removes_entry() {
        let mut s = state();
        s.bans = Some(vec![Ban {
            user_id: "u9".to_owned(),
            reason: None,
            user: None,
            moderator: None,
        }]);
        assert!(s.apply(&admin(SpaceOp::Unban("u9".to_owned()), Ok(()), "s1")));
        assert_eq!(s.bans.as_ref().map(Vec::len), Some(0));
    }
}
