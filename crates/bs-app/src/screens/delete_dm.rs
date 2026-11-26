//! «Удалить чат»: подтверждение и итог.

use egui::RichText;

use crate::bridge::{BridgeRequest, GroupEvent, GroupOp};
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;

/// Открытое подтверждение удаления.
#[derive(Debug, Clone)]
pub(crate) struct DeleteDm {
    origin: usize,
    dm_id: String,
    label: String,
    busy: bool,
    error: Option<String>,
}

impl DeleteDm {
    pub(crate) const fn new(origin: usize, dm_id: String, label: String) -> Self {
        Self {
            origin,
            dm_id,
            label,
            busy: false,
            error: None,
        }
    }

    /// Учитывает итог; `true` — окно пора закрыть.
    pub(crate) fn apply(&mut self, event: &GroupEvent) -> bool {
        if event.dm_id != self.dm_id || !matches!(event.op, GroupOp::Delete) {
            return false;
        }
        self.busy = false;
        match &event.result {
            Ok(()) => true,
            Err(err) => {
                self.error = Some(err.clone());
                false
            }
        }
    }
}

/// Показывает окно. Возвращает `false`, когда его надо закрыть.
pub(crate) fn show(ctx: &egui::Context, state: &mut DeleteDm, bridge: &mut Bridge) -> bool {
    let mut close = false;
    let open = super::space_dialogs::modal(ctx, "delete_dm", tr("Удалить чат"), |ui| {
        ui.add_space(8.0);
        ui.label(
            RichText::new(crate::tf!(
                "Удалить «{name}» из Rustspace? Чат из Telegram или другого моста удаляется вместе с историей и файлами — в самой сети он останется. Обычная переписка скрывается: у собеседника история сохранится, а новое сообщение вернёт её в список.",
                name = state.label.clone()
            ))
            .color(theme::text_muted()),
        );
        if let Some(err) = &state.error {
            ui.add_space(6.0);
            ui.label(RichText::new(err).color(theme::rose()));
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let label = if state.busy {
                tr("Удаление…")
            } else {
                tr("Удалить")
            };
            if ui
                .add_enabled(
                    !state.busy,
                    egui::Button::new(RichText::new(label).color(theme::rose())),
                )
                .clicked()
            {
                state.busy = true;
                state.error = None;
                bridge.focus_on(state.origin);
                bridge.request(BridgeRequest::GroupAdmin {
                    dm_id: state.dm_id.clone(),
                    op: GroupOp::Delete,
                });
            }
            if ui.button(tr("Отмена")).clicked() {
                close = true;
            }
        });
    });
    open && !close
}
