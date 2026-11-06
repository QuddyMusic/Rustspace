//! Звонки в личных сообщениях: исходящие, входящие и их связь с медиа-слоем.

use bs_types::{ClientEvent, LiveKitTokenResponse};
use egui::{Align2, RichText};

use crate::bridge::{BridgeRequest, CallEvent};
use crate::fleet::Bridge;
use crate::theme;
use crate::voice::{DockAction, VoiceState, after_deafen_toggle, after_mute_toggle};
use crate::voice_media::{ConnectRequest, MediaStatus};

/// Фаза звонка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DmPhase {
    /// Ждём ответа собеседника.
    Ringing,
    /// Разговор идёт.
    Active,
}

/// Текущий DM-звонок.
#[allow(clippy::struct_excessive_bools, reason = "независимые флаги состояния")]
#[derive(Debug)]
pub(crate) struct DmCall {
    /// Инстанс.
    pub(crate) origin: usize,
    /// Беседа.
    pub(crate) dm_id: Option<String>,
    /// Идентификатор федеративного звонка.
    pub(crate) federated_id: Option<String>,
    /// Имя собеседника.
    pub(crate) peer: String,
    /// Фаза.
    pub(crate) phase: DmPhase,
    /// Токен LiveKit.
    pub(crate) token: Option<Result<LiveKitTokenResponse, String>>,
    /// Микрофон выключен.
    pub(crate) muted: bool,
    /// Звук выключен.
    pub(crate) deafened: bool,
    /// Медиа запущено.
    pub(crate) media_started: bool,
    /// Флаги переданы живой сессии.
    pub(crate) flags_synced: bool,
}

/// Входящий звонок, на который ещё не ответили.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IncomingCall {
    /// Инстанс.
    pub(crate) origin: usize,
    /// Беседа.
    pub(crate) dm_id: Option<String>,
    /// Идентификатор федеративного звонка.
    pub(crate) federated_id: Option<String>,
    /// Кто звонит.
    pub(crate) caller: String,
    /// Готовый токен, если сервер прислал его сразу.
    pub(crate) token: Option<LiveKitTokenResponse>,
}

/// Ответ пользователя на входящий звонок.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncomingAnswer {
    /// Принять.
    Accept,
    /// Отклонить.
    Reject,
}

/// Совпадают ли идентификаторы звонка.
fn same_call(
    dm_id: Option<&String>,
    fed_id: Option<&String>,
    ev_dm: Option<&String>,
    ev_fed: Option<&String>,
) -> bool {
    (dm_id.is_some() && dm_id == ev_dm) || (fed_id.is_some() && fed_id == ev_fed)
}

impl VoiceState {
    /// Начинает исходящий звонок.
    pub(crate) fn start_dm(
        &mut self,
        bridge: &mut Bridge,
        origin: usize,
        dm_id: &str,
        federated_id: Option<String>,
        peer: &str,
    ) {
        self.leave(bridge);
        self.end_dm(bridge);
        let previous = bridge.focus_index();
        bridge.focus_on(origin);
        bridge.send(ClientEvent::DmCallStart {
            dm_channel_id: dm_id.to_owned(),
        });
        bridge.focus_on(previous);
        self.dm = Some(DmCall {
            origin,
            dm_id: Some(dm_id.to_owned()),
            federated_id,
            peer: peer.to_owned(),
            phase: DmPhase::Ringing,
            token: None,
            muted: self.idle_muted,
            deafened: self.idle_deafened,
            media_started: false,
            flags_synced: false,
        });
    }

    /// Завершает DM-звонок и сообщает серверу.
    pub(crate) fn end_dm(&mut self, bridge: &mut Bridge) {
        let Some(call) = self.dm.take() else {
            return;
        };
        self.reset_media();
        let previous = bridge.focus_index();
        bridge.focus_on(call.origin);
        bridge.send(ClientEvent::DmCallEnd {
            dm_channel_id: call.dm_id,
            federated_call_id: call.federated_id,
        });
        bridge.focus_on(previous);
    }

    /// Останавливает медиа и сбрасывает признаки говорящих.
    pub(crate) fn reset_media(&mut self) {
        if let Some(media) = self.media.as_mut() {
            media.disconnect();
        }
        self.speaking.clear();
        self.status = MediaStatus::Idle;
    }

    /// Разбирает событие звонка от сервера.
    pub(crate) fn handle_call(&mut self, bridge: &mut Bridge, origin: usize, event: CallEvent) {
        match event {
            CallEvent::Incoming {
                dm_channel_id,
                federated_call_id,
                caller_name,
                livekit_url,
                livekit_token,
                ..
            } => {
                if self.call.is_some() || self.dm.is_some() || self.incoming.is_some() {
                    // Заняты: сразу отклоняем.
                    let previous = bridge.focus_index();
                    bridge.focus_on(origin);
                    bridge.send(ClientEvent::DmCallReject {
                        dm_channel_id,
                        federated_call_id,
                    });
                    bridge.focus_on(previous);
                    return;
                }
                self.incoming = Some(IncomingCall {
                    origin,
                    dm_id: dm_channel_id,
                    federated_id: federated_call_id,
                    caller: caller_name,
                    token: livekit_url
                        .zip(livekit_token)
                        .map(|(url, token)| LiveKitTokenResponse { token, url }),
                });
            }
            CallEvent::Accepted {
                dm_channel_id,
                federated_call_id,
            } => {
                let Some(call) = self.dm.as_mut() else {
                    return;
                };
                if call.origin == origin
                    && same_call(
                        call.dm_id.as_ref(),
                        call.federated_id.as_ref(),
                        dm_channel_id.as_ref(),
                        federated_call_id.as_ref(),
                    )
                    && call.phase == DmPhase::Ringing
                {
                    call.phase = DmPhase::Active;
                    if let Some(id) = call.dm_id.clone() {
                        let previous = bridge.focus_index();
                        bridge.focus_on(origin);
                        bridge.request(BridgeRequest::DmVoiceToken(id));
                        bridge.focus_on(previous);
                    }
                }
            }
            CallEvent::Rejected {
                dm_channel_id,
                federated_call_id,
            } => self.finish_remote(
                origin,
                dm_channel_id.as_ref(),
                federated_call_id.as_ref(),
                "Звонок отклонён",
            ),
            CallEvent::Ended {
                dm_channel_id,
                federated_call_id,
            } => self.finish_remote(
                origin,
                dm_channel_id.as_ref(),
                federated_call_id.as_ref(),
                "Звонок завершён",
            ),
            CallEvent::Undeliverable {
                dm_channel_id,
                federated_call_id,
                terminal,
            } => {
                if terminal {
                    self.finish_remote(
                        origin,
                        dm_channel_id.as_ref(),
                        Some(&federated_call_id),
                        "Не удалось дозвониться",
                    );
                }
            }
        }
    }

    /// Собеседник или сервер закончил звонок.
    fn finish_remote(
        &mut self,
        origin: usize,
        dm_id: Option<&String>,
        fed_id: Option<&String>,
        notice: &str,
    ) {
        if let Some(call) = self.incoming.as_ref()
            && call.origin == origin
            && same_call(
                call.dm_id.as_ref(),
                call.federated_id.as_ref(),
                dm_id,
                fed_id,
            )
        {
            self.incoming = None;
            self.notice = Some("Пропущенный звонок".to_owned());
            return;
        }
        if let Some(call) = self.dm.as_ref()
            && call.origin == origin
            && same_call(
                call.dm_id.as_ref(),
                call.federated_id.as_ref(),
                dm_id,
                fed_id,
            )
        {
            self.dm = None;
            self.reset_media();
            self.notice = Some(notice.to_owned());
        }
    }

    /// Отвечает на входящий звонок.
    pub(crate) fn answer(&mut self, bridge: &mut Bridge, answer: IncomingAnswer) {
        let Some(incoming) = self.incoming.take() else {
            return;
        };
        let previous = bridge.focus_index();
        bridge.focus_on(incoming.origin);
        match answer {
            IncomingAnswer::Reject => bridge.send(ClientEvent::DmCallReject {
                dm_channel_id: incoming.dm_id,
                federated_call_id: incoming.federated_id,
            }),
            IncomingAnswer::Accept => {
                bridge.send(ClientEvent::DmCallAccept {
                    dm_channel_id: incoming.dm_id.clone(),
                    federated_call_id: incoming.federated_id.clone(),
                });
                let token = incoming.token.clone().map(Ok);
                if token.is_none()
                    && let Some(id) = incoming.dm_id.clone()
                {
                    bridge.request(BridgeRequest::DmVoiceToken(id));
                }
                bridge.focus_on(previous);
                self.leave(bridge);
                self.dm = Some(DmCall {
                    origin: incoming.origin,
                    dm_id: incoming.dm_id,
                    federated_id: incoming.federated_id,
                    peer: incoming.caller,
                    phase: DmPhase::Active,
                    token,
                    muted: self.idle_muted,
                    deafened: self.idle_deafened,
                    media_started: false,
                    flags_synced: false,
                });
                return;
            }
        }
        bridge.focus_on(previous);
    }

    /// Действие панели для DM-звонка.
    pub(crate) fn apply_dm(&mut self, action: DockAction, bridge: &mut Bridge) {
        match action {
            DockAction::None => {}
            DockAction::Leave => self.end_dm(bridge),
            DockAction::ToggleMute | DockAction::ToggleDeafen => {
                let Some(call) = self.dm.as_mut() else {
                    return;
                };
                (call.muted, call.deafened) = if action == DockAction::ToggleMute {
                    after_mute_toggle(call.muted, call.deafened)
                } else {
                    after_deafen_toggle(call.deafened)
                };
                if let Some(media) = self.media.as_ref() {
                    media.set_flags(call.muted, call.deafened);
                }
            }
        }
    }

    /// Токены, запуск медиа и состояние для DM-звонка.
    pub(crate) fn poll_dm(
        &mut self,
        bridge: &Bridge,
        tokens: &mut Vec<(usize, String, Result<LiveKitTokenResponse, String>)>,
    ) {
        if let Some(call) = self.dm.as_mut() {
            tokens.retain(|(origin, id, result)| {
                if *origin == call.origin && call.dm_id.as_deref() == Some(id.as_str()) {
                    call.token = Some(result.clone());
                    false
                } else {
                    true
                }
            });
        }
        let Some(call) = self.dm.as_mut() else {
            return;
        };
        if call.phase != DmPhase::Active {
            return;
        }
        let start = match (
            &call.token,
            call.media_started,
            bridge.cache_at(call.origin),
        ) {
            (Some(Ok(token)), false, Some(cache)) => {
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
        };
        let (muted, deafened, was_synced) = (call.muted, call.deafened, call.flags_synced);
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
            && let Some(call) = self.dm.as_mut()
        {
            call.flags_synced = true;
            media.set_flags(muted, deafened);
        }
    }
}

/// Окно «Входящий звонок».
pub(crate) fn show_incoming(ctx: &egui::Context, state: &VoiceState) -> Option<IncomingAnswer> {
    let call = state.incoming.as_ref()?;
    let mut answer = None;
    egui::Window::new("incoming_call")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(Align2::CENTER_TOP, [0.0, 24.0])
        .show(ctx, |ui| {
            ui.set_min_width(260.0);
            ui.vertical_centered(|ui| {
                ui.label(theme::muted("Входящий звонок").size(12.5));
                ui.label(
                    RichText::new(&call.caller)
                        .size(18.0)
                        .color(theme::TEXT_BRIGHT),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add(theme::primary_button("Принять")).clicked() {
                        answer = Some(IncomingAnswer::Accept);
                    }
                    if ui.button("Отклонить").clicked() {
                        answer = Some(IncomingAnswer::Reject);
                    }
                });
            });
        });
    answer
}

#[cfg(test)]
mod tests {
    use super::same_call;

    #[test]
    fn matches_by_dm_or_federated_id() {
        let a = "a".to_owned();
        let b = "b".to_owned();
        assert!(same_call(Some(&a), None, Some(&a), None));
        assert!(same_call(None, Some(&b), None, Some(&b)));
        assert!(!same_call(Some(&a), None, Some(&b), None));
        assert!(!same_call(None, None, None, None));
    }
}
