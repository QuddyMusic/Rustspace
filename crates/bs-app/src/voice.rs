//! Голосовое общение: состояние текущего звонка и панель управления.
//!
//! Сервер учитывает присутствие в голосе по событиям WebSocket (`voice_join`,
//! `voice_leave`, `voice_status`); токен LiveKit запрашивается отдельно и
//! нужен только самому медиа-слою.

use bs_types::{ClientEvent, LiveKitTokenResponse};
use egui::{Align, Layout, Margin, RichText};

use crate::bridge::BridgeRequest;
use crate::fleet::Bridge;
use crate::icons::Icon;
use crate::theme;
use crate::voice_media::{ConnectRequest, MediaEngine, MediaStatus, Speaking, VoicePrefs};
use crate::widgets;

/// Текущий голосовой звонок.
#[allow(clippy::struct_excessive_bools, reason = "независимые флаги состояния")]
#[derive(Debug)]
pub(crate) struct VoiceCall {
    /// Инстанс, на котором идёт звонок.
    pub(crate) origin: usize,
    /// Голосовой канал.
    pub(crate) channel_id: String,
    /// Название канала.
    pub(crate) channel_name: String,
    /// Название пространства.
    pub(crate) space_name: String,
    /// Микрофон выключен.
    pub(crate) muted: bool,
    /// Звук выключен (наушники).
    pub(crate) deafened: bool,
    /// Токен LiveKit или причина, по которой его нет.
    pub(crate) token: Option<Result<LiveKitTokenResponse, String>>,
    /// Сервер уже подтвердил наше присутствие в канале.
    pub(crate) confirmed: bool,
    /// Медиа-соединение запущено.
    pub(crate) media_started: bool,
    /// Флаги микрофона и звука переданы живой сессии.
    pub(crate) flags_synced: bool,
}

/// Состояние голоса в приложении.
#[derive(Debug, Default)]
pub(crate) struct VoiceState {
    /// Настройки голоса.
    pub(crate) prefs: VoicePrefs,
    /// Медиа-движок (создаётся при первом использовании).
    pub(crate) media: Option<MediaEngine>,
    /// Кто сейчас говорит (id пользователей).
    pub(crate) speaking: Speaking,
    /// Состояние медиа-соединения.
    pub(crate) status: MediaStatus,
    /// Активный звонок.
    pub(crate) call: Option<VoiceCall>,
    /// Сообщение для пользователя (например, «вас отключили»).
    pub(crate) notice: Option<String>,
}

/// Что сделать после кадра панели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DockAction {
    /// Ничего.
    None,
    /// Переключить микрофон.
    ToggleMute,
    /// Переключить наушники.
    ToggleDeafen,
    /// Завершить звонок.
    Leave,
}

/// Новые флаги после переключения наушников: как в оригинале, они тянут за собой микрофон.
#[must_use]
pub(crate) const fn after_deafen_toggle(deafened: bool) -> (bool, bool) {
    // (muted, deafened)
    (!deafened, !deafened)
}

/// Новый флаг микрофона; при выключенных наушниках включить микрофон нельзя.
#[must_use]
pub(crate) const fn after_mute_toggle(muted: bool, deafened: bool) -> (bool, bool) {
    if deafened {
        (true, true)
    } else {
        (!muted, false)
    }
}

impl VoiceState {
    /// Медиа-движок; создаётся на runtime моста.
    pub(crate) fn media(&mut self, bridge: &Bridge) -> &mut MediaEngine {
        self.media
            .get_or_insert_with(|| MediaEngine::new(bridge.runtime()))
    }

    /// Входит в голосовой канал; предыдущий звонок завершается.
    pub(crate) fn join(
        &mut self,
        bridge: &mut Bridge,
        origin: usize,
        channel_id: &str,
        channel_name: &str,
        space_name: &str,
    ) {
        self.leave(bridge);
        self.notice = None;
        let previous = bridge.focus_index();
        bridge.focus_on(origin);
        bridge.send(ClientEvent::VoiceJoin {
            channel_id: channel_id.to_owned(),
        });
        bridge.request(BridgeRequest::VoiceToken(channel_id.to_owned()));
        bridge.focus_on(previous);
        self.call = Some(VoiceCall {
            origin,
            channel_id: channel_id.to_owned(),
            channel_name: channel_name.to_owned(),
            space_name: space_name.to_owned(),
            muted: false,
            deafened: false,
            token: None,
            confirmed: false,
            media_started: false,
            flags_synced: false,
        });
    }

    /// Выходит из канала.
    pub(crate) fn leave(&mut self, bridge: &mut Bridge) {
        let Some(call) = self.call.take() else {
            return;
        };
        if let Some(media) = self.media.as_mut() {
            media.disconnect();
        }
        self.speaking.clear();
        self.status = MediaStatus::Idle;
        let previous = bridge.focus_index();
        bridge.focus_on(call.origin);
        bridge.send(ClientEvent::VoiceLeave);
        bridge.focus_on(previous);
    }

    /// Применяет действие панели.
    pub(crate) fn apply(&mut self, action: DockAction, bridge: &mut Bridge) {
        match action {
            DockAction::None => {}
            DockAction::Leave => self.leave(bridge),
            DockAction::ToggleMute => {
                if let Some(call) = self.call.as_mut() {
                    (call.muted, call.deafened) = after_mute_toggle(call.muted, call.deafened);
                    send_status(bridge, call);
                    if let Some(media) = self.media.as_ref() {
                        media.set_flags(call.muted, call.deafened);
                    }
                }
            }
            DockAction::ToggleDeafen => {
                if let Some(call) = self.call.as_mut() {
                    (call.muted, call.deafened) = after_deafen_toggle(call.deafened);
                    send_status(bridge, call);
                    if let Some(media) = self.media.as_ref() {
                        media.set_flags(call.muted, call.deafened);
                    }
                }
            }
        }
    }

    /// Разбирает ответы моста и следит, что сервер нас не отключил.
    pub(crate) fn poll(&mut self, bridge: &mut Bridge) {
        for (origin, event) in bridge.take_voice_tokens() {
            if let Some(call) = self.call.as_mut()
                && call.origin == origin
                && call.channel_id == event.channel_id
            {
                call.token = Some(event.result);
            }
        }
        let Some(call) = self.call.as_mut() else {
            return;
        };
        let Some(cache) = bridge.cache_at(call.origin) else {
            return;
        };
        let present = cache
            .users_in_voice(&call.channel_id)
            .iter()
            .any(|id| *id == cache.me().id);
        if present {
            call.confirmed = true;
        } else if call.confirmed {
            self.leave_quietly();
            self.notice = Some("Вы отключены от голосового канала".to_owned());
            return;
        }
        self.drive_media(bridge);
    }

    /// Закрывает звонок без сообщения серверу (он сам нас отключил).
    fn leave_quietly(&mut self) {
        self.call = None;
        if let Some(media) = self.media.as_mut() {
            media.disconnect();
        }
        self.speaking.clear();
        self.status = MediaStatus::Idle;
    }

    /// Запускает медиа, когда есть токен, и подтягивает его состояние.
    fn drive_media(&mut self, bridge: &Bridge) {
        let Some(call) = self.call.as_mut() else {
            return;
        };
        let start = if call.media_started {
            None
        } else {
            match (&call.token, bridge.cache_at(call.origin)) {
                (Some(Ok(token)), Some(cache)) => {
                    let me = cache.me();
                    Some(ConnectRequest {
                        url: token.url.clone(),
                        token: token.token.clone(),
                        identity: me.id.clone(),
                        name: me
                            .display_name
                            .clone()
                            .unwrap_or_else(|| me.username.clone()),
                        muted: call.muted,
                        deafened: call.deafened,
                    })
                }
                _ => None,
            }
        };
        let (muted, deafened) = (call.muted, call.deafened);
        let was_synced = call.flags_synced;
        if let Some(request) = start {
            call.media_started = true;
            let prefs = self.prefs.clone();
            self.media(bridge).connect(request, &prefs);
        }
        let Some(media) = self.media.as_ref() else {
            return;
        };
        self.status = media.status();
        self.speaking = media.speaking();
        if self.status == MediaStatus::Live
            && !was_synced
            && let Some(call) = self.call.as_mut()
        {
            call.flags_synced = true;
            media.set_flags(muted, deafened);
        }
    }
}

fn send_status(bridge: &mut Bridge, call: &VoiceCall) {
    let previous = bridge.focus_index();
    bridge.focus_on(call.origin);
    bridge.send(ClientEvent::VoiceStatus {
        is_muted: call.muted,
        is_deafened: call.deafened,
        is_camera_on: false,
        is_screen_sharing: false,
    });
    bridge.focus_on(previous);
}

/// Панель над профилем: канал, состояние соединения и кнопки.
pub(crate) fn show_dock(ctx_ui: &mut egui::Ui, state: &VoiceState) -> DockAction {
    let media = state.status.clone();
    let mut action = DockAction::None;
    let Some(call) = state.call.as_ref() else {
        return action;
    };
    egui::Panel::bottom("voice_dock")
        .exact_size(74.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(theme::BG_SIDEBAR)
                .inner_margin(Margin::symmetric(10, 4)),
        )
        .show(ctx_ui, |ui| {
            egui::Frame::new()
                .fill(theme::BG_RAIL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .corner_radius(egui::CornerRadius::same(12))
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            let (text, color) = status_line(call, &media);
                            ui.label(RichText::new(text).size(12.5).color(color));
                            ui.label(
                                theme::muted(theme::truncate_chars(
                                    &format!("{} / {}", call.channel_name, call.space_name),
                                    13,
                                ))
                                .size(11.5),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::icon_button(ui, Icon::HangUp, 30.0)
                                .on_hover_text("Отключиться")
                                .clicked()
                            {
                                action = DockAction::Leave;
                            }
                            let headphones = if call.deafened {
                                Icon::HeadphonesOff
                            } else {
                                Icon::Headphones
                            };
                            if widgets::icon_button(ui, headphones, 30.0)
                                .on_hover_text(if call.deafened {
                                    "Включить звук"
                                } else {
                                    "Выключить звук"
                                })
                                .clicked()
                            {
                                action = DockAction::ToggleDeafen;
                            }
                            let mic = if call.muted { Icon::MicOff } else { Icon::Mic };
                            if widgets::icon_button(ui, mic, 30.0)
                                .on_hover_text(if call.muted {
                                    "Включить микрофон"
                                } else {
                                    "Выключить микрофон"
                                })
                                .clicked()
                            {
                                action = DockAction::ToggleMute;
                            }
                        });
                    });
                });
        });
    action
}

fn status_line(call: &VoiceCall, media: &MediaStatus) -> (&'static str, egui::Color32) {
    match (&call.token, media, call.confirmed) {
        (Some(Err(_)), _, _) => ("Нет токена LiveKit", theme::ROSE),
        (_, MediaStatus::Failed(_), _) => ("Нет звукового соединения", theme::ROSE),
        (_, MediaStatus::Live, _) => ("Голос подключён", theme::GREEN),
        (_, MediaStatus::Unavailable, true) => ("Без звука (нет LiveKit)", theme::TEXT_MUTED),
        _ => ("Подключение…", theme::AMBER),
    }
}

#[cfg(test)]
mod tests {
    use super::{after_deafen_toggle, after_mute_toggle};

    #[test]
    fn deafen_drags_mute_along_and_back() {
        assert_eq!(after_deafen_toggle(false), (true, true));
        assert_eq!(after_deafen_toggle(true), (false, false));
    }

    #[test]
    fn cannot_unmute_while_deafened() {
        assert_eq!(after_mute_toggle(true, true), (true, true));
        assert_eq!(after_mute_toggle(false, false), (true, false));
        assert_eq!(after_mute_toggle(true, false), (false, false));
    }
}
