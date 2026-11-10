//! Страницы «Аккаунт», «Приватность», «Подключения».

#![allow(
    clippy::wildcard_imports,
    reason = "части одного модуля, разнесённые по файлам"
)]

use super::*;

/// «Аккаунт»: предпросмотр профиля, цвет, «обо мне», статус и имя.
pub(super) fn account(ui: &mut egui::Ui, state: &mut SettingsState, me: &User, bridge: &Bridge) {
    page_title(ui, tr("Мой аккаунт"));
    widgets::section_label(ui, tr("Профиль"));
    ui.add_space(6.0);
    profile_preview(ui, state, me);
    ui.add_space(14.0);
    card(ui, |ui| {
        crate::screens::settings_extra::images_card(ui, &mut state.extra, me, bridge);
        widgets::field_label(ui, tr("Цвет аватара"));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for name in theme::PALETTE_NAMES {
                let color = theme::palette(name).unwrap_or_else(theme::accent);
                let selected = state.avatar_color.as_deref() == Some(name);
                if widgets::color_swatch(ui, color, selected, 32.0).clicked() {
                    state.avatar_color = Some(name.to_owned());
                }
            }
        });
        widgets::field_label(ui, tr("Обо мне"));
        widgets::text_area(ui, &mut state.bio, tr("Расскажите о себе…"), 190, 3);
    });
    ui.add_space(14.0);
    widgets::section_label(ui, tr("Учётная запись"));
    ui.add_space(6.0);
    card(ui, |ui| {
        widgets::section_label(ui, tr("Статус"));
        ui.add_space(4.0);
        egui::ComboBox::from_id_salt("settings_status")
            .width(ui.available_width())
            .selected_text(chosen_label(state.status))
            .show_ui(ui, |ui| {
                for value in [
                    ChosenUserStatus::Online,
                    ChosenUserStatus::Idle,
                    ChosenUserStatus::DoNotDisturb,
                ] {
                    ui.selectable_value(&mut state.status, value, chosen_label(value));
                }
            });
        widgets::field_label(ui, tr("Отображаемое имя"));
        widgets::text_field(ui, &mut state.display_name, &me.username, 32);
        widgets::field_label(ui, tr("Пользовательский статус"));
        widgets::text_field(ui, &mut state.custom_status, tr("Чем занимаетесь?"), 128);
    });
    save_bar(ui, state, me, bridge);
    ui.add_space(18.0);
    crate::screens::settings_extra::security_cards(ui, &mut state.extra, me, bridge);
}

/// Карточка-предпросмотр профиля: баннер, аватар, имя, «обо мне».
fn profile_preview(ui: &mut egui::Ui, state: &SettingsState, me: &User) {
    let shown = if state.display_name.trim().is_empty() {
        me.username.clone()
    } else {
        state.display_name.trim().to_owned()
    };
    let banner_color = state
        .avatar_color
        .as_deref()
        .and_then(theme::palette)
        .unwrap_or_else(theme::accent);
    egui::Frame::new()
        .fill(theme::bg_sidebar())
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(CornerRadius::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let (banner, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 80.0), Sense::hover());
            ui.painter().rect_filled(
                banner,
                CornerRadius {
                    nw: 10,
                    ne: 10,
                    sw: 0,
                    se: 0,
                },
                banner_color.gamma_multiply(0.5),
            );
            if let Some(value) = me.banner.as_deref() {
                crate::images::paint_value(
                    ui.painter(),
                    banner,
                    value,
                    CornerRadius {
                        nw: 10,
                        ne: 10,
                        sw: 0,
                        se: 0,
                    },
                    true,
                    egui::Color32::WHITE,
                );
            }
            let avatar_rect = egui::Rect::from_min_size(
                banner.left_bottom() + egui::vec2(16.0, -32.0),
                egui::Vec2::splat(64.0),
            );
            ui.painter()
                .circle_filled(avatar_rect.center(), 35.0, theme::bg_sidebar());
            theme::paint_avatar(
                ui.painter(),
                avatar_rect,
                theme::Face {
                    name: &shown,
                    key: Some(me.home_user_id.as_deref().unwrap_or(&me.id)),
                    // Предпросмотр показывает ещё не сохранённый выбор цвета.
                    color: state.avatar_color.as_deref(),
                },
                None,
                false,
                me.avatar.as_deref(),
            );
            ui.add_space(38.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(
                        RichText::new(&shown)
                            .size(16.0)
                            .family(theme::bold_family())
                            .color(theme::text_bright()),
                    );
                    ui.label(theme::muted(format!("@{}", me.username)).size(12.5));
                    if !state.bio.trim().is_empty() {
                        ui.add_space(4.0);
                        ui.label(theme::muted(theme::truncate_chars(state.bio.trim(), 90)));
                    }
                    ui.add_space(12.0);
                });
            });
        });
}

/// Подпись выбранного статуса.
pub(super) fn chosen_label(status: ChosenUserStatus) -> &'static str {
    match status {
        ChosenUserStatus::Online => tr("В сети"),
        ChosenUserStatus::Idle => tr("Нет на месте"),
        ChosenUserStatus::DoNotDisturb => tr("Не беспокоить"),
    }
}

/// «Приватность»: видимость в поиске и показ активности.
pub(super) fn privacy(ui: &mut egui::Ui, state: &mut SettingsState, me: &User, bridge: &Bridge) {
    page_title(ui, tr("Приватность"));
    card(ui, |ui| {
        for (value, title, hint) in [
            (
                0,
                tr("Показывать меня в поиске"),
                tr("Другие смогут найти вас среди людей инстанса."),
            ),
            (
                1,
                tr("Показывать активность"),
                tr("Друзья увидят, чем вы заняты."),
            ),
        ] {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() - 60.0);
                    ui.label(RichText::new(title).color(theme::text_bright()));
                    ui.label(theme::muted(hint).size(12.5));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let flag = if value == 0 {
                        &mut state.discoverable
                    } else {
                        &mut state.show_activity
                    };
                    widgets::toggle(ui, flag);
                });
            });
            ui.add_space(10.0);
        }
    });
    save_bar(ui, state, me, bridge);
}

/// Панель «Подключения».
pub(super) fn connections(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    bridge: &Bridge,
    action: &mut SettingsAction,
) {
    page_title(ui, tr("Подключённые серверы"));
    let chosen = crate::screens::connections::show(ui, &mut state.connections, bridge);
    if chosen != crate::screens::connections::ConnAction::None {
        *action = SettingsAction::Connections(chosen);
    }
}
