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
    /// Раздел резервной копии раскрыт.
    backup_open: bool,
    /// Состояние копии уже спросили.
    backup_asked: bool,
    /// Только что созданный ключ восстановления (показывается один раз).
    recovery: Option<zeroize::Zeroizing<String>>,
    /// Введённый ключ восстановления.
    restore_code: zeroize::Zeroizing<String>,
    /// Итог действия с копией.
    backup_outcome: Option<(String, bool)>,
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
            ui.label(theme::muted(tr("Текст сообщений шифруется на этом устройстве и расшифровывается у собеседника: сервер и его администратор видят только шифротекст. Вложения тоже шифруются; реакции и сам факт переписки не скрываются. Собеседнику нужен клиент с поддержкой шифрования.")).size(13.0));
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
            server_backup(ui, state, bridge);
            transfer(ui, state, bridge);
            ui.add_space(12.0);
            if ui.button(tr("Закрыть")).clicked() {
                keep_open = false;
            }
        });
    keep_open && !response.should_close()
}

/// Резервная копия ключа на сервере под ключом восстановления.
///
/// Ключ восстановления создаёт это устройство (125 бит), сервер его не видит
/// и хранит только запись, зашифрованную им: взлом сервера даёт шифротекст,
/// который не перебрать. Пароль от аккаунта для этого не годится — сервер
/// получает его при каждом входе.
fn server_backup(ui: &mut egui::Ui, state: &mut E2eeDialog, bridge: &mut Bridge) {
    let title = if state.backup_open {
        tr("▾ Резервная копия на сервере")
    } else {
        tr("▸ Резервная копия на сервере")
    };
    if ui.selectable_label(state.backup_open, title).clicked() {
        state.backup_open = !state.backup_open;
    }
    let Some(session) = bridge.e2ee_session(state.origin) else {
        return;
    };
    if !state.backup_asked {
        state.backup_asked = true;
        session.load_e2ee_backup();
    }
    let backup = session.e2ee_backup().cloned();
    let own = session.e2ee_public_key();
    // Копия с другим ключом (новое устройство): раздел раскрывается сам.
    let foreign = matches!(&backup, Some(Ok(Some(b))) if Some(&b.public_key) != own.as_ref());
    if foreign && state.recovery.is_none() {
        state.backup_open = true;
    }
    if !state.backup_open {
        return;
    }
    ui.label(theme::muted(tr("Ключ шифрования сохраняется на сервере, зашифрованный ключом восстановления. Ключ восстановления создаётся на этом устройстве и на сервер не уходит: даже взломав сервер, переписки не прочитать. Без ключа восстановления копию не откроет никто — и вы тоже.")).size(13.0));
    ui.add_space(4.0);
    if let Some(code) = state.recovery.clone() {
        recovery_card(ui, state, &code);
        return;
    }
    match backup {
        None => {
            ui.spinner();
        }
        Some(Err(error)) => {
            ui.label(RichText::new(error).color(theme::amber()).size(13.0));
        }
        Some(Ok(None)) => {
            ui.label(theme::muted(tr("Копии на сервере нет.")).size(13.0));
            if ui.button(tr("Создать резервную копию")).clicked() {
                create(state, session);
            }
        }
        Some(Ok(Some(copy))) => {
            let when =
                crate::theme::short_stamp(copy.updated_at, chrono::Utc::now().timestamp_millis());
            if foreign {
                ui.label(RichText::new(crate::tf!("Копия от {a0} хранит другой ключ, не тот, что на этом устройстве. Введите ключ восстановления, чтобы читать прежние зашифрованные переписки здесь.", a0 = when)).color(theme::amber()).size(13.0));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut *state.restore_code)
                            .password(true)
                            .hint_text("ABCDE-FGHJK-MNPQR-STVWX-YZ012")
                            .desired_width(250.0),
                    );
                    if ui.button(tr("Восстановить")).clicked() {
                        state.backup_outcome =
                            Some(match session.restore_e2ee_backup(&state.restore_code) {
                                Ok(()) => {
                                    state.restore_code.clear();
                                    (tr("Ключ восстановлен из копии").to_owned(), true)
                                }
                                Err(error) => (error, false),
                            });
                    }
                });
            } else {
                ui.label(
                    RichText::new(crate::tf!(
                        "Копия от {a0} — с ключом этого устройства.",
                        a0 = when
                    ))
                    .color(theme::green())
                    .size(13.0),
                );
            }
            ui.horizontal(|ui| {
                // Копия чужого ключа: «заново» затёрло бы ключ прежних переписок.
                if !foreign
                    && ui
                        .button(tr("Создать заново"))
                        .on_hover_text(tr("Новый ключ восстановления; старый перестанет подходить"))
                        .clicked()
                {
                    create(state, session);
                }
                if ui
                    .button(RichText::new(tr("Удалить копию")).color(theme::red()))
                    .clicked()
                {
                    session.delete_e2ee_backup();
                    state.backup_outcome = None;
                }
            });
        }
    }
    if let Some((text, ok)) = &state.backup_outcome {
        let color = if *ok { theme::green() } else { theme::amber() };
        ui.label(RichText::new(text).color(color).size(13.0));
    }
    ui.add_space(6.0);
}

/// Создаёт копию: ключ восстановления — здесь, на сервер — шифротекст.
fn create(state: &mut E2eeDialog, session: &crate::bridge::Session) {
    match session.create_e2ee_backup() {
        Ok(code) => {
            state.recovery = Some(zeroize::Zeroizing::new(code));
            state.backup_outcome = None;
        }
        Err(error) => state.backup_outcome = Some((error, false)),
    }
}

/// Только что созданный ключ восстановления: показать один раз.
fn recovery_card(ui: &mut egui::Ui, state: &mut E2eeDialog, code: &str) {
    ui.label(RichText::new(tr("Ключ восстановления:")).color(theme::text_bright()));
    ui.label(
        RichText::new(code)
            .monospace()
            .size(18.0)
            .color(theme::text_bright()),
    );
    ui.horizontal(|ui| {
        if ui.small_button(tr("Скопировать")).clicked() {
            ui.ctx().copy_text(code.to_owned());
        }
    });
    ui.label(RichText::new(tr("Запишите его и храните отдельно от устройства — в менеджере паролей или на бумаге. Он показывается один раз; сервер его не знает и восстановить не сможет.")).color(theme::amber()).size(13.0));
    if ui.button(tr("Я сохранил ключ восстановления")).clicked() {
        state.recovery = None;
    }
    ui.add_space(6.0);
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
