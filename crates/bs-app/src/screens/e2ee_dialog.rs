//! Окно шифрования личной переписки: включение, контрольное число,
//! подтверждение нового ключа собеседника.

use eframe::egui::{self, Margin, RichText};

use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;

/// Открытое окно: инстанс и переписка.
#[derive(Debug, Clone)]
pub(crate) struct E2eeDialog {
    pub(crate) origin: usize,
    pub(crate) dm_id: String,
}

/// Рисует окно; `false` — закрыть.
pub(crate) fn show(ctx: &egui::Context, state: &E2eeDialog, bridge: &mut Bridge) -> bool {
    let mut keep_open = true;
    let status = bridge.e2ee_status(state.origin, &state.dm_id);
    let response = egui::Modal::new(egui::Id::unique("e2ee_dialog"))
        .frame(
            egui::Frame::new()
                .fill(theme::bg_bubble())
                .stroke(egui::Stroke::new(1.0, theme::border()))
                .corner_radius(crate::theme::rounded(16))
                .inner_margin(Margin::same(20)),
        )
        .show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(tr("Сквозное шифрование")).size(18.0).strong().color(theme::text_bright()));
            ui.add_space(6.0);
            ui.label(theme::muted(tr("Текст сообщений шифруется на этом устройстве и расшифровывается у собеседника: сервер и его администратор видят только шифротекст. Вложения, реакции и сам факт переписки не скрываются. Собеседнику нужен клиент с поддержкой шифрования.")).size(13.0));
            ui.add_space(10.0);
            let mut enabled = status.enabled;
            if ui.checkbox(&mut enabled, tr("Шифровать мои сообщения в этой переписке")).changed() {
                bridge.set_e2ee(state.origin, &state.dm_id, enabled);
            }
            ui.add_space(8.0);
            if status.key_changed {
                ui.label(RichText::new(tr("Собеседник сменил ключ шифрования. Так бывает после переустановки или на новом устройстве — но так же выглядит и подмена ключа. Сверьте контрольное число по другому каналу связи и только потом примите новый ключ.")).color(theme::amber()).size(13.0));
                ui.add_space(4.0);
                if ui.button(tr("Принять новый ключ")).clicked() {
                    bridge.accept_e2ee_key(state.origin, &state.dm_id);
                }
                ui.add_space(8.0);
            }
            match &status.safety_number {
                Some(number) => {
                    ui.label(theme::muted(tr("Контрольное число. Если у собеседника оно такое же, между вами нет подменённого ключа:")).size(13.0));
                    ui.label(RichText::new(number).monospace().size(15.0).color(theme::text_bright()));
                    if ui.small_button(tr("Скопировать")).clicked() {
                        ui.ctx().copy_text(number.clone());
                    }
                }
                None if status.enabled => {
                    ui.label(theme::muted(tr("Ждём ключ собеседника: он придёт, когда собеседник откроет переписку в клиенте с поддержкой шифрования. До этого сообщения не отправляются.")).size(13.0));
                }
                None => {}
            }
            ui.add_space(12.0);
            if ui.button(tr("Закрыть")).clicked() {
                keep_open = false;
            }
        });
    keep_open && !response.should_close()
}
