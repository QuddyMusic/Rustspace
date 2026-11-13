//! Окно шифрования личной переписки: включение, контрольное число,
//! подтверждение нового ключа собеседника, перенос своего ключа на другое
//! устройство.

use eframe::egui::{self, Margin, RichText};

use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;

/// Открытое окно: инстанс и переписка.
#[derive(Clone, Default)]
pub(crate) struct E2eeDialog {
    pub(crate) origin: usize,
    pub(crate) dm_id: String,
    /// Раздел переноса ключа раскрыт.
    transfer: bool,
    /// Пароль записи ключа.
    passphrase: zeroize::Zeroizing<String>,
    /// Выгруженная запись.
    exported: Option<zeroize::Zeroizing<String>>,
    /// Запись с другого устройства.
    import: zeroize::Zeroizing<String>,
    /// Итог последнего действия: текст и успех.
    outcome: Option<(String, bool)>,
}

impl E2eeDialog {
    /// Окно для переписки.
    pub(crate) fn new(origin: usize, dm_id: String) -> Self {
        Self {
            origin,
            dm_id,
            ..Self::default()
        }
    }
}

// Пароль и запись ключа — секреты: в журнал не попадают.
impl std::fmt::Debug for E2eeDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2eeDialog")
            .field("origin", &self.origin)
            .field("dm_id", &self.dm_id)
            .finish_non_exhaustive()
    }
}

/// Рисует окно; `false` — закрыть.
pub(crate) fn show(ctx: &egui::Context, state: &mut E2eeDialog, bridge: &mut Bridge) -> bool {
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
            if bridge.e2ee_locked(state.origin) {
                ui.add_space(8.0);
                ui.label(RichText::new(tr("Ключ шифрования лежит в системном хранилище ключей, но сейчас оно недоступно: работает временный ключ, а сохранённый не тронут. Откройте хранилище (например, войдите в сеанс рабочего стола) и перезапустите приложение.")).color(theme::amber()).size(13.0));
            }
            ui.add_space(10.0);
            ui.separator();
            transfer(ui, state, bridge);
            ui.add_space(12.0);
            if ui.button(tr("Закрыть")).clicked() {
                keep_open = false;
            }
        });
    keep_open && !response.should_close()
}

/// Перенос своего ключа на другое устройство.
fn transfer(ui: &mut egui::Ui, state: &mut E2eeDialog, bridge: &mut Bridge) {
    let title = if state.transfer {
        tr("▾ Ключ на другом устройстве")
    } else {
        tr("▸ Ключ на другом устройстве")
    };
    if ui.selectable_label(state.transfer, title).clicked() {
        state.transfer = !state.transfer;
    }
    if !state.transfer {
        return;
    }
    ui.label(theme::muted(tr("Чтобы читать зашифрованные переписки на другом устройстве, перенесите туда ключ: выгрузите его здесь под паролем и загрузите там с тем же паролем. Запись ключа и пароль передавайте разными путями.")).size(13.0));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(tr("Пароль"));
        ui.add(
            egui::TextEdit::singleline(&mut *state.passphrase)
                .password(true)
                .desired_width(220.0),
        );
    });
    ui.add_space(4.0);
    if ui.button(tr("Выгрузить ключ")).clicked() {
        match bridge.export_e2ee_key(state.origin, &state.passphrase) {
            Ok(text) => {
                state.exported = Some(zeroize::Zeroizing::new(text));
                state.outcome = None;
            }
            Err(error) => state.outcome = Some((error, false)),
        }
    }
    if let Some(text) = &state.exported {
        ui.label(
            RichText::new(text.as_str())
                .monospace()
                .size(11.0)
                .color(theme::text_bright()),
        );
        if ui.small_button(tr("Скопировать")).clicked() {
            ui.ctx().copy_text(text.to_string());
        }
    }
    ui.add_space(8.0);
    ui.label(
        theme::muted(tr(
            "Загрузить ключ с другого устройства (заменит ключ здесь):",
        ))
        .size(13.0),
    );
    ui.add(
        egui::TextEdit::multiline(&mut *state.import)
            .desired_rows(2)
            .desired_width(f32::INFINITY),
    );
    if ui.button(tr("Загрузить ключ")).clicked() {
        state.outcome = Some(
            match bridge.import_e2ee_key(state.origin, &state.import, &state.passphrase) {
                Ok(()) => {
                    state.import.clear();
                    (tr("Ключ загружен").to_owned(), true)
                }
                Err(error) => (error, false),
            },
        );
    }
    if let Some((text, ok)) = &state.outcome {
        let color = if *ok { theme::green() } else { theme::amber() };
        ui.label(RichText::new(text).color(color).size(13.0));
    }
}
