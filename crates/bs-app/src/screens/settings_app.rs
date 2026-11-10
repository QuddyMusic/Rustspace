//! Настройки клиента, которые не живут на сервере: горячие клавиши.
//!
//! Всё сохраняется через [`crate::prefs`].

use egui::{Align, Layout, RichText};

use super::dialogs::{card, page_title};
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
