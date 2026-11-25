//! «Ключи доступа» в настройках аккаунта: вход без пароля и второй фактор.

use egui::RichText;

use crate::bridge::{AdminRequest, BridgeRequest, PasskeyOp};
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;

/// Состояние раздела.
#[derive(Debug, Default)]
pub(crate) struct PasskeysUi {
    /// Ключи (`None` — ещё не знаем).
    pub(crate) list: Option<Result<Vec<bs_api::Passkey>, String>>,
    /// Ждём ответа сервера или браузера.
    pub(crate) busy: bool,
    /// Ждём именно подтверждения нового ключа.
    adding: bool,
    /// Код для сверки со страницей в браузере.
    pub(crate) code: Option<String>,
    /// Билет, подтверждения которого пора ждать.
    pub(crate) ticket: Option<String>,
}

/// Раздел целиком (у федеративного аккаунта ключи на его домашнем инстансе).
pub(crate) fn section(ui: &mut egui::Ui, state: &mut PasskeysUi, federated: bool, bridge: &Bridge) {
    if federated {
        return;
    }
    ui.add_space(14.0);
    crate::screens::settings_extra::page_title_small(ui, tr("Ключи доступа"));
    crate::screens::dialogs::card(ui, |ui| show(ui, state, bridge));
}

fn send(bridge: &Bridge, account: &mut PasskeysUi, op: PasskeyOp) {
    account.adding = matches!(op, PasskeyOp::Add | PasskeyOp::Await(_));
    account.busy = true;
    bridge.request(BridgeRequest::Admin(AdminRequest::Passkeys(op)));
}

/// Список ключей, «Добавить», «Удалить».
fn show(ui: &mut egui::Ui, account: &mut PasskeysUi, bridge: &Bridge) {
    if let Some(ticket) = account.ticket.take() {
        send(bridge, account, PasskeyOp::Await(ticket));
    }
    if account.list.is_none() && !account.busy {
        send(bridge, account, PasskeyOp::List);
    }
    ui.label(
        theme::muted(tr(
            "Ключ доступа (passkey) заменяет пароль и код. Хранится в Bitwarden, телефоне или Windows Hello.",
        ))
        .size(12.5),
    );
    ui.add_space(6.0);
    let now = chrono::Utc::now().timestamp_millis();
    let mut remove = None;
    match &account.list {
        Some(Ok(keys)) if keys.is_empty() => {
            ui.label(theme::muted(tr("Ключей пока нет.")));
        }
        Some(Ok(keys)) => {
            for key in keys {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("🔑").size(15.0));
                    let name = if key.name.is_empty() {
                        tr("Ключ доступа")
                    } else {
                        key.name.as_str()
                    };
                    ui.label(RichText::new(name).color(theme::text_bright()));
                    let used = key.last_used_at.map_or_else(
                        || {
                            crate::tf!(
                                "добавлен {a0}",
                                a0 = theme::short_stamp(key.created_at, now)
                            )
                        },
                        |at| crate::tf!("вход {a0}", a0 = theme::short_stamp(at, now)),
                    );
                    ui.label(theme::muted(used).size(12.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !account.busy,
                                egui::Button::new(RichText::new(tr("Удалить")).color(theme::red())),
                            )
                            .clicked()
                        {
                            remove = Some(key.id.clone());
                        }
                    });
                });
            }
        }
        Some(Err(err)) => {
            ui.label(RichText::new(err).color(theme::rose()).size(13.0));
        }
        None => {
            ui.spinner();
        }
    }
    if let Some(id) = remove {
        send(bridge, account, PasskeyOp::Remove(id));
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!account.busy, theme::primary_button(tr("Добавить ключ")))
            .clicked()
        {
            send(bridge, account, PasskeyOp::Add);
        }
        if account.busy && account.adding {
            ui.spinner();
            ui.label(theme::muted(tr("Подтвердите в браузере…")).size(12.5));
        }
    });
    if let Some(code) = account
        .code
        .as_ref()
        .filter(|_| account.busy && account.adding)
    {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(theme::muted(tr("Код на странице в браузере должен быть таким:")).size(12.5));
            ui.label(
                RichText::new(code)
                    .monospace()
                    .size(18.0)
                    .color(theme::text_bright()),
            );
        });
        ui.label(
            theme::muted(tr(
                "Это не код двухфакторной аутентификации: он только связывает окно с этой страницей.",
            ))
            .size(11.5),
        );
    }
}
