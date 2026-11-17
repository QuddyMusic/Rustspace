//! Настройки пространства: «Основное», «Обнаружение», «Участники», «Роли», «Блокировки».
//!
//! Тексты — как в `spaceSettingsPanels` и `TransferOwnershipModal`
//! оригинального веб-клиента. Права лишь решают, что показать; решает сервер.

mod discovery;
mod roles;

pub(crate) use roles::GROUPS as PERMISSION_GROUPS;

use bs_api::{Ban, SpaceUpdate};
use bs_permissions::{
    Permissions, can_manage_role_at, has_permission_bit, parse_permission_string,
};
use bs_types::SpaceVisibility;
use egui::{Align, Layout, Margin, RichText};

use crate::bridge::{BridgeRequest, SpaceEvent, SpaceOp};
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::{theme, widgets};

const NAME_MAX: usize = 100;
const DESCRIPTION_MAX: usize = 200;

/// Вкладка окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Discovery,
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
    /// Имя файла значка на сервере (пусто — нет).
    icon: String,
    /// Имя файла баннера на сервере (пусто — нет).
    banner: String,
    directory_listed: bool,
}

/// Состояние окна настроек пространства.
#[derive(Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "независимые флаги интерфейса"
)]
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
    requests: Option<Vec<bs_api::JoinRequest>>,
    requests_requested: bool,
    picking: Option<discovery::Picking>,
    /// Что сейчас загружается: `false` — значок, `true` — баннер.
    uploading: Option<bool>,
}

/// Права текущего пользователя в пространстве.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::struct_excessive_bools, reason = "независимые права")]
pub(crate) struct Rights {
    pub(crate) owner: bool,
    pub(crate) manage_space: bool,
    pub(crate) kick: bool,
    pub(crate) ban: bool,
    pub(crate) manage_roles: bool,
    /// Все права текущего пользователя (владелец — все).
    pub(crate) perms: Permissions,
    /// Место в иерархии ролей: по нему сервер решает, кем и какими ролями
    /// можно управлять.
    pub(crate) standing: bs_permissions::Standing,
}

impl SpaceSettings {
    /// Открывает окно для пространства из кэша `origin`.
    pub(crate) fn open(bridge: &Bridge, origin: usize, space_id: &str) -> Option<Self> {
        let space = bridge.cache_at(origin)?.space(space_id)?;
        let fields = Fields {
            name: space.space.name.clone(),
            color: space
                .space
                .avatar_color
                .clone()
                .unwrap_or_else(|| "mint".to_owned()),
            description: space.space.description.clone().unwrap_or_default(),
            visibility: space.space.visibility,
            icon: space.space.icon.clone().unwrap_or_default(),
            banner: space.space.banner.clone().unwrap_or_default(),
            directory_listed: space.space.directory_listed,
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
            requests: None,
            requests_requested: false,
            picking: None,
            uploading: None,
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
                return self.apply_admin(op, result);
            }
            SpaceEvent::Uploaded {
                space_id,
                banner,
                result,
            } if *space_id == self.space_id => self.apply_upload(*banner, result),
            SpaceEvent::Requests { space_id, result } if *space_id == self.space_id => match result
            {
                Ok(list) => self.requests = Some(list.clone()),
                Err(err) => {
                    self.requests = Some(Vec::new());
                    self.error = Some(err.clone());
                }
            },
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

    /// Итог административного действия. `false` — пространство удалено.
    fn apply_admin(&mut self, op: &SpaceOp, result: &Result<(), String>) -> bool {
        match (op, result) {
            (SpaceOp::Delete, Ok(())) => return false,
            (op, Err(err)) => {
                if matches!(op, SpaceOp::UpdateRole(_, _)) && err.starts_with(tr("Кто-то другой"))
                {
                    self.roles.reload();
                }
                self.error = Some(err.clone());
            }
            (SpaceOp::DecideRequest(id, _), Ok(())) => {
                if let Some(list) = self.requests.as_mut() {
                    list.retain(|r| r.id != *id);
                }
            }
            (SpaceOp::Update(_), Ok(())) => {
                self.saved = self.fields.clone();
                self.notice = Some(tr("Настройки сохранены").to_owned());
            }
            (SpaceOp::Transfer(_), Ok(())) => {
                self.transfer_open = false;
                self.confirm = None;
                self.notice = Some(tr("Права владельца переданы").to_owned());
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
                self.notice = Some(tr("Роль сохранена").to_owned());
            }
            (SpaceOp::DeleteRole(_), Ok(())) => self.roles.closed(),
            (SpaceOp::CreateRole(_), Ok(())) => {
                self.notice = Some(tr("Роль создана. Настройте её ниже.").to_owned());
            }
            (
                SpaceOp::ListBans
                | SpaceOp::ListJoinRequests
                | SpaceOp::UploadImage(..)
                | SpaceOp::AddMemberRole(_, _)
                | SpaceOp::RemoveMemberRole(_, _)
                | SpaceOp::Notify(..)
                | SpaceOp::UpdateChannel(..)
                | SpaceOp::DeleteChannel(_)
                | SpaceOp::RenameCategory(..)
                | SpaceOp::DeleteCategory(_)
                | SpaceOp::ListOverrides(_)
                | SpaceOp::PutOverride(..)
                | SpaceOp::DeleteOverride(..)
                | SpaceOp::ListBridges(_)
                | SpaceOp::AddBridge(..)
                | SpaceOp::RemoveBridge(_)
                | SpaceOp::SetNsfw(..)
                | SpaceOp::ListWebhooks(_)
                | SpaceOp::CreateWebhook(..)
                | SpaceOp::DeleteWebhook(_),
                Ok(()),
            ) => {}
        }
        true
    }

    /// Загружен (или нет) значок либо баннер.
    fn apply_upload(&mut self, banner: bool, result: &Result<String, String>) {
        self.busy = false;
        self.uploading = None;
        match result {
            Ok(file) if banner => self.fields.banner.clone_from(file),
            Ok(file) => self.fields.icon.clone_from(file),
            Err(_) => {
                self.error = Some(if banner {
                    tr("Не удалось загрузить баннер").to_owned()
                } else {
                    tr("Не удалось загрузить значок").to_owned()
                });
            }
        }
    }

    fn rights(&self, bridge: &Bridge) -> Option<Rights> {
        rights_for(bridge, self.origin, &self.space_id)
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
            icon: (self.fields.icon != self.saved.icon).then(|| self.fields.icon.clone()),
            banner: (self.fields.banner != self.saved.banner).then(|| self.fields.banner.clone()),
            directory_listed: Some(self.fields.directory_listed),
        }
    }
}

/// Права текущего пользователя в пространстве.
pub(crate) fn rights_for(bridge: &Bridge, origin: usize, space_id: &str) -> Option<Rights> {
    let cache = bridge.cache_at(origin)?;
    let space = cache.space(space_id)?;
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
        standing: bs_permissions::Standing {
            is_owner: owner,
            is_instance_admin: cache.me().is_admin,
            top_position: space
                .members
                .iter()
                .find(|m| m.user.id == cache.me().id)
                .and_then(|m| {
                    m.roles
                        .iter()
                        .filter(|r| r.is_everyone != Some(true))
                        .map(|r| r.position)
                        .max()
                })
                .unwrap_or(0),
        },
    })
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
    if state.tab == Tab::Discovery
        && !state.requests_requested
        && rights.manage_space
        && state.fields.visibility == SpaceVisibility::Request
    {
        state.requests_requested = true;
        bridge.focus_on(state.origin);
        bridge.request(BridgeRequest::SpaceAdmin {
            space_id: state.space_id.clone(),
            op: SpaceOp::ListJoinRequests,
        });
    }
    discovery::drain_pick(state, bridge);
    let mut keep_open = true;
    let response = egui::Modal::new(egui::Id::unique("space_settings"))
        .frame(
            egui::Frame::new()
                .fill(theme::bg_bubble())
                .stroke(egui::Stroke::new(1.0, theme::border()))
                .corner_radius(crate::theme::rounded(16))
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
                                Tab::Discovery => discovery::show(ui, state, rights, bridge),
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
            widgets::section_label(ui, tr("Настройки пространства"));
        });
        ui.add_space(8.0);
        let mut tabs = vec![(Tab::Overview, tr("Основное"), Icon::Gear)];
        if rights.manage_space {
            tabs.push((Tab::Discovery, tr("Обнаружение"), Icon::Compass));
        }
        tabs.push((Tab::Members, tr("Участники"), Icon::Users));
        if rights.manage_roles {
            tabs.push((Tab::Roles, tr("Роли"), Icon::Gear));
        }
        if rights.ban {
            tabs.push((Tab::Bans, tr("Блокировки"), Icon::User));
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
            if ui.button(tr("Закрыть")).clicked() {
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
            .color(theme::text_bright()),
    );
    ui.add_space(8.0);
}

fn status_lines(ui: &mut egui::Ui, state: &SpaceSettings) {
    if let Some(err) = &state.error {
        ui.add_space(6.0);
        ui.label(RichText::new(err).color(theme::rose()));
    }
    if let Some(note) = &state.notice {
        ui.add_space(6.0);
        ui.label(RichText::new(note).color(theme::text_muted()));
    }
}

fn overview(ui: &mut egui::Ui, state: &mut SpaceSettings, rights: Rights, bridge: &mut Bridge) {
    heading(ui, tr("Основное"));
    discovery::appearance(ui, state, rights, bridge);
    ui.add_enabled_ui(rights.manage_space, |ui| {
        widgets::field_label(ui, tr("Цвет значка"));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for name in theme::PALETTE_NAMES {
                let color = theme::palette(name).unwrap_or_else(theme::accent);
                if widgets::color_swatch(ui, color, state.fields.color == name, 30.0).clicked() {
                    name.clone_into(&mut state.fields.color);
                }
            }
        });
        widgets::field_label(ui, tr("Название пространства"));
        widgets::text_field(ui, &mut state.fields.name, tr("Название"), NAME_MAX);
    });
    status_lines(ui, state);
    discovery::save_bar(ui, state, rights, bridge);
    if rights.owner {
        danger_zone(ui, state, bridge);
    }
}

fn danger_zone(ui: &mut egui::Ui, state: &mut SpaceSettings, bridge: &mut Bridge) {
    ui.add_space(22.0);
    ui.label(
        RichText::new(tr("ОПАСНАЯ ЗОНА"))
            .size(11.0)
            .family(theme::bold_family())
            .color(theme::rose()),
    );
    ui.add_space(6.0);
    ui.label(
        RichText::new(tr(
            "Передать права владельца другому участнику. Вы станете обычным участником.",
        ))
        .color(theme::text_muted()),
    );
    ui.add_space(6.0);
    if state.transfer_open {
        transfer_picker(ui, state, bridge);
    } else if ui.button(tr("Передать права владельца")).clicked() {
        state.transfer_open = true;
        state.confirm = None;
    }
    ui.add_space(14.0);
    ui.label(
        RichText::new(tr(
            "Удалить это пространство и все его данные навсегда. Это нельзя отменить.",
        ))
        .color(theme::text_muted()),
    );
    ui.add_space(6.0);
    let label = if state.delete_armed {
        tr("Нажмите ещё раз, чтобы подтвердить удаление")
    } else {
        tr("Удалить пространство")
    };
    let delete = ui.add_enabled(
        !state.busy,
        egui::Button::new(RichText::new(label).color(theme::bg_rail()))
            .fill(theme::rose())
            .corner_radius(crate::theme::rounded(8)),
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
        ui.label(crate::tf!("Передать права владельца «{space_name}» пользователю {member}? Вы станете обычным участником.", space_name = space_name, member = member));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !state.busy,
                    theme::primary_button(if state.busy {
                        tr("Передаём права…")
                    } else {
                        tr("Передать")
                    }),
                )
                .clicked()
            {
                state.request(bridge, SpaceOp::Transfer(target));
            }
            if ui.button(tr("Назад")).clicked() {
                state.confirm = None;
            }
        });
        return;
    }
    widgets::text_field(ui, &mut state.search, tr("Поиск участников…"), 64);
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
        ui.label(RichText::new(tr("Участники не найдены")).color(theme::text_muted()));
    }
    for (id, name) in candidates {
        if ui.button(name).clicked() {
            state.confirm = Some(Confirm::Transfer(id));
        }
    }
    if ui.button(tr("Отмена")).clicked() {
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
    /// Аватар: картинка, ключ цвета заглушки и выбранный цвет.
    avatar: Option<String>,
    face_key: String,
    color: Option<String>,
}

fn members(ui: &mut egui::Ui, state: &mut SpaceSettings, rights: Rights, bridge: &mut Bridge) {
    heading(ui, tr("Участники"));
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
                    avatar: m.user.avatar.clone(),
                    face_key: m
                        .user
                        .home_user_id
                        .clone()
                        .unwrap_or_else(|| m.user.id.clone()),
                    color: m.user.avatar_color.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    ui.label(
        RichText::new(tr("Управляйте участниками этого пространства.")).color(theme::text_muted()),
    );
    ui.add_space(8.0);
    widgets::section_label(ui, &crate::tf!("Участники ({a0})", a0 = rows.len()));
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
        .fill(theme::bg_rail())
        .corner_radius(crate::theme::rounded(10))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::avatar(
                    ui,
                    theme::Face {
                        name: &row.name,
                        key: Some(&row.face_key),
                        color: row.color.as_deref(),
                    },
                    32.0,
                    None,
                    row.avatar.as_deref(),
                );
                ui.vertical(|ui| {
                    ui.label(RichText::new(&row.name).color(theme::text_bright()));
                    let roles = if row.owner {
                        tr("Владелец").to_owned()
                    } else if row.roles.is_empty() {
                        tr("Нет ролей").to_owned()
                    } else {
                        row.roles.join(", ")
                    };
                    ui.label(
                        RichText::new(format!("@{} · {roles}", row.username))
                            .size(12.0)
                            .color(theme::text_muted()),
                    );
                });
                if row.owner || row.me {
                    return;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if rights.ban && ui.button(tr("Заблокировать")).clicked() {
                        state.confirm = Some(Confirm::Ban(row.id.clone()));
                        state.reason.clear();
                    }
                    if rights.kick && ui.button(tr("Исключить")).clicked() {
                        state.confirm = Some(Confirm::Kick(row.id.clone()));
                    }
                    let outranks = bs_permissions::can_act_on_member(
                        rights.standing,
                        bs_permissions::Standing {
                            is_owner: row.owner,
                            is_instance_admin: false,
                            top_position: row.top,
                        },
                    );
                    if rights.manage_roles && outranks {
                        ui.menu_button(tr("Роли"), |ui| {
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
        ui.label(RichText::new(tr("Ролей пока нет")).color(theme::text_muted()));
    }
    for (id, name, position) in assignable {
        let allowed = can_manage_role_at(rights.standing, *position);
        let mut has = row.role_ids.contains(id);
        let was = has;
        if ui
            .add_enabled(
                allowed && !state.busy,
                egui::Checkbox::new(&mut has, name.as_str()),
            )
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
            crate::tf!("Заблокировать {a0}", a0 = row.name)
        } else {
            crate::tf!("Исключить {a0}", a0 = row.name)
        })
        .family(theme::bold_family())
        .color(theme::text_bright()),
    );
    ui.label(
        RichText::new(if ban {
            tr("Пользователь не сможет вернуться в это пространство, пока его не разблокируют.")
        } else {
            tr("Пользователь сможет вернуться по ссылке для приглашения.")
        })
        .color(theme::text_muted()),
    );
    if ban {
        ui.add_space(4.0);
        widgets::text_field(ui, &mut state.reason, tr("Причина (необязательно)"), 512);
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let label = match (ban, state.busy) {
            (true, false) => tr("Заблокировать"),
            (false, false) => tr("Исключить"),
            (_, true) => tr("Подождите…"),
        };
        let go = ui.add_enabled(
            !state.busy,
            egui::Button::new(RichText::new(label).color(theme::bg_rail()))
                .fill(theme::rose())
                .corner_radius(crate::theme::rounded(8)),
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
        if ui.button(tr("Отмена")).clicked() {
            state.confirm = None;
        }
    });
}

fn bans(ui: &mut egui::Ui, state: &mut SpaceSettings, bridge: &mut Bridge) {
    heading(ui, tr("Блокировки"));
    ui.label(
        RichText::new(
            tr("Заблокированные пользователи не могут вернуться в это пространство, пока их не разблокируют."),
        )
        .color(theme::text_muted()),
    );
    ui.add_space(8.0);
    status_lines(ui, state);
    let Some(list) = state.bans.clone() else {
        ui.label(RichText::new(tr("Загрузка блокировок…")).color(theme::text_muted()));
        return;
    };
    if list.is_empty() {
        ui.label(RichText::new(tr("Нет заблокированных пользователей")).color(theme::text_muted()));
        return;
    }
    widgets::section_label(ui, &crate::tf!("Блокировки ({a0})", a0 = list.len()));
    ui.add_space(6.0);
    for ban in &list {
        let name = ban
            .user
            .as_ref()
            .map_or_else(|| ban.user_id.clone(), display_name);
        egui::Frame::new()
            .fill(theme::bg_rail())
            .corner_radius(crate::theme::rounded(10))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::avatar(ui, &name, 32.0, None, None);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&name).color(theme::text_bright()));
                        let by = ban
                            .moderator
                            .as_ref()
                            .map_or_else(|| tr("неизвестно").to_owned(), display_name);
                        let text = ban.reason.as_deref().filter(|r| !r.is_empty()).map_or_else(
                            || crate::tf!("Заблокировано пользователем {by}", by = by),
                            |reason| {
                                crate::tf!(
                                    "Заблокировано пользователем {by}: {reason}",
                                    by = by,
                                    reason = reason
                                )
                            },
                        );
                        ui.label(RichText::new(text).size(12.0).color(theme::text_muted()));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(!state.busy, egui::Button::new(tr("Разблокировать")))
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
            icon: String::new(),
            banner: String::new(),
            directory_listed: false,
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
            requests: None,
            requests_requested: false,
            picking: None,
            uploading: None,
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
        assert!(s.apply(&admin(
            SpaceOp::Ban("u2".to_owned(), None),
            Err("нет".to_owned()),
            "s1"
        )));
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
    fn patch_carries_images_only_when_changed() {
        let mut s = state();
        assert_eq!(s.update_patch().icon, None);
        assert_eq!(s.update_patch().banner, None);
        s.fields.icon = "new.webp".to_owned();
        s.fields.banner.clear();
        s.saved.banner = "old.webp".to_owned();
        let patch = s.update_patch();
        assert_eq!(patch.icon.as_deref(), Some("new.webp"));
        assert_eq!(patch.banner.as_deref(), Some(""));
    }

    #[test]
    fn upload_result_fills_the_form_or_reports_failure() {
        let mut s = state();
        s.uploading = Some(true);
        let done = |result| SpaceEvent::Uploaded {
            space_id: "s1".to_owned(),
            banner: true,
            result,
        };
        assert!(s.apply(&done(Err("x".to_owned()))));
        assert_eq!(s.error.as_deref(), Some("Не удалось загрузить баннер"));
        assert!(s.uploading.is_none());
        assert!(s.apply(&done(Ok("b.webp".to_owned()))));
        assert_eq!(s.fields.banner, "b.webp");
    }

    #[test]
    fn deciding_a_request_drops_it_from_the_list() {
        let mut s = state();
        s.requests = Some(Vec::new());
        assert!(s.apply(&admin(
            SpaceOp::DecideRequest("j1".to_owned(), true),
            Ok(()),
            "s1"
        )));
        assert_eq!(s.requests.as_ref().map(Vec::len), Some(0));
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
