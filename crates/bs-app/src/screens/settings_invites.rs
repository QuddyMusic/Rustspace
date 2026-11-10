//! Админская панель «Регистрация»: открытость регистрации и инвайт-ссылки.

use bs_api::{Invite, InvitePatch, NewInvite, Redemption};
use egui::RichText;

use super::settings_extra::{Note, fail, ok, setting_row, show_note};
use crate::bridge::{AdminEvent, AdminRequest, BridgeRequest, InviteOp};
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::screens::dialogs::{card, page_title};
use crate::theme;
use crate::widgets;

/// Сроки действия для новой ссылки: подпись и миллисекунды (0 — бессрочно).
const EXPIRIES: [(&str, i64); 5] = [
    ("Бессрочно", 0),
    ("1 час", 3_600_000),
    ("1 день", 86_400_000),
    ("7 дней", 7 * 86_400_000),
    ("30 дней", 30 * 86_400_000),
];

/// Форма создания или правки.
#[derive(Debug, Clone, Default)]
struct Form {
    /// `None` — создание, иначе правка/восстановление этой ссылки.
    id: Option<String>,
    reinstate: bool,
    name: String,
    max_uses: u32,
    expiry: usize,
    keep_expiry: bool,
}

/// Состояние панели.
#[derive(Debug, Default)]
pub(crate) struct InvitesUi {
    list: Option<Vec<Invite>>,
    archived: bool,
    asked: Option<bool>,
    note: Option<Note>,
    form: Option<Form>,
    confirm_delete: Option<String>,
    redemptions: Option<(String, Result<Vec<Redemption>, String>)>,
}

impl InvitesUi {
    /// Забирает свои события, остальные возвращает.
    pub(crate) fn accept(&mut self, event: AdminEvent) -> Option<AdminEvent> {
        match event {
            AdminEvent::Invites(result) => match result {
                Ok((archived, list)) if archived == self.archived => self.list = Some(list),
                Ok(_) => {}
                Err(message) => self.note = Some(fail(message)),
            },
            AdminEvent::InviteDone(result) => {
                self.note = Some(match result {
                    Ok(text) => ok(text),
                    Err(message) => fail(message),
                });
                self.asked = None;
                self.form = None;
                self.confirm_delete = None;
            }
            AdminEvent::Redemptions { id, result } => self.redemptions = Some((id, result)),
            other => return Some(other),
        }
        None
    }
}

fn send(bridge: &Bridge, request: AdminRequest) -> bool {
    bridge.request(BridgeRequest::Admin(request))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn status_text(invite: &Invite) -> (&'static str, egui::Color32) {
    match invite.status.as_str() {
        "active" => (tr("Действует"), theme::green()),
        "expired" => (tr("Истекла"), theme::amber()),
        "exhausted" => (tr("Использована"), theme::amber()),
        "revoked" => (tr("Отозвана"), theme::rose()),
        _ => ("—", theme::text_faint()),
    }
}

fn uses_text(invite: &Invite) -> String {
    invite.max_uses.map_or_else(
        || crate::tf!("Использований: {a0}", a0 = invite.used_count),
        |max| {
            crate::tf!(
                "Использований: {a0} из {max}",
                a0 = invite.used_count,
                max = max
            )
        },
    )
}

fn expiry_text(invite: &Invite) -> String {
    match invite.expires_at {
        None => tr("Бессрочно").to_owned(),
        Some(at) if at <= now_ms() => crate::tf!("Истекла {a0}", a0 = theme::stamp(at, now_ms())),
        Some(at) => crate::tf!("Действует до {a0}", a0 = theme::stamp(at, now_ms())),
    }
}

/// Собирает запрос из формы.
fn form_patch(form: &Form, now: i64) -> InvitePatch {
    let (_, span) = EXPIRIES[form.expiry.min(EXPIRIES.len() - 1)];
    InvitePatch {
        name: (!form.reinstate).then(|| form.name.trim().to_owned()),
        max_uses: Some((form.max_uses > 0).then_some(form.max_uses)),
        expires_at: if form.keep_expiry {
            None
        } else {
            Some((span > 0).then_some(now + span))
        },
    }
}

fn show_form(ui: &mut egui::Ui, ui_state: &mut InvitesUi, bridge: &Bridge) {
    let Some(form) = ui_state.form.as_mut() else {
        return;
    };
    let title = match (&form.id, form.reinstate) {
        (None, _) => tr("Новая ссылка"),
        (Some(_), true) => tr("Восстановить ссылку"),
        (Some(_), false) => tr("Изменить ссылку"),
    };
    let mut submit = false;
    let mut cancel = false;
    card(ui, |ui| {
        ui.label(RichText::new(title).color(theme::text_bright()));
        if !form.reinstate {
            widgets::field_label(ui, tr("Название"));
            widgets::text_field(ui, &mut form.name, tr("Например, «для друзей»"), 64);
        }
        ui.horizontal(|ui| {
            ui.label(tr("Макс. использований (0 — без предела)"));
            ui.add(egui::DragValue::new(&mut form.max_uses).range(0..=100_000));
        });
        ui.horizontal(|ui| {
            ui.label(tr("Срок действия"));
            if form.id.is_some() {
                ui.checkbox(&mut form.keep_expiry, tr("не менять"));
            }
            ui.add_enabled_ui(!form.keep_expiry, |ui| {
                egui::ComboBox::from_id_salt("invite_expiry")
                    .selected_text(tr(EXPIRIES[form.expiry.min(EXPIRIES.len() - 1)].0))
                    .show_ui(ui, |ui| {
                        for (i, (label, _)) in EXPIRIES.iter().enumerate() {
                            ui.selectable_value(&mut form.expiry, i, tr(label));
                        }
                    });
            });
        });
        let valid = form.reinstate || !form.name.trim().is_empty();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(valid, theme::primary_button(tr("Сохранить")))
                .clicked()
            {
                submit = true;
            }
            if ui.button(tr("Отмена")).clicked() {
                cancel = true;
            }
        });
    });
    if submit {
        let form = form.clone();
        let patch = form_patch(&form, now_ms());
        let op = match (form.id, form.reinstate) {
            (None, _) => InviteOp::Create(NewInvite {
                name: form.name.trim().to_owned(),
                max_uses: patch.max_uses.flatten(),
                expires_at: patch.expires_at.flatten(),
            }),
            (Some(id), true) => InviteOp::Reinstate(id, patch),
            (Some(id), false) => InviteOp::Update(id, patch),
        };
        send(bridge, AdminRequest::Invite(op));
    } else if cancel {
        ui_state.form = None;
    }
    ui.add_space(8.0);
}

#[allow(clippy::too_many_lines, reason = "карточка ссылки с действиями")]
fn invite_card(ui: &mut egui::Ui, state: &mut InvitesUi, invite: &Invite, bridge: &Bridge) {
    let (label, color) = status_text(invite);
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&invite.name).color(theme::text_bright()));
            ui.label(RichText::new(label).color(color).size(12.5));
        });
        ui.label(theme::muted(uses_text(invite)).size(12.5));
        ui.label(theme::muted(expiry_text(invite)).size(12.5));
        if let Some(by) = &invite.created_by_username {
            ui.label(theme::muted(crate::tf!("Создал: {by}", by = by)).size(12.5));
        }
        ui.horizontal_wrapped(|ui| {
            if invite.status == "active" && ui.button(tr("Копировать ссылку")).clicked()
            {
                ui.ctx().copy_text(invite.url.clone());
                state.note = Some(ok(tr("Ссылка скопирована")));
            }
            if invite.status == "active" {
                if ui.button(tr("Изменить")).clicked() {
                    state.form = Some(Form {
                        id: Some(invite.id.clone()),
                        name: invite.name.clone(),
                        max_uses: invite.max_uses.unwrap_or(0),
                        keep_expiry: true,
                        ..Form::default()
                    });
                }
                if ui.button(tr("Отозвать")).clicked() {
                    send(
                        bridge,
                        AdminRequest::Invite(InviteOp::Revoke(invite.id.clone())),
                    );
                }
            } else if ui.button(tr("Восстановить")).clicked() {
                state.form = Some(Form {
                    id: Some(invite.id.clone()),
                    reinstate: true,
                    max_uses: invite.max_uses.unwrap_or(0),
                    expiry: 3,
                    ..Form::default()
                });
            }
            if ui.button(tr("Кто использовал")).clicked() {
                send(
                    bridge,
                    AdminRequest::Invite(InviteOp::Redemptions(invite.id.clone())),
                );
            }
            if state.confirm_delete.as_deref() == Some(invite.id.as_str()) {
                if ui.button(tr("Точно удалить")).clicked() {
                    send(
                        bridge,
                        AdminRequest::Invite(InviteOp::Delete(invite.id.clone())),
                    );
                }
                if ui.button(tr("Отмена")).clicked() {
                    state.confirm_delete = None;
                }
            } else if ui.button(tr("Удалить")).clicked() {
                state.confirm_delete = Some(invite.id.clone());
            }
        });
        if let Some((id, result)) = &state.redemptions
            && *id == invite.id
        {
            match result {
                Err(message) => {
                    ui.label(RichText::new(message).color(theme::rose()).size(12.5));
                }
                Ok(list) if list.is_empty() => {
                    ui.label(theme::muted(tr("Пока никто не воспользовался")).size(12.5));
                }
                Ok(list) => {
                    for r in list {
                        let name = if r.is_deleted {
                            crate::tf!("{a0} (удалён)", a0 = r.registrant_username)
                        } else {
                            r.current_username
                                .clone()
                                .unwrap_or_else(|| r.registrant_username.clone())
                        };
                        ui.label(
                            theme::muted(format!(
                                "{name} · {}",
                                theme::stamp(r.redeemed_at, now_ms())
                            ))
                            .size(12.5),
                        );
                    }
                }
            }
        }
    });
    ui.add_space(8.0);
}

/// «Регистрация».
pub(crate) fn registration_panel(
    ui: &mut egui::Ui,
    extra: &mut super::settings_extra::ExtraUi,
    bridge: &Bridge,
) {
    let admin = &mut extra.admin;
    page_title(ui, tr("Регистрация"));
    super::settings_extra::ensure_settings(admin, bridge);
    if let Some(edit) = admin.edit_mut() {
        card(ui, |ui| {
            setting_row(
                ui,
                tr("Открытая регистрация"),
                tr("Любой может создать аккаунт без инвайта."),
                &mut edit.registration_open,
            );
            setting_row(
                ui,
                tr("Регистрация федеративных аккаунтов"),
                tr("Пользователи других инстансов могут создавать здесь аккаунты."),
                &mut edit.federated_registration_open,
            );
        });
        let patch = admin.settings_patch_only();
        let changed = patch != bs_api::InstanceSettingsPatch::default();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(changed, theme::primary_button(tr("Сохранить")))
                .clicked()
            {
                send(bridge, AdminRequest::SaveSettings(patch));
            }
            if ui
                .add_enabled(changed, egui::Button::new(tr("Сбросить")))
                .clicked()
            {
                admin.reset_edit();
            }
        });
        ui.add_space(12.0);
    }
    let state = &mut admin.invites;
    widgets::section_label(ui, tr("Инвайт-ссылки"));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let before = state.archived;
        ui.selectable_value(&mut state.archived, false, tr("Активные"));
        ui.selectable_value(&mut state.archived, true, tr("Архив"));
        if state.archived != before {
            state.list = None;
            state.asked = None;
        }
        if ui.button(tr("Создать ссылку")).clicked() {
            state.form = Some(Form::default());
        }
    });
    ui.add_space(6.0);
    if state.asked != Some(state.archived) {
        let archived = state.archived;
        if send(bridge, AdminRequest::LoadInvites { archived }) {
            state.asked = Some(archived);
        }
    }
    show_note(ui, state.note.as_ref());
    show_form(ui, state, bridge);
    let Some(list) = state.list.clone() else {
        ui.label(theme::muted(tr("Загрузка…")));
        return;
    };
    if list.is_empty() {
        theme::empty_state(
            ui,
            if state.archived {
                tr("Архив пуст")
            } else {
                tr("Активных ссылок нет")
            },
        );
    }
    for invite in &list {
        invite_card(ui, state, invite, bridge);
    }
}

#[cfg(test)]
mod tests {
    use super::{Form, form_patch};

    #[test]
    fn zero_means_unlimited_and_never() {
        let patch = form_patch(&Form::default(), 1_000);
        assert_eq!(patch.max_uses, Some(None));
        assert_eq!(patch.expires_at, Some(None));
    }

    #[test]
    fn keep_expiry_leaves_it_untouched() {
        let form = Form {
            keep_expiry: true,
            max_uses: 5,
            id: Some("i".to_owned()),
            ..Form::default()
        };
        let patch = form_patch(&form, 1_000);
        assert_eq!(patch.expires_at, None);
        assert_eq!(patch.max_uses, Some(Some(5)));
    }

    #[test]
    fn expiry_is_relative_to_now() {
        let form = Form {
            expiry: 2,
            ..Form::default()
        };
        assert_eq!(
            form_patch(&form, 1_000).expires_at,
            Some(Some(1_000 + 86_400_000))
        );
    }
}
