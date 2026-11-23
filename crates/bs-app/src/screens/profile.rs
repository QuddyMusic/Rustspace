//! Окно профиля другого пользователя.

use bs_api::Mutuals;
use bs_types::User;
use egui::{Color32, CornerRadius, RichText, Sense, Vec2};

use crate::bridge::BridgeRequest;
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::{social, theme};

/// Ширина окна.
const WIDTH: f32 = 380.0;
/// Высота баннера.
const BANNER: f32 = 110.0;

/// Где показать окно профиля.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Anchor {
    /// У правого края, вплотную к списку участников.
    Panel,
    /// Рядом с тем, по чему щёлкнули (аватар или имя в ленте).
    Beside(egui::Rect),
}

/// Что открыто в окне.
#[derive(Debug)]
pub(crate) struct ProfileState {
    user_id: String,
    anchor: Anchor,
    /// Пространство, из которого смотрят профиль: показать роли в нём.
    space: Option<(usize, String)>,
    name: String,
    user: Option<Box<User>>,
    error: Option<String>,
    mutuals: Option<Mutuals>,
}

/// Результат кадра.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProfileAction {
    /// Ничего.
    None,
    /// Закрыть окно.
    Close,
}

impl ProfileState {
    /// Открывает профиль и запрашивает данные у сервера.
    pub(crate) fn open(bridge: &Bridge, user_id: &str, name: &str) -> Self {
        Self::open_at(bridge, user_id, name, Anchor::Panel)
    }

    /// Открывает профиль у заданного места.
    pub(crate) fn open_at(bridge: &Bridge, user_id: &str, name: &str, anchor: Anchor) -> Self {
        bridge.request(BridgeRequest::Profile(user_id.to_owned()));
        Self {
            user_id: user_id.to_owned(),
            anchor,
            space: None,
            name: name.to_owned(),
            user: None,
            error: None,
            mutuals: None,
        }
    }

    /// Пространство, в котором показать роли пользователя.
    pub(crate) fn set_space(&mut self, space: Option<(usize, String)>) {
        self.space = space;
    }

    /// Открыт ли профиль этого пользователя (повторный щелчок закрывает).
    pub(crate) fn is_of(&self, user_id: &str) -> bool {
        self.user_id == user_id
    }

    /// Применяет пришедшие ответы.
    pub(crate) fn poll(&mut self, bridge: &mut Bridge) {
        for event in bridge.take_profile_events() {
            if event.user_id != self.user_id {
                continue;
            }
            match event.user {
                Ok(user) => {
                    self.user = Some(user);
                    self.error = None;
                }
                Err(err) => self.error = Some(err),
            }
            self.mutuals = event.mutuals;
        }
    }
}

/// Разбирает `#rrggbb`.
fn parse_hex(value: &str) -> Option<Color32> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    let [_, r, g, b] = n.to_be_bytes();
    Some(Color32::from_rgb(r, g, b))
}

fn date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%d.%m.%Y")
                .to_string()
        })
        .unwrap_or_default()
}

fn header(ui: &mut egui::Ui, state: &ProfileState) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(WIDTH, BANNER), Sense::hover());
    let user = state.user.as_deref();
    let accent = user
        .and_then(|u| u.accent_color.as_deref())
        .and_then(parse_hex)
        .unwrap_or_else(|| theme::color_for(&state.name));
    let round = CornerRadius {
        nw: 12,
        ne: 12,
        sw: 0,
        se: 0,
    };
    ui.painter()
        .rect_filled(rect, round, accent.gamma_multiply(0.7));
    if let Some(banner) = user.and_then(|u| u.banner.as_deref()) {
        crate::images::paint_value(ui.painter(), rect, banner, round, true, Color32::WHITE);
    }
    let center = rect.left_bottom() + Vec2::new(54.0, 0.0);
    ui.painter()
        .circle_filled(center, 46.0, theme::bg_sidebar());
    let avatar = egui::Rect::from_center_size(center, Vec2::splat(80.0));
    theme::paint_avatar(
        ui.painter(),
        avatar,
        theme::Face {
            name: &state.name,
            key: user.map(|u| u.home_user_id.as_deref().unwrap_or(&u.id)),
            color: user.and_then(|u| u.avatar_color.as_deref()),
        },
        user.map(|u| u.status),
        false,
        user.and_then(|u| u.avatar.as_deref()),
    );
}

/// Роль в профиле: id, имя, цвет и позиция.
#[derive(Debug, Clone)]
struct RoleChip {
    id: String,
    name: String,
    color: Color32,
    position: i64,
}

/// Роли пользователя в пространстве, откуда открыт профиль (от старшей), и
/// все роли пространства — для выбора.
fn space_roles(bridge: &Bridge, state: &ProfileState) -> (Vec<RoleChip>, Vec<RoleChip>) {
    let Some((origin, space_id)) = &state.space else {
        return (Vec::new(), Vec::new());
    };
    let Some(space) = bridge.cache_at(*origin).and_then(|c| c.space(space_id)) else {
        return (Vec::new(), Vec::new());
    };
    let chip = |r: &bs_types::Role| RoleChip {
        id: r.id.clone(),
        name: r.name.clone(),
        color: role_color(&r.color),
        position: r.position,
    };
    let mut all: Vec<RoleChip> = space
        .roles
        .iter()
        .filter(|r| r.is_everyone != Some(true))
        .map(chip)
        .collect();
    all.sort_by_key(|r| std::cmp::Reverse(r.position));
    let mut mine: Vec<RoleChip> = space
        .members
        .iter()
        .find(|m| m.user.id == state.user_id)
        .map(|member| {
            member
                .roles
                .iter()
                .filter(|r| r.is_everyone != Some(true))
                .map(chip)
                .collect()
        })
        .unwrap_or_default();
    mine.sort_by_key(|r| std::cmp::Reverse(r.position));
    (mine, all)
}

/// Что можно делать с ролями в этом профиле.
struct RoleEditor<'a> {
    bridge: &'a Bridge,
    origin: usize,
    space_id: &'a str,
    user_id: &'a str,
    standing: bs_permissions::Standing,
    all: &'a [RoleChip],
}

impl RoleEditor<'_> {
    fn allowed(&self, position: i64) -> bool {
        bs_permissions::can_manage_role_at(self.standing, position)
    }

    fn set(&self, role: &str, on: bool) {
        let op = if on {
            crate::bridge::SpaceOp::AddMemberRole(self.user_id.to_owned(), role.to_owned())
        } else {
            crate::bridge::SpaceOp::RemoveMemberRole(self.user_id.to_owned(), role.to_owned())
        };
        if let Some(session) = self.bridge.session_at(self.origin) {
            session.request(BridgeRequest::SpaceAdmin {
                space_id: self.space_id.to_owned(),
                op,
            });
        }
    }
}

/// Редактор ролей, если у открывшего есть право «Управлять ролями».
fn role_editor<'a>(
    bridge: &'a Bridge,
    state: &'a ProfileState,
    all: &'a [RoleChip],
) -> Option<RoleEditor<'a>> {
    let (origin, space_id) = state.space.as_ref()?;
    let rights = super::space_settings::rights_for(bridge, *origin, space_id)?;
    rights.manage_roles.then_some(RoleEditor {
        bridge,
        origin: *origin,
        space_id,
        user_id: &state.user_id,
        standing: rights.standing,
        all,
    })
}

/// Цвет роли `#rrggbb`; без цвета — приглушённый.
fn role_color(hex: &str) -> Color32 {
    let digits = hex.trim().trim_start_matches('#');
    match u32::from_str_radix(digits, 16) {
        Ok(value) if digits.len() == 6 => {
            let [_, red, green, blue] = value.to_be_bytes();
            Color32::from_rgb(red, green, blue)
        }
        _ => theme::text_muted(),
    }
}

/// Роли плашками с цветной точкой; с правом «Управлять ролями» — крестик
/// у роли и «+» со списком ролей пространства.
fn roles_section(ui: &mut egui::Ui, roles: &[RoleChip], editor: Option<&RoleEditor<'_>>) {
    if roles.is_empty() && editor.is_none() {
        return;
    }
    ui.add_space(10.0);
    ui.label(theme::section_title(&crate::tf!(
        "РОЛИ — {a0}",
        a0 = roles.len()
    )));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
        for role in roles {
            egui::Frame::new()
                .fill(theme::bg_hover())
                .corner_radius(theme::rounded(6))
                .inner_margin(egui::Margin::symmetric(8, 3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let (dot, _) =
                            ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 4.5, role.color);
                        ui.label(RichText::new(&role.name).size(12.5).color(theme::text()));
                        if let Some(editor) = editor
                            && editor.allowed(role.position)
                            && ui
                                .add(crate::widgets::flat_icon(
                                    crate::icons::Icon::Close,
                                    10.0,
                                    theme::text_faint(),
                                ))
                                .on_hover_text(tr("Снять роль"))
                                .clicked()
                        {
                            editor.set(&role.id, false);
                        }
                    });
                });
        }
        if let Some(editor) = editor {
            let add = ui
                .add(egui::Button::new(RichText::new("+").size(13.0)).small())
                .on_hover_text(tr("Выдать роль"));
            egui::Popup::menu(&add).width(220.0).show(|ui| {
                let free: Vec<&RoleChip> = editor
                    .all
                    .iter()
                    .filter(|r| !roles.iter().any(|mine| mine.id == r.id))
                    .collect();
                if free.is_empty() {
                    ui.label(theme::muted(tr("Все роли уже выданы")));
                }
                for role in free {
                    let allowed = editor.allowed(role.position);
                    let label = RichText::new(&role.name).color(if allowed {
                        role.color
                    } else {
                        theme::text_faint()
                    });
                    if ui
                        .add_enabled(allowed, egui::Button::new(label).frame(false))
                        .clicked()
                    {
                        editor.set(&role.id, true);
                        ui.close();
                    }
                }
            });
        }
    });
}

fn body(
    ui: &mut egui::Ui,
    state: &ProfileState,
    activities: &[bs_types::Activity],
    roles: &[RoleChip],
    editor: Option<&RoleEditor<'_>>,
) {
    let Some(user) = state.user.as_deref() else {
        if let Some(error) = &state.error {
            ui.colored_label(theme::red(), error);
        } else {
            ui.label(theme::muted(tr("Загрузка…")));
        }
        return;
    };
    let shown = user.display_name.as_deref().unwrap_or(&user.username);
    ui.label(
        RichText::new(shown)
            .family(theme::bold_family())
            .size(22.0)
            .color(theme::text_bright()),
    );
    ui.label(theme::muted(format!(
        "@{} · {}",
        user.username,
        social::status_label(user.status)
    )));
    if let Some(custom) = user.custom_status.as_deref().filter(|s| !s.is_empty()) {
        ui.label(RichText::new(custom).color(theme::text()));
    }
    if let Some(home) = user.home_instance.as_deref() {
        ui.label(theme::muted(crate::tf!("Домашний инстанс: {home}", home = home)).size(12.5));
    }
    super::activity_view::section(ui, &user.id, activities);
    roles_section(ui, roles, editor);
    if let Some(bio) = user.bio.as_deref().filter(|b| !b.trim().is_empty()) {
        ui.add_space(10.0);
        ui.label(theme::section_title(tr("О СЕБЕ")));
        ui.label(RichText::new(bio).color(theme::text()));
    }
    ui.add_space(10.0);
    ui.label(theme::section_title(tr("НА ПЛАТФОРМЕ С")));
    ui.label(RichText::new(date(user.created_at)).color(theme::text()));
    if let Some(mutuals) = &state.mutuals {
        if !mutuals.mutual_friends.is_empty() {
            ui.add_space(10.0);
            ui.label(theme::section_title(&crate::tf!(
                "ОБЩИЕ ДРУЗЬЯ — {a0}",
                a0 = mutuals.mutual_friends.len()
            )));
            for friend in &mutuals.mutual_friends {
                ui.horizontal(|ui| {
                    theme::avatar(
                        ui,
                        friend.display_name.as_deref().unwrap_or(&friend.username),
                        24.0,
                        None,
                        friend.avatar.as_deref(),
                    );
                    ui.label(friend.display_name.as_deref().unwrap_or(&friend.username));
                });
            }
        }
        if !mutuals.mutual_spaces.is_empty() {
            ui.add_space(10.0);
            ui.label(theme::section_title(&crate::tf!(
                "ОБЩИЕ ПРОСТРАНСТВА — {a0}",
                a0 = mutuals.mutual_spaces.len()
            )));
            for space in &mutuals.mutual_spaces {
                ui.horizontal(|ui| {
                    theme::avatar(ui, &space.name, 24.0, None, space.icon.as_deref());
                    ui.label(&space.name);
                });
            }
        }
    }
}

/// Рисует окно справа, вплотную к списку участников (`right_edge` — его левая
/// граница). `me` — свой id (у себя кнопок нет).
pub(crate) fn show(
    ctx: &egui::Context,
    state: &ProfileState,
    bridge: &Bridge,
    me: &str,
    right_edge: f32,
) -> ProfileAction {
    let mut action = ProfileAction::None;
    let mut open = true;
    let (pivot, pos) = match state.anchor {
        Anchor::Panel => (egui::Align2::RIGHT_TOP, egui::pos2(right_edge - 4.0, 56.0)),
        Anchor::Beside(rect) => {
            // Справа от аватара; не влезает — слева. По высоте — в пределах окна.
            let screen = ctx.content_rect();
            let height = 420.0_f32.min(screen.height() - 16.0);
            let top = rect
                .top()
                .clamp(screen.top() + 8.0, (screen.bottom() - height).max(8.0));
            if rect.right() + 12.0 + WIDTH <= screen.right() - 8.0 {
                (egui::Align2::LEFT_TOP, egui::pos2(rect.right() + 12.0, top))
            } else {
                (egui::Align2::RIGHT_TOP, egui::pos2(rect.left() - 12.0, top))
            }
        }
    };
    egui::Window::new("profile_window")
        .title_bar(false)
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .fixed_size(Vec2::new(WIDTH, 0.0))
        .pivot(pivot)
        .fixed_pos(pos)
        .constrain(true)
        .frame(
            egui::Frame::new()
                .fill(theme::bg_sidebar())
                .corner_radius(crate::theme::rounded(12))
                .stroke(egui::Stroke::new(1.0, theme::border())),
        )
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            header(ui, state);
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.set_width(WIDTH - 32.0);
                    ui.add_space(30.0);
                    let activities = bridge
                        .cache()
                        .map(|c| c.user_activities(&state.user_id).to_vec())
                        .unwrap_or_default();
                    let (roles, all) = space_roles(bridge, state);
                    let editor = role_editor(bridge, state, &all);
                    body(ui, state, &activities, &roles, editor.as_ref());
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if state.user_id != me {
                            if ui.button(tr("Написать")).clicked() {
                                bridge.request(BridgeRequest::OpenDm(state.user_id.clone()));
                                action = ProfileAction::Close;
                            }
                            if let Some(user) = &state.user
                                && ui.button(tr("Добавить в друзья")).clicked()
                            {
                                bridge.request(BridgeRequest::SendFriendRequest(
                                    bs_api::SendFriendRequest::for_user(
                                        &user.username,
                                        user.home_user_id.as_deref(),
                                        user.home_instance.as_deref(),
                                    ),
                                ));
                            }
                        }
                        if ui.button(tr("Закрыть")).clicked() {
                            action = ProfileAction::Close;
                        }
                    });
                });
        });
    if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = ProfileAction::Close;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::parse_hex;

    #[test]
    fn hex_colors_parse() {
        assert_eq!(
            parse_hex("#ff8000"),
            Some(egui::Color32::from_rgb(255, 128, 0))
        );
        assert_eq!(parse_hex("ff8000"), None);
        assert_eq!(parse_hex("#fff"), None);
    }
}
