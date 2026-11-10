//! Настройки клиента, которые не живут на сервере: горячие клавиши,
//! уведомления, звуки интерфейса.
//!
//! Всё сохраняется через [`crate::prefs`].

use egui::{Align, Layout, RichText};

use super::dialogs::{card, page_title};
use super::settings_extra::setting_row;
use crate::prefs::Action;
use crate::theme;

/// Состояние страницы горячих клавиш.
#[derive(Debug, Default)]
pub(crate) struct HotkeysState {
    /// Действие, для которого сейчас записывается сочетание.
    recording: Option<Action>,
    /// Предупреждение о конфликте.
    conflict: Option<String>,
}

/// «Горячие клавиши».
pub(crate) fn hotkeys(ui: &mut egui::Ui, state: &mut HotkeysState) {
    page_title(ui, "Горячие клавиши");
    if let Some(action) = state.recording
        && let Some(captured) = crate::hotkeys::capture(ui.ctx())
    {
        state.recording = None;
        state.conflict = None;
        if let crate::hotkeys::Captured::Bind(bind) = captured {
            let taken = crate::prefs::get()
                .keybinds
                .iter()
                .find(|(other, existing)| **other != action && **existing == bind)
                .map(|(other, _)| *other);
            if let Some(other) = taken {
                state.conflict = Some(format!(
                    "{} уже назначено на «{}»",
                    bind.label(),
                    other.title()
                ));
            } else {
                crate::prefs::update(|prefs| {
                    prefs.keybinds.insert(action, bind);
                });
            }
        }
    }
    card(ui, |ui| {
        ui.label(
            theme::muted(
                "Сочетания работают, пока окно Backspace в фокусе. В поле ввода срабатывают \
                 только сочетания с Ctrl или Alt.",
            )
            .size(12.5),
        );
        ui.add_space(10.0);
        let binds = crate::prefs::get().keybinds;
        for action in Action::ALL {
            ui.horizontal(|ui| {
                ui.label(RichText::new(action.title()).color(theme::text_bright()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let bound = binds.get(&action).copied();
                    if bound.is_some() && ui.small_button("✕").on_hover_text("Убрать").clicked()
                    {
                        crate::prefs::update(|prefs| {
                            prefs.keybinds.remove(&action);
                        });
                    }
                    let recording = state.recording == Some(action);
                    let label = if recording {
                        "Нажмите сочетание… (Esc — отмена)".to_owned()
                    } else {
                        bound.map_or_else(
                            || "Не назначено".to_owned(),
                            super::super::prefs::Keybind::label,
                        )
                    };
                    if ui
                        .add(
                            egui::Button::new(label)
                                .selected(recording)
                                .min_size(egui::vec2(180.0, 26.0)),
                        )
                        .clicked()
                    {
                        state.recording = (!recording).then_some(action);
                        state.conflict = None;
                    }
                });
            });
            ui.add_space(6.0);
        }
        if let Some(conflict) = &state.conflict {
            ui.label(RichText::new(conflict).color(theme::rose()).size(12.5));
        }
    });
}

/// «Уведомления»: системные всплывающие уведомления.
pub(crate) fn notifications(ui: &mut egui::Ui) {
    page_title(ui, "Уведомления");
    let mut desktop = crate::prefs::get().desktop;
    let before = desktop.clone();
    card(ui, |ui| {
        setting_row(
            ui,
            "Системные уведомления",
            "Всплывающее окно о новом сообщении по тем же правилам, что и звук: \
             ЛС, упоминания и уровень уведомлений канала.",
            &mut desktop.notifications,
        );
        ui.add_enabled_ui(desktop.notifications, |ui| {
            setting_row(
                ui,
                "Показывать текст сообщения",
                "Без этого в уведомлении только автор и канал.",
                &mut desktop.previews,
            );
            setting_row(
                ui,
                "Только когда окно не в фокусе",
                "Не беспокоить, пока вы и так смотрите на Backspace.",
                &mut desktop.only_unfocused,
            );
        });
        if ui.button("Проверить уведомление").clicked() {
            crate::notify::show(crate::notify::Toast {
                title: "Backspace".to_owned(),
                body: "Так будут выглядеть уведомления о сообщениях.".to_owned(),
            });
        }
    });
    if desktop != before {
        crate::prefs::update(|prefs| prefs.desktop = desktop);
    }
}

/// Карточка «Звуки» для страницы «Голос и видео».
pub(crate) fn sounds_card(ui: &mut egui::Ui) {
    let mut sounds = crate::prefs::get().sounds;
    let before = sounds.clone();
    card(ui, |ui| {
        crate::widgets::section_label(ui, "Звуки интерфейса");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Громкость");
            ui.spacing_mut().slider_width = 240.0;
            ui.add(egui::Slider::new(&mut sounds.volume, 0..=200).suffix(" %"));
        });
        ui.add_space(8.0);
        setting_row(
            ui,
            "Звук на каждое сообщение",
            "Играть звук на все сообщения в каналах, а не только на ЛС и упоминания.",
            &mut sounds.message_all_channels,
        );
    });
    if sounds != before {
        crate::prefs::update(|prefs| prefs.sounds = sounds);
    }
}
