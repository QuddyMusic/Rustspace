//! Горячие клавиши голосовых действий.
//!
//! В окне — через события egui: если фокус в поле ввода, срабатывают только
//! сочетания с Ctrl или Alt, чтобы обычный набор текста не переключал
//! микрофон. Со сборкой `global-hotkeys` те же сочетания перехватываются
//! глобально (см. [`crate::desktop`]), и тогда окно их не обрабатывает,
//! чтобы действие не сработало дважды.

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
/// Переводит нажатие или отпускание привязки в действие звонка.
pub(crate) fn dispatch(
    action: Action,
    pressed: bool,
    ptt: &mut PushToTalk,
    muted: bool,
    out: &mut Vec<DockAction>,
) {
    if action == Action::PushToTalk {
        if pressed && muted && !ptt.active {
            ptt.active = true;
            out.push(DockAction::ToggleMute);
        } else if !pressed && ptt.active {
            ptt.active = false;
            out.push(DockAction::ToggleMute);
        }
        return;
    }
    if !pressed {
        return;
    }
    out.push(match action {
        Action::ToggleMute => DockAction::ToggleMute,
        Action::ToggleDeafen => DockAction::ToggleDeafen,
        Action::Disconnect => DockAction::Leave,
        Action::ToggleCamera => DockAction::ToggleCamera,
        Action::ToggleScreenShare => DockAction::ToggleScreenShare,
        Action::PushToTalk => return,
    });
}

/// Действия по клавишам окна. `global` — привязки, перехваченные глобально.
pub(crate) fn poll(
    ctx: &egui::Context,
    binds: &BTreeMap<Action, Keybind>,
    global: &std::collections::BTreeSet<Action>,
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
                if bind.key != *key || global.contains(action) {
                    continue;
                }
                // Отпускание рации — без проверки модификаторов: их могли
                // отпустить раньше основной клавиши.
                if *action == Action::PushToTalk && !*pressed {
                    dispatch(*action, false, ptt, muted, &mut out);
                    continue;
                }
                if !*pressed || !bind.modifiers_match(*modifiers) {
                    continue;
                }
                if typing && !(bind.command || bind.alt) {
                    continue;
                }
                dispatch(*action, true, ptt, muted, &mut out);
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
            out = poll(
                ui.ctx(),
                binds,
                &std::collections::BTreeSet::new(),
                ptt,
                muted,
            );
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
