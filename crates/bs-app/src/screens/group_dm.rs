//! Групповые беседы: «Создать группу» и «Настройки группы».
//!
//! Тексты — из `dm.json` (`groupSettings.*`, `kick.*`, `transfer.*`,
//! `leave.*`) оригинала. Решения принимает сервер; клиент лишь прячет
//! действия, которых у пользователя нет.

use bs_api::{GROUP_MAX_MEMBERS, GROUP_NAME_MAX, GroupMember};
use bs_types::{DmChannel, Friend};
use egui::{Margin, RichText};

use crate::bridge::{BridgeRequest, GroupEvent, GroupOp};
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::social::user_display_name;
use crate::{theme, widgets};

/// Что сделало окно после события.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Остаётся открытым.
    Keep,
    /// Закрыть.
    Close,
    /// Закрыть и открыть эту беседу.
    Open(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Confirm {
    Leave,
    Kick(String),
    Transfer(String),
}

#[derive(Debug)]
enum Mode {
    Create {
        picked: Vec<String>,
    },
    Manage {
        dm_id: String,
        name: String,
        confirm: Option<Confirm>,
        adding: bool,
    },
}

/// Состояние окна.
#[derive(Debug)]
pub(crate) struct GroupDialog {
    origin: usize,
    mode: Mode,
    search: String,
    busy: bool,
    error: Option<String>,
}

fn friend_name(friend: &Friend) -> String {
    user_display_name(friend.display_name.as_deref(), &friend.username)
}

fn member(friend: &Friend) -> GroupMember {
    GroupMember {
        id: friend.id.clone(),
        home_user_id: friend.home_user_id.clone(),
        home_instance: friend.home_instance.clone(),
    }
}

/// Можно ли отправить создание: нужны от 2 до `GROUP_MAX_MEMBERS - 1` человек.
fn can_create(picked: usize) -> bool {
    (2..GROUP_MAX_MEMBERS).contains(&picked)
}

impl GroupDialog {
    /// Окно создания группы на инстансе `origin`.
    pub(crate) fn create(origin: usize) -> Self {
        Self {
            origin,
            mode: Mode::Create { picked: Vec::new() },
            search: String::new(),
            busy: false,
            error: None,
        }
    }

    /// Окно управления группой.
    pub(crate) fn manage(origin: usize, dm: &DmChannel) -> Self {
        Self {
            origin,
            mode: Mode::Manage {
                dm_id: dm.id.clone(),
                name: dm.name.clone().unwrap_or_default(),
                confirm: None,
                adding: false,
            },
            search: String::new(),
            busy: false,
            error: None,
        }
    }

    fn send(&mut self, bridge: &mut Bridge, dm_id: String, op: GroupOp) {
        self.busy = true;
        self.error = None;
        bridge.focus_on(self.origin);
        bridge.request(BridgeRequest::GroupAdmin { dm_id, op });
    }

    /// Учитывает итог действия.
    pub(crate) fn apply(&mut self, event: &GroupEvent) -> Outcome {
        let mine = match &self.mode {
            Mode::Create { .. } => matches!(event.op, GroupOp::Create(_)),
            Mode::Manage { dm_id, .. } => *dm_id == event.dm_id,
        };
        if !mine {
            return Outcome::Keep;
        }
        self.busy = false;
        match (&event.op, &event.result) {
            (GroupOp::Create(_), Ok(())) => Outcome::Open(event.dm_id.clone()),
            (GroupOp::Leave, Ok(())) => Outcome::Close,
            (op, Ok(())) => {
                if let Mode::Manage {
                    confirm, adding, ..
                } = &mut self.mode
                {
                    *confirm = None;
                    if matches!(op, GroupOp::Add(_)) {
                        *adding = false;
                    }
                }
                Outcome::Keep
            }
            (_, Err(err)) => {
                self.error = Some(err.clone());
                Outcome::Keep
            }
        }
    }
}

/// Рисует окно. `false` — окно нужно закрыть.
pub(crate) fn show(ctx: &egui::Context, state: &mut GroupDialog, bridge: &mut Bridge) -> bool {
    let mut keep_open = true;
    let response = egui::Modal::new(egui::Id::unique("group_dm"))
        .frame(
            egui::Frame::new()
                .fill(theme::bg_bubble())
                .stroke(egui::Stroke::new(1.0, theme::border()))
                .corner_radius(crate::theme::rounded(16))
                .inner_margin(Margin::same(20)),
        )
        .show(ctx, |ui| {
            ui.set_width(400.0);
            let managed = match &state.mode {
                Mode::Create { .. } => None,
                Mode::Manage { dm_id, .. } => Some(dm_id.clone()),
            };
            if let Some(dm_id) = managed {
                let dm = bridge
                    .cache_at(state.origin)
                    .and_then(|c| c.dm_channel(&dm_id))
                    .cloned();
                match dm {
                    Some(dm) => manage(ui, state, bridge, &dm),
                    None => keep_open = false,
                }
            } else {
                create(ui, state, bridge);
            }
            if let Some(err) = &state.error {
                ui.add_space(6.0);
                ui.label(RichText::new(err).color(theme::rose()));
            }
            ui.add_space(10.0);
            if ui.button(tr("Закрыть")).clicked() {
                keep_open = false;
            }
        });
    keep_open && !response.should_close()
}

fn title(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(18.0)
            .family(theme::bold_family())
            .color(theme::text_bright()),
    );
    ui.add_space(8.0);
}

fn create(ui: &mut egui::Ui, state: &mut GroupDialog, bridge: &mut Bridge) {
    title(ui, tr("Создать группу"));
    ui.label(
        RichText::new(crate::tf!(
            "Выберите от 2 до {a0} друзей. Вы добавитесь в группу автоматически.",
            a0 = GROUP_MAX_MEMBERS - 1
        ))
        .color(theme::text_muted()),
    );
    ui.add_space(6.0);
    let friends: Vec<Friend> = bridge.friends().to_vec();
    let Mode::Create { picked } = &mut state.mode else {
        return;
    };
    let search = &mut state.search;
    widgets::text_field(ui, search, tr("Поиск друзей…"), 64);
    ui.add_space(4.0);
    let needle = search.trim().to_lowercase();
    egui::ScrollArea::vertical()
        .max_height(260.0)
        .show(ui, |ui| {
            for friend in friends
                .iter()
                .filter(|f| needle.is_empty() || friend_name(f).to_lowercase().contains(&needle))
            {
                let mut on = picked.contains(&friend.id);
                let full = picked.len() >= GROUP_MAX_MEMBERS - 1 && !on;
                ui.add_enabled_ui(!full, |ui| {
                    ui.horizontal(|ui| {
                        let name = friend_name(friend);
                        theme::avatar(ui, &name, 28.0, None, None);
                        if ui.checkbox(&mut on, name).changed() {
                            if on {
                                picked.push(friend.id.clone());
                            } else {
                                picked.retain(|id| *id != friend.id);
                            }
                        }
                    });
                });
            }
            if friends.is_empty() {
                ui.label(RichText::new(tr("Сначала добавьте друзей")).color(theme::text_muted()));
            }
        });
    ui.add_space(8.0);
    let count = picked.len();
    let users: Vec<GroupMember> = friends
        .iter()
        .filter(|f| picked.contains(&f.id))
        .map(member)
        .collect();
    let label = if state.busy {
        tr("Создание…").to_owned()
    } else {
        crate::tf!("Создать группу ({count})", count = count)
    };
    if ui
        .add_enabled(
            can_create(count) && !state.busy,
            theme::primary_button(&label),
        )
        .clicked()
    {
        state.send(bridge, String::new(), GroupOp::Create(users));
    }
}

fn manage(ui: &mut egui::Ui, state: &mut GroupDialog, bridge: &mut Bridge, dm: &DmChannel) {
    let me = bridge
        .cache_at(state.origin)
        .map(|c| c.me().id.clone())
        .unwrap_or_default();
    let owner = dm.owner_id.as_deref() == Some(me.as_str());
    title(ui, tr("Настройки группы"));
    icon_row(ui, state, bridge, dm, owner);
    ui.add_space(8.0);
    name_row(ui, state, bridge, dm, owner);
    ui.add_space(10.0);
    widgets::section_label(
        ui,
        &crate::tf!(
            "Участники — {a0} из {GROUP_MAX_MEMBERS}",
            a0 = dm.members.len(),
            GROUP_MAX_MEMBERS = GROUP_MAX_MEMBERS
        ),
    );
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .show(ui, |ui| {
            for user in &dm.members {
                member_row(ui, state, bridge, dm, owner, &me, user);
            }
        });
    ui.add_space(8.0);
    add_member(ui, state, bridge, dm);
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);
    leave(ui, state, bridge, dm);
}

/// Предел размера файла иконки (`GROUP_DM_ICON_MAX_BYTES` оригинала).
const ICON_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Иконка группы: превью, «Загрузить…» и «Убрать» (только владелец).
fn icon_row(
    ui: &mut egui::Ui,
    state: &mut GroupDialog,
    bridge: &mut Bridge,
    dm: &DmChannel,
    owner: bool,
) {
    let mut op = None;
    ui.horizontal(|ui| {
        let base = bridge.base_at(state.origin).map(str::to_owned);
        let icon = dm.icon.as_deref().map(|value| {
            base.as_deref().map_or_else(
                || value.to_owned(),
                |base| crate::images::absolute(base, value),
            )
        });
        let name = dm.name.clone().unwrap_or_else(|| tr("Группа").to_owned());
        theme::avatar(
            ui,
            theme::Face {
                name: &name,
                key: Some(&dm.id),
                color: None,
            },
            56.0,
            None,
            icon.as_deref(),
        );
        ui.add_space(8.0);
        ui.vertical(|ui| {
            widgets::field_label(ui, tr("Иконка группы"));
            ui.add_enabled_ui(owner && !state.busy, |ui| {
                ui.horizontal(|ui| {
                    if ui.add(theme::primary_button(tr("Загрузить…"))).clicked() {
                        op = pick_icon()
                            .map(|picked| picked.map(|bytes| GroupOp::Icon(Some(bytes))));
                    }
                    if dm.icon.is_some() && ui.button(tr("Убрать")).clicked() {
                        op = Some(Ok(GroupOp::Icon(None)));
                    }
                });
            });
            ui.label(theme::muted(tr("Квадрат по центру картинки, до 8 МБ.")).size(12.0));
        });
    });
    match op {
        Some(Ok(op)) => state.send(bridge, dm.id.clone(), op),
        Some(Err(err)) => state.error = Some(err),
        None => {}
    }
}

/// Диалог выбора картинки; `None` — пользователь передумал.
fn pick_icon() -> Option<Result<Vec<u8>, String>> {
    let path = rfd::FileDialog::new()
        .add_filter(tr("Картинки"), &["png", "jpg", "jpeg", "gif", "webp"])
        .pick_file()?;
    let size = std::fs::metadata(&path).map_or(0, |m| m.len());
    if size > ICON_MAX_BYTES {
        return Some(Err(tr("Файл больше 8 МБ").to_owned()));
    }
    Some(
        std::fs::read(&path)
            .map_err(|err| crate::tf!("Не удалось прочитать файл: {err}", err = err)),
    )
}

fn name_row(
    ui: &mut egui::Ui,
    state: &mut GroupDialog,
    bridge: &mut Bridge,
    dm: &DmChannel,
    owner: bool,
) {
    let saved = dm.name.clone().unwrap_or_default();
    let busy = state.busy;
    let mut send = None;
    if let Mode::Manage { name, .. } = &mut state.mode {
        widgets::field_label(ui, tr("Название группы"));
        ui.add_enabled_ui(owner, |ui| {
            widgets::text_field(ui, name, tr("Название группы"), GROUP_NAME_MAX);
        });
        ui.label(
            RichText::new(format!("{}/{GROUP_NAME_MAX}", name.chars().count()))
                .size(12.0)
                .color(theme::text_muted()),
        );
        let trimmed = name.trim().to_owned();
        if owner && trimmed != saved.trim() {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!busy, theme::primary_button(tr("Сохранить")))
                    .clicked()
                {
                    send = Some((!trimmed.is_empty()).then_some(trimmed));
                }
                if ui.button(tr("Отменить изменения")).clicked() {
                    name.clone_from(&saved);
                }
            });
        }
    }
    if let Some(next) = send {
        state.send(bridge, dm.id.clone(), GroupOp::Rename(next));
    }
}

fn member_row(
    ui: &mut egui::Ui,
    state: &mut GroupDialog,
    bridge: &mut Bridge,
    dm: &DmChannel,
    owner: bool,
    me: &str,
    user: &bs_types::User,
) {
    let name = user_display_name(user.display_name.as_deref(), &user.username);
    let is_owner = dm.owner_id.as_deref() == Some(user.id.as_str());
    let confirm = match &state.mode {
        Mode::Manage { confirm, .. } => confirm.clone(),
        Mode::Create { .. } => None,
    };
    ui.horizontal(|ui| {
        theme::avatar(ui, &name, 28.0, None, None);
        ui.label(RichText::new(&name).color(theme::text_bright()));
        if is_owner {
            ui.label(
                RichText::new(tr("· Владелец группы"))
                    .size(12.0)
                    .color(theme::text_muted()),
            );
        }
        if owner && user.id != me {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button(tr("Удалить")).clicked() {
                    set_confirm(state, Some(Confirm::Kick(user.id.clone())));
                }
                if ui.small_button(tr("Передать права")).clicked() {
                    set_confirm(state, Some(Confirm::Transfer(user.id.clone())));
                }
            });
        }
    });
    let asked = |c: &Confirm| confirm.as_ref() == Some(c);
    if asked(&Confirm::Kick(user.id.clone())) {
        let text = crate::tf!(
            "Удалить {name} из этой группы? Этот участник больше не увидит новые сообщения.",
            name = name
        );
        match confirm_box(ui, &text, tr("Удалить из группы"), state.busy) {
            Some(true) => {
                state.send(bridge, dm.id.clone(), GroupOp::Remove(user.id.clone()));
            }
            Some(false) => set_confirm(state, None),
            None => {}
        }
    } else if asked(&Confirm::Transfer(user.id.clone())) {
        let text = crate::tf!(
            "Передать права владельца {name}? Вы лишитесь прав владельца.",
            name = name
        );
        match confirm_box(ui, &text, tr("Передать"), state.busy) {
            Some(true) => {
                state.send(bridge, dm.id.clone(), GroupOp::Transfer(user.id.clone()));
            }
            Some(false) => set_confirm(state, None),
            None => {}
        }
    }
}

/// Текст подтверждения: `Some(true)` — подтвердили, `Some(false)` — отменили.
fn confirm_box(ui: &mut egui::Ui, text: &str, button: &str, busy: bool) -> Option<bool> {
    let mut pressed = None;
    egui::Frame::new()
        .fill(theme::bg_rail())
        .corner_radius(crate::theme::rounded(8))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(theme::text_muted()));
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new(button)).clicked() {
                    pressed = Some(true);
                }
                if ui.button(tr("Отмена")).clicked() {
                    pressed = Some(false);
                }
            });
        });
    pressed
}

fn set_confirm(state: &mut GroupDialog, value: Option<Confirm>) {
    if let Mode::Manage { confirm, .. } = &mut state.mode {
        *confirm = value;
    }
}

fn add_member(ui: &mut egui::Ui, state: &mut GroupDialog, bridge: &mut Bridge, dm: &DmChannel) {
    if dm.members.len() >= GROUP_MAX_MEMBERS {
        ui.label(RichText::new(tr("Группа заполнена")).color(theme::text_muted()));
        return;
    }
    let adding = matches!(&state.mode, Mode::Manage { adding: true, .. });
    if !adding {
        if ui.button(tr("Добавить участника")).clicked()
            && let Mode::Manage { adding, .. } = &mut state.mode
        {
            *adding = true;
        }
        return;
    }
    widgets::text_field(ui, &mut state.search, tr("Поиск друзей…"), 64);
    let needle = state.search.trim().to_lowercase();
    let candidates: Vec<Friend> = bridge
        .friends()
        .iter()
        .filter(|f| !dm.members.iter().any(|m| m.id == f.id))
        .filter(|f| needle.is_empty() || friend_name(f).to_lowercase().contains(&needle))
        .cloned()
        .collect();
    egui::ScrollArea::vertical()
        .max_height(140.0)
        .show(ui, |ui| {
            if candidates.is_empty() {
                ui.label(RichText::new(tr("Нет подходящих друзей")).color(theme::text_muted()));
            }
            for friend in &candidates {
                ui.horizontal(|ui| {
                    let name = friend_name(friend);
                    theme::avatar(ui, &name, 24.0, None, None);
                    ui.label(&name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!state.busy, egui::Button::new(tr("Добавить")))
                            .clicked()
                        {
                            state.send(bridge, dm.id.clone(), GroupOp::Add(friend.id.clone()));
                        }
                    });
                });
            }
        });
}

fn leave(ui: &mut egui::Ui, state: &mut GroupDialog, bridge: &mut Bridge, dm: &DmChannel) {
    let asked = matches!(
        &state.mode,
        Mode::Manage {
            confirm: Some(Confirm::Leave),
            ..
        }
    );
    if !asked {
        if ui
            .add(egui::Button::new(
                RichText::new(tr("Покинуть группу")).color(theme::rose()),
            ))
            .clicked()
        {
            set_confirm(state, Some(Confirm::Leave));
        }
        return;
    }
    let name = dm
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| tr("группа").to_owned());
    let text = crate::tf!(
        "Покинуть «{name}»? Вы перестанете получать сообщения из этой беседы. Остальные участники увидят системное сообщение.",
        name = name
    );
    let label = if state.busy {
        tr("Выход…")
    } else {
        tr("Покинуть группу")
    };
    match confirm_box(ui, &text, label, state.busy) {
        Some(true) => {
            state.send(bridge, dm.id.clone(), GroupOp::Leave);
        }
        Some(false) => set_confirm(state, None),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_size_limits() {
        assert!(!can_create(1));
        assert!(can_create(2));
        assert!(can_create(9));
        assert!(!can_create(10));
    }

    #[test]
    fn create_finishes_by_opening_the_new_group() {
        let mut dialog = GroupDialog::create(0);
        dialog.busy = true;
        let event = GroupEvent {
            dm_id: "g1".to_owned(),
            op: GroupOp::Create(vec![GroupMember::local("a"), GroupMember::local("b")]),
            result: Ok(()),
        };
        assert_eq!(dialog.apply(&event), Outcome::Open("g1".to_owned()));
        assert!(!dialog.busy);
    }

    #[test]
    fn errors_stay_in_the_dialog() {
        let mut dialog = GroupDialog::create(0);
        let event = GroupEvent {
            dm_id: String::new(),
            op: GroupOp::Create(Vec::new()),
            result: Err("нет".to_owned()),
        };
        assert_eq!(dialog.apply(&event), Outcome::Keep);
        assert_eq!(dialog.error.as_deref(), Some("нет"));
    }
}
