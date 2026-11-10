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

/// Что открыто в окне.
#[derive(Debug)]
pub(crate) struct ProfileState {
    user_id: String,
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
        bridge.request(BridgeRequest::Profile(user_id.to_owned()));
        Self {
            user_id: user_id.to_owned(),
            name: name.to_owned(),
            user: None,
            error: None,
            mutuals: None,
        }
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

fn body(ui: &mut egui::Ui, state: &ProfileState) {
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
    egui::Window::new("profile_window")
        .title_bar(false)
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .fixed_size(Vec2::new(WIDTH, 0.0))
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(egui::pos2(right_edge - 4.0, 56.0))
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
                    body(ui, state);
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
                                    user.username.clone(),
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
