//! Горячие клавиши голосовых действий.
//!
//! Работают, пока окно в фокусе (глобальный перехват клавиатуры требует
//! прав в системе и отдельной зависимости — оставлен на потом). Если
//! фокус в поле ввода, срабатывают только сочетания с Ctrl или Alt, чтобы
//! обычный набор текста не переключал микрофон.

use std::collections::BTreeMap;

use crate::prefs::{Action, Keybind};
use crate::voice::DockAction;

/// Состояние рации между кадрами.
#[derive(Debug, Default)]
pub(crate) struct PushToTalk {
    /// Клавиша рации зажата и микрофон включён ею.
    active: bool,
}

/// Голосовые действия, которые нужно выполнить в этом кадре.
///
/// `muted` — выключен ли микрофон сейчас: рация включает его на время
/// нажатия и выключает при отпускании.
pub(crate) fn poll(
    ctx: &egui::Context,
    binds: &BTreeMap<Action, Keybind>,
    ptt: &mut PushToTalk,
    muted: bool,
) -> Vec<DockAction> {
    if binds.is_empty() {
        return Vec::new();
    }
    let typing = ctx.egui_wants_keyboard_input();
    let mut out = Vec::new();
    ctx.input(|input| {
        for event in &input.events {
            let egui::Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } = event
            else {
                continue;
            };
            if *repeat {
                continue;
            }
            for (action, bind) in binds {
                if bind.key != *key {
                    continue;
                }
                // Отпускание рации — без проверки модификаторов: их могли
                // отпустить раньше основной клавиши.
                if *action == Action::PushToTalk && !*pressed {
                    if ptt.active {
                        ptt.active = false;
                        out.push(DockAction::ToggleMute);
                    }
                    continue;
                }
                if !*pressed || !bind.modifiers_match(*modifiers) {
                    continue;
                }
                if typing && !(bind.command || bind.alt) {
                    continue;
                }
                match action {
                    Action::ToggleMute => out.push(DockAction::ToggleMute),
                    Action::ToggleDeafen => out.push(DockAction::ToggleDeafen),
                    Action::Disconnect => out.push(DockAction::Leave),
                    Action::PushToTalk => {
                        if muted && !ptt.active {
                            ptt.active = true;
                            out.push(DockAction::ToggleMute);
                        }
                    }
                    // Камера и показ экрана пока без интерфейса: привязка
                    // сохраняется (совместимость с настройками оригинала).
                    Action::ToggleCamera | Action::ToggleScreenShare => {}
                }
            }
        }
    });
    out
}

/// Итог записи сочетания.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Captured {
    /// Нажато сочетание.
    Bind(Keybind),
    /// Escape: запись отменена.
    Cancel,
}

/// Ловит сочетание для записи: первая нажатая клавиша с модификаторами.
pub(crate) fn capture(ctx: &egui::Context) -> Option<Captured> {
    ctx.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::Key {
                key: egui::Key::Escape,
                pressed: true,
                ..
            } => Some(Captured::Cancel),
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } => Some(Captured::Bind(Keybind {
                command: modifiers.command,
                shift: modifiers.shift,
                alt: modifiers.alt,
                key: *key,
            })),
            _ => None,
        })
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{PushToTalk, poll};
    use crate::prefs::{Action, Keybind};
    use crate::voice::DockAction;

    fn key_event(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        }
    }

    fn run(
        binds: &BTreeMap<Action, Keybind>,
        ptt: &mut PushToTalk,
        muted: bool,
        events: Vec<egui::Event>,
    ) -> Vec<DockAction> {
        let ctx = egui::Context::default();
        let mut out = Vec::new();
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            out = poll(ui.ctx(), binds, ptt, muted);
        });
        output.textures_delta.clear();
        out
    }

    fn bind(key: egui::Key, command: bool) -> Keybind {
        Keybind {
            command,
            shift: false,
            alt: false,
            key,
        }
    }

    #[test]
    fn toggle_requires_exact_modifiers() {
        let binds = BTreeMap::from([(Action::ToggleMute, bind(egui::Key::M, true))]);
        let mut ptt = PushToTalk::default();
        let plain = run(
            &binds,
            &mut ptt,
            false,
            vec![key_event(egui::Key::M, true, egui::Modifiers::NONE)],
        );
        assert!(plain.is_empty());
        let with_ctrl = run(
            &binds,
            &mut ptt,
            false,
            vec![key_event(egui::Key::M, true, egui::Modifiers::COMMAND)],
        );
        assert_eq!(with_ctrl, vec![DockAction::ToggleMute]);
    }

    #[test]
    fn push_to_talk_unmutes_while_held() {
        let binds = BTreeMap::from([(Action::PushToTalk, bind(egui::Key::V, false))]);
        let mut ptt = PushToTalk::default();
        let down = run(
            &binds,
            &mut ptt,
            true,
            vec![key_event(egui::Key::V, true, egui::Modifiers::NONE)],
        );
        assert_eq!(down, vec![DockAction::ToggleMute]);
        let up = run(
            &binds,
            &mut ptt,
            false,
            vec![key_event(egui::Key::V, false, egui::Modifiers::NONE)],
        );
        assert_eq!(up, vec![DockAction::ToggleMute]);
        // Микрофон уже включён вручную — рация его не трогает.
        let mut ptt = PushToTalk::default();
        let open = run(
            &binds,
            &mut ptt,
            false,
            vec![
                key_event(egui::Key::V, true, egui::Modifiers::NONE),
                key_event(egui::Key::V, false, egui::Modifiers::NONE),
            ],
        );
        assert!(open.is_empty());
    }
}
