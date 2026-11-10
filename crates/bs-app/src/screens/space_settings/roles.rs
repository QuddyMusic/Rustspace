//! Вкладка «Роли» настроек пространства: список, порядок, редактор прав.
//!
//! Тексты — из `spaces.json` (`roles.*`, `permissions.names.*`) оригинала.
//! Блокировки по правам и иерархии здесь лишь подсказка; решает сервер.

use bs_api::{NewRole, RoleSide, RoleUpdate};
use bs_permissions::{
    Permissions, can_manage_role_at, parse_permission_string, permissions_to_string,
    permissions_version,
};
use bs_types::Role;
use egui::{Color32, CornerRadius, Margin, RichText};

use super::{Rights, SpaceSettings, heading, status_lines};
use crate::bridge::SpaceOp;
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::{theme, widgets};

const NAME_MAX: usize = 100;
const DEFAULT_COLOR: &str = "#b9bbbe";
const PRESETS: [&str; 12] = [
    "#b9bbbe", "#1abc9c", "#2ecc71", "#3498db", "#9b59b6", "#e91e63", "#f1c40f", "#e67e22",
    "#e74c3c", "#95a5a6", "#607d8b", "#7289da",
];

/// Группы прав в порядке оригинала (`permissionGroups.ts`).
pub(crate) const GROUPS: [(&str, &[(Permissions, &str)]); 3] = [
    (
        "Общие",
        &[
            (Permissions::ADMINISTRATOR, "Администратор"),
            (Permissions::VIEW_CHANNEL, "Просмотр каналов"),
            (Permissions::MANAGE_CHANNELS, "Управление каналами"),
            (Permissions::MANAGE_ROLES, "Управление ролями"),
            (Permissions::MANAGE_SPACE, "Управление пространством"),
            (Permissions::CREATE_INVITE, "Создание приглашений"),
            (Permissions::KICK_MEMBERS, "Исключение участников"),
            (Permissions::BAN_MEMBERS, "Блокировка участников"),
        ],
    ),
    (
        "Текст",
        &[
            (Permissions::SEND_MESSAGES, "Отправка сообщений"),
            (Permissions::MANAGE_MESSAGES, "Управление сообщениями"),
            (Permissions::ATTACH_FILES, "Прикрепление файлов"),
            (
                Permissions::MENTION_EVERYONE,
                "Упоминать @everyone, @here и роли",
            ),
            (
                Permissions::READ_MESSAGE_HISTORY,
                "Чтение истории сообщений",
            ),
            (Permissions::ADD_REACTIONS, "Добавление реакций"),
        ],
    ),
    (
        "Голосовые каналы",
        &[
            (Permissions::CONNECT, "Подключение"),
            (Permissions::SPEAK, "Разговор"),
            (Permissions::MUTE_MEMBERS, "Отключение микрофона участникам"),
            (Permissions::DEAFEN_MEMBERS, "Отключение звука участникам"),
            (Permissions::MOVE_MEMBERS, "Перемещение участников"),
            (Permissions::DISCONNECT_MEMBERS, "Отключение участников"),
            (Permissions::STREAM, "Трансляция"),
        ],
    ),
];

/// Что редактируется сейчас.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Draft {
    role_id: String,
    name: String,
    color: String,
    perms: Permissions,
    base_name: String,
    base_color: String,
    base_perms: Permissions,
    version: String,
}

/// Состояние вкладки.
#[derive(Debug, Clone, Default)]
pub(super) struct RolesState {
    selected: Option<String>,
    draft: Option<Draft>,
    delete_armed: bool,
    /// Идентификаторы ролей до создания: новая роль откроется сама.
    select_new_after: Option<Vec<String>>,
}

impl RolesState {
    /// Сбросить черновик: он перечитается из кэша.
    pub(super) fn reload(&mut self) {
        self.draft = None;
        self.delete_armed = false;
    }

    /// Роль удалена.
    pub(super) fn closed(&mut self) {
        self.selected = None;
        self.reload();
    }

    /// Роль создана: открыть ту, которой раньше не было.
    pub(super) fn expect_new(&mut self, known: Vec<String>) {
        self.select_new_after = Some(known);
    }
}

/// Роль глазами вкладки.
fn bits_of(role: &Role) -> Permissions {
    role.permissions
        .as_deref()
        .and_then(|raw| parse_permission_string(raw).ok())
        .unwrap_or(Permissions::empty())
}

/// Цвет `#rrggbb` → `Color32`.
fn hex_color(hex: &str) -> Option<Color32> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    let [_, red, green, blue] = value.to_be_bytes();
    Some(Color32::from_rgb(red, green, blue))
}

fn valid_hex(hex: &str) -> bool {
    hex_color(hex).is_some()
}

/// Роли пространства от старшей к младшей, `@everyone` последней.
fn ordered(bridge: &Bridge, state: &SpaceSettings) -> Vec<Role> {
    let mut roles = bridge
        .cache_at(state.origin)
        .and_then(|cache| cache.space(&state.space_id))
        .map(|space| space.roles.clone())
        .unwrap_or_default();
    roles.sort_by_key(|role| {
        (
            role.is_everyone == Some(true),
            std::cmp::Reverse(role.position),
        )
    });
    roles
}

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut SpaceSettings,
    rights: Rights,
    bridge: &mut Bridge,
) {
    let roles = ordered(bridge, state);
    if let Some(known) = state.roles.select_new_after.clone()
        && let Some(fresh) = roles.iter().find(|role| !known.contains(&role.id))
    {
        state.roles.select_new_after = None;
        state.roles.selected = Some(fresh.id.clone());
        state.roles.reload();
    }
    let selected = state
        .roles
        .selected
        .as_ref()
        .and_then(|id| roles.iter().find(|role| role.id == *id))
        .cloned();
    match selected {
        Some(role) => editor(ui, state, rights, bridge, &role),
        None => list(ui, state, rights, bridge, &roles),
    }
}

fn list(
    ui: &mut egui::Ui,
    state: &mut SpaceSettings,
    rights: Rights,
    bridge: &mut Bridge,
    roles: &[Role],
) {
    heading(ui, tr("Роли"));
    ui.label(
        RichText::new(tr("Роли определяют, какие права есть у участников."))
            .color(theme::text_muted()),
    );
    ui.label(
        RichText::new(tr("Роли выше в списке стоят выше ролей под ними."))
            .color(theme::text_muted()),
    );
    ui.add_space(8.0);
    if rights.manage_roles
        && ui
            .add_enabled(!state.busy, theme::primary_button(tr("Создать роль")))
            .clicked()
    {
        let known = roles.iter().map(|role| role.id.clone()).collect();
        state.roles.expect_new(known);
        let op = SpaceOp::CreateRole(NewRole {
            name: tr("новая роль").to_owned(),
            color: DEFAULT_COLOR.to_owned(),
            permissions: None,
        });
        state.request(bridge, op);
    }
    status_lines(ui, state);
    ui.add_space(8.0);
    widgets::section_label(ui, &crate::tf!("Роли ({a0})", a0 = roles.len()));
    ui.add_space(6.0);
    for index in 0..roles.len() {
        role_row(ui, state, rights, bridge, roles, index);
        ui.add_space(4.0);
    }
}

fn role_row(
    ui: &mut egui::Ui,
    state: &mut SpaceSettings,
    rights: Rights,
    bridge: &mut Bridge,
    roles: &[Role],
    index: usize,
) {
    let role = &roles[index];
    let everyone = role.is_everyone == Some(true);
    let editable =
        rights.manage_roles && can_manage_role_at(rights.top, role.position, rights.perms);
    let mut open = false;
    let mut mv: Option<(String, RoleSide)> = None;
    egui::Frame::new()
        .fill(theme::bg_rail())
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let color = hex_color(&role.color).unwrap_or_else(theme::text_muted);
                let (dot, _) =
                    ui.allocate_exact_size(egui::Vec2::splat(14.0), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 6.0, color);
                let name = if everyone {
                    "@everyone"
                } else {
                    role.name.as_str()
                };
                if ui
                    .add(
                        egui::Label::new(RichText::new(name).color(theme::text_bright()))
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    open = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(if editable {
                            tr("Изменить")
                        } else {
                            tr("Просмотр")
                        })
                        .clicked()
                    {
                        open = true;
                    }
                    if !editable || everyone {
                        return;
                    }
                    let below = roles
                        .get(index + 1)
                        .filter(|next| next.is_everyone != Some(true));
                    if ui
                        .add_enabled(below.is_some() && !state.busy, egui::Button::new("▼"))
                        .on_hover_text(crate::tf!("Переместить «{a0}» ниже", a0 = role.name))
                        .clicked()
                        && let Some(anchor) = below
                    {
                        mv = Some((anchor.id.clone(), RoleSide::Below));
                    }
                    let above = index
                        .checked_sub(1)
                        .and_then(|i| roles.get(i))
                        .filter(|prev| can_manage_role_at(rights.top, prev.position, rights.perms));
                    if ui
                        .add_enabled(above.is_some() && !state.busy, egui::Button::new("▲"))
                        .on_hover_text(crate::tf!("Переместить «{a0}» выше", a0 = role.name))
                        .clicked()
                        && let Some(anchor) = above
                    {
                        mv = Some((anchor.id.clone(), RoleSide::Above));
                    }
                });
            });
        });
    if open {
        state.roles.selected = Some(role.id.clone());
        state.roles.reload();
        state.error = None;
        state.notice = None;
    }
    if let Some((anchor, side)) = mv {
        let op = SpaceOp::UpdateRole(role.id.clone(), RoleUpdate::moved(&anchor, side));
        state.request(bridge, op);
    }
}

fn load_draft(role: &Role) -> Draft {
    let perms = bits_of(role);
    Draft {
        role_id: role.id.clone(),
        name: role.name.clone(),
        color: role.color.clone(),
        perms,
        base_name: role.name.clone(),
        base_color: role.color.clone(),
        base_perms: perms,
        version: permissions_version(perms),
    }
}

fn editor(
    ui: &mut egui::Ui,
    state: &mut SpaceSettings,
    rights: Rights,
    bridge: &mut Bridge,
    role: &Role,
) {
    if state
        .roles
        .draft
        .as_ref()
        .is_none_or(|d| d.role_id != role.id)
    {
        state.roles.draft = Some(load_draft(role));
    }
    let everyone = role.is_everyone == Some(true);
    let editable =
        rights.manage_roles && can_manage_role_at(rights.top, role.position, rights.perms);
    if ui.button(tr("← Вернуться к ролям")).clicked() {
        state.roles.closed();
        return;
    }
    ui.add_space(8.0);
    heading(ui, if everyone { "@everyone" } else { &role.name });
    if !editable && rights.manage_roles {
        ui.label(
            RichText::new(
                tr("Эта роль находится на уровне вашей самой высокой роли или выше. Её можно просматривать, но не изменять."),
            )
            .color(theme::text_muted()),
        );
        ui.add_space(6.0);
    }
    let Some(mut draft) = state.roles.draft.clone() else {
        return;
    };
    ui.add_enabled_ui(editable, |ui| {
        if !everyone {
            widgets::section_label(ui, tr("Внешний вид"));
            widgets::field_label(ui, tr("Название роли"));
            widgets::text_field(ui, &mut draft.name, tr("Название роли"), NAME_MAX);
            widgets::field_label(ui, tr("Цвет роли"));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for preset in PRESETS {
                    let color = hex_color(preset).unwrap_or_else(theme::text_muted);
                    if widgets::color_swatch(
                        ui,
                        color,
                        draft.color.eq_ignore_ascii_case(preset),
                        30.0,
                    )
                    .clicked()
                    {
                        preset.clone_into(&mut draft.color);
                    }
                }
            });
            widgets::text_field(ui, &mut draft.color, "#rrggbb", 7);
            ui.add_space(8.0);
        }
        permission_grid(ui, &mut draft.perms, rights, editable);
    });
    state.roles.draft = Some(draft.clone());
    status_lines(ui, state);
    buttons(ui, state, rights, bridge, role, &draft, editable, everyone);
}

fn permission_grid(ui: &mut egui::Ui, perms: &mut Permissions, rights: Rights, editable: bool) {
    if editable && !rights.perms.contains(Permissions::all()) {
        ui.label(
            RichText::new(tr("Переключать можно только те права, которые есть у вас самих. Остальные заблокированы."))
                .color(theme::text_muted()),
        );
        ui.add_space(4.0);
    }
    for (title, list) in GROUPS {
        ui.add_space(6.0);
        widgets::section_label(ui, tr(title));
        for (bit, label) in list {
            let held = rights.perms.contains(*bit);
            let mut on = perms.contains(*bit);
            let changed = ui
                .horizontal(|ui| {
                    let changed = ui
                        .add_enabled_ui(held, |ui| widgets::toggle(ui, &mut on))
                        .inner;
                    let color = if held {
                        theme::text_bright()
                    } else {
                        theme::text_muted()
                    };
                    ui.label(RichText::new(tr(label)).color(color));
                    changed
                })
                .inner;
            if changed {
                perms.set(*bit, on);
            }
        }
    }
}

/// Только изменённые поля черновика роли.
fn role_update(draft: &Draft, everyone: bool) -> RoleUpdate {
    let mut update = RoleUpdate::default();
    if !everyone && draft.name.trim() != draft.base_name {
        update.name = Some(draft.name.trim().to_owned());
    }
    if !everyone && draft.color != draft.base_color {
        update.color = Some(draft.color.clone());
    }
    if draft.perms != draft.base_perms {
        update.permissions = Some(permissions_to_string(draft.perms));
        update.permissions_version = Some(draft.version.clone());
    }
    update
}

#[allow(clippy::too_many_arguments, reason = "кнопки редактора роли")]
fn buttons(
    ui: &mut egui::Ui,
    state: &mut SpaceSettings,
    rights: Rights,
    bridge: &mut Bridge,
    role: &Role,
    draft: &Draft,
    editable: bool,
    everyone: bool,
) {
    ui.add_space(12.0);
    let dirty = draft.name != draft.base_name
        || draft.color != draft.base_color
        || draft.perms != draft.base_perms;
    if editable && dirty {
        let name = draft.name.trim();
        let duplicate = bridge
            .cache_at(state.origin)
            .and_then(|c| c.space(&state.space_id))
            .is_some_and(|s| {
                s.roles
                    .iter()
                    .any(|r| r.id != role.id && r.name.eq_ignore_ascii_case(name))
            });
        let invalid = (!everyone && (name.is_empty() || !valid_hex(&draft.color))) || duplicate;
        if name.is_empty() && !everyone {
            ui.label(RichText::new(tr("Название роли не может быть пустым")).color(theme::rose()));
        } else if duplicate {
            ui.label(
                RichText::new(tr("Роль с таким названием уже существует")).color(theme::rose()),
            );
        }
        ui.horizontal(|ui| {
            let label = if state.busy {
                tr("Сохранение…")
            } else {
                tr("Сохранить")
            };
            if ui
                .add_enabled(!invalid && !state.busy, theme::primary_button(label))
                .clicked()
            {
                let update = role_update(draft, everyone);
                state.request(bridge, SpaceOp::UpdateRole(role.id.clone(), update));
            }
            if ui.button(tr("Отменить изменения")).clicked() {
                state.roles.draft = Some(load_draft(role));
            }
        });
    }
    if !editable || everyone {
        return;
    }
    ui.add_space(14.0);
    let foreign = bits_of(role) & !rights.perms;
    ui.horizontal(|ui| {
        let copy = ui.add_enabled(
            foreign.is_empty() && !state.busy,
            egui::Button::new(tr("Копировать роль")),
        );
        let copy = if foreign.is_empty() {
            copy
        } else {
            copy.on_disabled_hover_text(tr(
                "У этой роли есть права, которых нет у вас, поэтому её нельзя скопировать.",
            ))
        };
        if copy.clicked() {
            let known = bridge
                .cache_at(state.origin)
                .and_then(|c| c.space(&state.space_id))
                .map(|s| s.roles.iter().map(|r| r.id.clone()).collect())
                .unwrap_or_default();
            state.roles.expect_new(known);
            let op = SpaceOp::CreateRole(NewRole {
                name: crate::tf!("Копия {a0}", a0 = role.name),
                color: role.color.clone(),
                permissions: Some(permissions_to_string(bits_of(role))),
            });
            state.request(bridge, op);
            state.roles.closed();
        }
        let label = if state.roles.delete_armed {
            tr("Подтвердить?")
        } else {
            tr("Удалить роль")
        };
        let delete = ui.add_enabled(
            foreign.is_empty() && !state.busy,
            egui::Button::new(RichText::new(label).color(theme::bg_rail()))
                .fill(theme::rose())
                .corner_radius(CornerRadius::same(8)),
        );
        if delete.clicked() {
            if state.roles.delete_armed {
                state.request(bridge, SpaceOp::DeleteRole(role.id.clone()));
            } else {
                state.roles.delete_armed = true;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(perms: &str) -> Role {
        Role {
            id: "r1".to_owned(),
            space_id: "s1".to_owned(),
            name: "Модератор".to_owned(),
            color: "#3498db".to_owned(),
            position: 1,
            permissions: Some(perms.to_owned()),
            is_everyone: Some(false),
            created_at: 1,
        }
    }

    #[test]
    fn hex_colors_parse_strictly() {
        assert_eq!(
            hex_color("#3498db"),
            Some(Color32::from_rgb(0x34, 0x98, 0xdb))
        );
        assert_eq!(hex_color("3498db"), None);
        assert_eq!(hex_color("#34"), None);
        assert_eq!(hex_color("#zzzzzz"), None);
        assert!(valid_hex(DEFAULT_COLOR));
    }

    #[test]
    fn draft_starts_clean_with_version_of_loaded_bits() {
        let draft = load_draft(&role("1088"));
        assert_eq!(
            draft.perms,
            Permissions::KICK_MEMBERS | Permissions::SEND_MESSAGES
        );
        assert_eq!(draft.perms, draft.base_perms);
        assert_eq!(draft.version, permissions_version(draft.perms));
    }

    #[test]
    fn unreadable_permissions_are_empty() {
        let mut hidden = role("1088");
        hidden.permissions = None;
        assert_eq!(bits_of(&hidden), Permissions::empty());
    }

    #[test]
    fn groups_cover_every_permission_once() {
        let mut all = Permissions::empty();
        for (_, list) in GROUPS {
            for (bit, _) in list {
                assert!(!all.intersects(*bit), "повтор права");
                all |= *bit;
            }
        }
        assert_eq!(all, Permissions::all());
    }
}
