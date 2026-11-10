//! Окно «Уведомления» пространства или канала.
//!
//! Тексты — из `spaces.json` (`notifications.*`) оригинала. Настройки живут
//! на инстансе, где расположено пространство.

use bs_core::notifications::{DEFAULT_LEVEL, mute_active, resolve_policy};
use bs_types::{NotificationLevel, NotificationMuteDuration, UpdateNotificationSettingRequest};
use egui::{CornerRadius, Margin, RichText};

use crate::bridge::{BridgeRequest, SpaceEvent, SpaceOp};
use crate::fleet::Bridge;
use crate::{theme, widgets};

/// Состояние окна.
#[derive(Debug)]
pub(crate) struct NotificationSettings {
    origin: usize,
    space_id: String,
    /// `None` — настройка всего пространства.
    channel_id: Option<String>,
    title: String,
    busy: bool,
    error: Option<String>,
}

/// Что выбрано в строке уровня: свой уровень либо «как в пространстве».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Inherit,
    Level(NotificationLevel),
}

const LEVELS: [NotificationLevel; 3] = [
    NotificationLevel::All,
    NotificationLevel::Mentions,
    NotificationLevel::Nothing,
];

const DURATIONS: [(NotificationMuteDuration, &str); 4] = [
    (NotificationMuteDuration::OneHour, "На 1 час"),
    (NotificationMuteDuration::EightHours, "На 8 часов"),
    (NotificationMuteDuration::TwentyFourHours, "На 24 часа"),
    (NotificationMuteDuration::Indefinite, "Пока я не отменю"),
];

fn level_label(level: NotificationLevel) -> &'static str {
    match level {
        NotificationLevel::All => "Обо всех сообщениях",
        NotificationLevel::Mentions => "Только об @упоминаниях",
        NotificationLevel::Nothing => "Ни о чём",
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl NotificationSettings {
    /// Открывает окно; `None`, если пространство или канал неизвестны.
    pub(crate) fn open(
        bridge: &Bridge,
        origin: usize,
        space_id: &str,
        channel_id: Option<&str>,
    ) -> Option<Self> {
        let space = bridge.cache_at(origin)?.space(space_id)?;
        let title = match channel_id {
            Some(id) => {
                let channel = space.channels.iter().find(|c| c.id == id)?;
                format!("Уведомления для #{}", channel.name)
            }
            None => format!("Уведомления для «{}»", space.space.name),
        };
        Some(Self {
            origin,
            space_id: space_id.to_owned(),
            channel_id: channel_id.map(str::to_owned),
            title,
            busy: false,
            error: None,
        })
    }

    fn save(&mut self, bridge: &mut Bridge, change: UpdateNotificationSettingRequest) {
        self.busy = true;
        self.error = None;
        bridge.focus_on(self.origin);
        bridge.request(BridgeRequest::SpaceAdmin {
            space_id: self.space_id.clone(),
            op: SpaceOp::Notify(self.channel_id.clone(), change),
        });
    }

    /// Учитывает итог. `false` — окно надо закрыть.
    pub(crate) fn apply(&mut self, event: &SpaceEvent) -> bool {
        match event {
            SpaceEvent::Notified { space_id, result } if *space_id == self.space_id => {
                self.busy = false;
                if let Err(err) = result {
                    self.error = Some(err.clone());
                }
            }
            SpaceEvent::Left {
                space_id,
                result: Ok(()),
            } if *space_id == self.space_id => return false,
            _ => {}
        }
        true
    }
}

/// Рисует окно. Возвращает `false`, когда его надо закрыть.
pub(crate) fn show(
    ctx: &egui::Context,
    state: &mut NotificationSettings,
    bridge: &mut Bridge,
) -> bool {
    let mut keep_open = true;
    let response = egui::Modal::new(egui::Id::unique("notification_settings"))
        .frame(
            egui::Frame::new()
                .fill(theme::BG_BUBBLE)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(16))
                .inner_margin(Margin::same(20)),
        )
        .show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(
                RichText::new(&state.title)
                    .size(18.0)
                    .family(theme::bold_family())
                    .color(theme::TEXT_BRIGHT),
            );
            ui.add_space(10.0);
            body(ui, state, bridge);
            ui.add_space(12.0);
            if ui.button("Закрыть").clicked() {
                keep_open = false;
            }
        });
    keep_open && !response.should_close()
}

fn body(ui: &mut egui::Ui, state: &mut NotificationSettings, bridge: &mut Bridge) {
    let now = now_ms();
    let Some(cache) = bridge.cache_at(state.origin) else {
        return;
    };
    let own = cache
        .notification_setting(&state.space_id, state.channel_id.as_deref())
        .cloned();
    let space = cache.notification_setting(&state.space_id, None).cloned();
    let is_channel = state.channel_id.is_some();
    let inherited = resolve_policy(space.as_ref(), None, now).level;
    let selected = own.as_ref().and_then(|s| s.level).map_or(
        if is_channel {
            Choice::Inherit
        } else {
            Choice::Level(DEFAULT_LEVEL)
        },
        Choice::Level,
    );
    let mut choices = Vec::new();
    if is_channel {
        choices.push(Choice::Inherit);
    }
    choices.extend(LEVELS.map(Choice::Level));
    widgets::section_label(ui, "Уведомлять");
    ui.add_space(4.0);
    let mut change = None;
    ui.add_enabled_ui(!state.busy, |ui| {
        for choice in choices {
            let title = match choice {
                Choice::Inherit => format!("Как в пространстве ({})", level_label(inherited)),
                Choice::Level(level) => level_label(level).to_owned(),
            };
            if widgets::radio_card(ui, &title, "", choice == selected).clicked()
                && choice != selected
            {
                // Для пространства «наследовать» нет: сброс — это умолчание.
                change = Some(UpdateNotificationSettingRequest {
                    level: Some(match choice {
                        Choice::Inherit => None,
                        Choice::Level(level) => Some(level),
                    }),
                    ..UpdateNotificationSettingRequest::default()
                });
            }
        }
    });
    ui.add_space(10.0);
    widgets::section_label(ui, "Заглушить");
    ui.add_space(4.0);
    mute_block(ui, state, own.as_ref(), now, &mut change);
    if is_channel && mute_active(space.as_ref(), now) {
        ui.add_space(4.0);
        ui.label(
            RichText::new("Это пространство заглушено, поэтому заглушены и его каналы.")
                .size(12.0)
                .color(theme::AMBER),
        );
    }
    if !is_channel {
        mass_mentions(ui, state, own.as_ref(), &mut change);
    }
    ui.add_space(8.0);
    ui.label(
        RichText::new("О личных сообщениях вы получаете уведомления всегда.")
            .size(12.0)
            .color(theme::TEXT_MUTED),
    );
    if let Some(err) = &state.error {
        ui.add_space(6.0);
        ui.label(RichText::new(err).color(theme::ROSE));
    }
    if let Some(change) = change {
        state.save(bridge, change);
    }
}

fn mute_block(
    ui: &mut egui::Ui,
    state: &NotificationSettings,
    own: Option<&bs_types::NotificationSetting>,
    now: i64,
    change: &mut Option<UpdateNotificationSettingRequest>,
) {
    if let Some(setting) = own.filter(|s| mute_active(Some(s), now)) {
        let text = setting.muted_until.map_or_else(
            || "Заглушено, пока вы не отмените".to_owned(),
            |end| format!("Заглушено до {}", theme::stamp(end, now)),
        );
        ui.horizontal(|ui| {
            ui.label(RichText::new(text).color(theme::TEXT_MUTED));
            if ui
                .add_enabled(!state.busy, egui::Button::new("Отменить заглушение"))
                .clicked()
            {
                *change = Some(UpdateNotificationSettingRequest {
                    mute: Some(None),
                    ..UpdateNotificationSettingRequest::default()
                });
            }
        });
        return;
    }
    ui.add_enabled_ui(!state.busy, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (duration, label) in DURATIONS {
                if ui.button(label).clicked() {
                    *change = Some(UpdateNotificationSettingRequest {
                        mute: Some(Some(duration)),
                        ..UpdateNotificationSettingRequest::default()
                    });
                }
            }
        });
    });
}

fn mass_mentions(
    ui: &mut egui::Ui,
    state: &NotificationSettings,
    own: Option<&bs_types::NotificationSetting>,
    change: &mut Option<UpdateNotificationSettingRequest>,
) {
    ui.add_space(10.0);
    widgets::section_label(ui, "Групповые упоминания");
    ui.add_space(4.0);
    for (label, current, roles) in [
        (
            "Подавлять @everyone и @here",
            own.and_then(|s| s.suppress_everyone) == Some(true),
            false,
        ),
        (
            "Подавлять упоминания ролей",
            own.and_then(|s| s.suppress_roles) == Some(true),
            true,
        ),
    ] {
        let mut value = current;
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(theme::TEXT_BRIGHT));
            ui.add_enabled_ui(!state.busy, |ui| widgets::toggle(ui, &mut value));
        });
        if value != current {
            let mut request = UpdateNotificationSettingRequest::default();
            if roles {
                request.suppress_roles = Some(value);
            } else {
                request.suppress_everyone = Some(value);
            }
            *change = Some(request);
        }
    }
    ui.add_space(2.0);
    ui.label(
        RichText::new(
            "@here доходит до тех же участников, что и @everyone. Упоминания вас по имени всегда приходят.",
        )
        .size(12.0)
        .color(theme::TEXT_MUTED),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_labels_are_distinct() {
        let labels: Vec<_> = LEVELS.iter().map(|l| level_label(*l)).collect();
        assert_eq!(labels.len(), 3);
        assert!(labels[0] != labels[1] && labels[1] != labels[2]);
    }

    #[test]
    fn closes_when_user_leaves_the_space() {
        let mut state = NotificationSettings {
            origin: 0,
            space_id: "s1".to_owned(),
            channel_id: None,
            title: String::new(),
            busy: true,
            error: None,
        };
        let failed = SpaceEvent::Notified {
            space_id: "s1".to_owned(),
            result: Err("нет".to_owned()),
        };
        assert!(state.apply(&failed));
        assert!(!state.busy);
        assert_eq!(state.error.as_deref(), Some("нет"));
        let left = SpaceEvent::Left {
            space_id: "s1".to_owned(),
            result: Ok(()),
        };
        assert!(!state.apply(&left));
    }
}
