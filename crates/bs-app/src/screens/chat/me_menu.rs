//! Меню своей аватарки в нижней панели: статус, свой статус, профиль —
//! как в Discord, без похода в настройки.

use bs_types::{ChosenUserStatus, UserStatus};
use egui::RichText;

use super::Nav;
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;

/// Аватар и имя в нижней панели — одна кнопка, открывающая меню.
pub(super) fn button(
    ui: &egui::Ui,
    left: egui::Pos2,
    bridge: &Bridge,
    status: UserStatus,
    nav: &mut Option<Nav>,
) {
    let rect = egui::Rect::from_min_max(
        egui::pos2(left.x - 6.0, ui.max_rect().top() + 4.0),
        egui::pos2(ui.min_rect().right() + 4.0, ui.max_rect().bottom() - 4.0),
    );
    let response = ui
        .interact(rect, ui.scope_id().with("me_menu"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.hovered() {
        ui.painter().rect_filled(
            rect,
            crate::theme::rounded(8),
            theme::text_bright().gamma_multiply(0.05),
        );
    }
    if let Some(me) = bridge.cache_at(0).map(|c| c.me().clone()) {
        menu(&response, bridge, &me, status, nav);
    }
}

fn update(bridge: &Bridge, request: bs_api::UpdateProfileRequest) {
    bridge.request(crate::bridge::BridgeRequest::UpdateProfile(request));
}

fn menu(
    button: &egui::Response,
    bridge: &Bridge,
    me: &bs_types::User,
    status: UserStatus,
    nav: &mut Option<Nav>,
) {
    egui::Popup::from_toggle_button_response(button)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::TOP_START)
        .show(|ui| {
            ui.set_width(250.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(
                RichText::new(me.display_name.as_deref().unwrap_or(&me.username))
                    .family(theme::bold_family())
                    .size(15.0)
                    .color(theme::text_bright()),
            );
            ui.label(theme::muted(format!("@{}", me.username)).size(12.0));
            ui.add_space(8.0);
            statuses(ui, bridge, status);
            ui.add_space(6.0);
            custom_status(ui, bridge, me, button.id.with("custom_status"));
            ui.add_space(6.0);
            ui.separator();
            if ui
                .add(egui::Button::new(tr("Мой профиль")).frame(false))
                .clicked()
            {
                *nav = Some(Nav::OwnProfile);
                ui.close();
            }
            if ui
                .add(egui::Button::new(tr("Изменить профиль…")).frame(false))
                .clicked()
            {
                *nav = Some(Nav::Settings);
                ui.close();
            }
        });
}

/// Три статуса с цветными точками; текущий подсвечен.
fn statuses(ui: &mut egui::Ui, bridge: &Bridge, status: UserStatus) {
    for (chosen, shown, label) in [
        (ChosenUserStatus::Online, UserStatus::Online, tr("В сети")),
        (ChosenUserStatus::Idle, UserStatus::Idle, tr("Неактивен")),
        (
            ChosenUserStatus::DoNotDisturb,
            UserStatus::DoNotDisturb,
            tr("Не беспокоить — без уведомлений"),
        ),
    ] {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::click());
        if response.hovered() || status == shown {
            ui.painter()
                .rect_filled(rect, crate::theme::rounded(6), theme::bg_hover());
        }
        ui.painter().circle_filled(
            rect.left_center() + egui::vec2(14.0, 0.0),
            5.0,
            theme::status_color(shown),
        );
        ui.painter().text(
            rect.left_center() + egui::vec2(28.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(13.5),
            theme::text(),
        );
        if response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            update(
                bridge,
                bs_api::UpdateProfileRequest {
                    status: Some(chosen),
                    ..Default::default()
                },
            );
            ui.close();
        }
    }
}

/// Свой статус: поле, «Сохранить» (или Enter) и «Убрать».
fn custom_status(ui: &mut egui::Ui, bridge: &Bridge, me: &bs_types::User, id: egui::Id) {
    ui.label(theme::muted(tr("Свой статус")).size(11.5));
    let saved = me.custom_status.clone().unwrap_or_default();
    let mut draft = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| saved.clone());
    let edit = ui.add(
        egui::TextEdit::singleline(&mut draft)
            .hint_text(tr("Чем заняты?"))
            .char_limit(128)
            .desired_width(f32::INFINITY),
    );
    ui.data_mut(|d| d.insert_temp(id, draft.clone()));
    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.horizontal(|ui| {
        let changed = draft.trim() != saved.trim();
        if ui
            .add_enabled(changed, egui::Button::new(tr("Сохранить")))
            .clicked()
            || (enter && changed)
        {
            update(
                bridge,
                bs_api::UpdateProfileRequest {
                    custom_status: Some(draft.trim().to_owned()),
                    ..Default::default()
                },
            );
        }
        if !saved.is_empty() && ui.button(tr("Убрать")).clicked() {
            ui.data_mut(|d| d.insert_temp(id, String::new()));
            update(
                bridge,
                bs_api::UpdateProfileRequest {
                    custom_status: Some(String::new()),
                    ..Default::default()
                },
            );
        }
    });
}
