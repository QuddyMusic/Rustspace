//! Звонки в личных сообщениях: исходящие, входящие и их связь с медиа-слоем.

use bs_types::{ClientEvent, LiveKitTokenResponse};
use egui::{Align2, RichText};

use crate::bridge::{BridgeRequest, CallEvent};
use crate::fleet::Bridge;
use crate::i18n::tr;
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
    /// Аватар звонящего.
    pub(crate) avatar: Option<String>,
    /// Когда пришёл звонок: без ответа 30 секунд — отклоняется, как у
    /// оригинала.
    pub(crate) since: web_time::Instant,
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
            incoming @ CallEvent::Incoming { .. } => self.on_incoming(bridge, origin, incoming),
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
                tr("Звонок отклонён"),
            ),
            CallEvent::Ended {
                dm_channel_id,
                federated_call_id,
            } => self.finish_remote(
                origin,
                dm_channel_id.as_ref(),
                federated_call_id.as_ref(),
                tr("Звонок завершён"),
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
                        tr("Не удалось дозвониться"),
                    );
                }
            }
        }
    }

    /// Входящий звонок: окно принятия, повтор того же звонка или «занято».
    fn on_incoming(&mut self, bridge: &mut Bridge, origin: usize, event: CallEvent) {
        let CallEvent::Incoming {
            dm_channel_id,
            federated_call_id,
            caller_id,
            caller_name,
            livekit_url,
            livekit_token,
        } = event
        else {
            return;
        };
        // Звонок между инстансами приходит через каждый из них: повтор
        // того же звонка — не «занято» (отказ по копии оборвал бы и
        // сам звонок, и он стал бы «пропущенным»).
        if self.incoming.as_ref().is_some_and(|known| {
            same_incoming(
                known,
                origin,
                dm_channel_id.as_ref(),
                federated_call_id.as_ref(),
            )
        }) {
            return;
        }
        // Голосовой канал не мешает: принятый звонок сам из него выйдет.
        if self.dm.is_some() || self.incoming.is_some() {
            // Заняты другим личным звонком: сразу отклоняем.
            tracing::info!(
                origin,
                dm = ?dm_channel_id,
                in_call = self.dm.is_some(),
                "входящий звонок отклонён: заняты другим звонком"
            );
            let previous = bridge.focus_index();
            bridge.focus_on(origin);
            bridge.send(ClientEvent::DmCallReject {
                dm_channel_id,
                federated_call_id,
            });
            bridge.focus_on(previous);
            return;
        }
        let member = dm_channel_id
            .as_deref()
            .and_then(|id| bridge.cache_at(origin)?.dm_channel(id))
            .and_then(|channel| channel.members.iter().find(|m| m.id == caller_id));
        let avatar = member.and_then(|m| m.avatar.clone());
        let caller_name = member
            .and_then(|m| m.display_name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(caller_name);
        self.incoming = Some(IncomingCall {
            origin,
            dm_id: dm_channel_id,
            federated_id: federated_call_id,
            caller: caller_name,
            avatar,
            since: web_time::Instant::now(),
            token: livekit_url
                .zip(livekit_token)
                .map(|(url, token)| LiveKitTokenResponse { token, url }),
        });
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
            self.notice = Some(tr("Пропущенный звонок").to_owned());
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
            DockAction::None
            | DockAction::ToggleCamera
            | DockAction::ToggleScreenShare
            | DockAction::SetBitrate(_) => {}
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
                // Собеседник (и клиент оригинала) видит микрофон и звук из
                // voice_status — как в звонке пространства.
                let previous = bridge.focus_index();
                bridge.focus_on(call.origin);
                bridge.send(ClientEvent::VoiceStatus {
                    is_muted: call.muted,
                    is_deafened: call.deafened,
                    is_camera_on: self.video_sent.0,
                    is_screen_sharing: self.video_sent.1,
                });
                bridge.focus_on(previous);
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
                    bitrate_kbps: call
                        .dm_id
                        .as_ref()
                        .and_then(|id| self.bitrates.get(&(call.origin, id.clone())))
                        .map(|b| b.effective_kbps),
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
        if self.status.is_live()
            && !was_synced
            && let Some(call) = self.dm.as_mut()
        {
            call.flags_synced = true;
            media.set_flags(muted, deafened);
        }
    }
}

/// Окно «Входящий звонок», как у оригинала: по центру над затемнением —
/// аватар, имя и круглые кнопки «Отклонить» и «Принять». Без ответа за
/// 30 секунд звонок отклоняется.
pub(crate) fn show_incoming(ctx: &egui::Context, state: &VoiceState) -> Option<IncomingAnswer> {
    let call = state.incoming.as_ref()?;
    if call.since.elapsed() >= std::time::Duration::from_secs(30) {
        return Some(IncomingAnswer::Reject);
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    let mut answer = None;
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::unique("incoming_call_shade"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.painter()
                .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(128));
            ui.allocate_rect(screen, egui::Sense::click());
        });
    egui::Area::new(egui::Id::unique("incoming_call"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::bg_bubble())
                .corner_radius(theme::rounded(12))
                .inner_margin(egui::Margin::same(32))
                .show(ui, |ui| {
                    ui.set_width(276.0);
                    ui.vertical_centered(|ui| {
                        theme::avatar(ui, call.caller.as_str(), 88.0, None, call.avatar.as_deref());
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(&call.caller)
                                .size(20.0)
                                .family(theme::bold_family())
                                .color(theme::text_bright()),
                        );
                        ui.label(theme::muted(tr("Входящий звонок…")).size(14.0));
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            // Две кнопки по 56 и зазор 24 — по центру.
                            ui.add_space((ui.available_width() - 136.0) / 2.0);
                            ui.spacing_mut().item_spacing.x = 24.0;
                            if call_button(ui, theme::rose(), true)
                                .on_hover_text(tr("Отклонить"))
                                .clicked()
                            {
                                answer = Some(IncomingAnswer::Reject);
                            }
                            if call_button(ui, theme::mint(), false)
                                .on_hover_text(tr("Принять"))
                                .clicked()
                            {
                                answer = Some(IncomingAnswer::Accept);
                            }
                        });
                    });
                });
        });
    answer
}

/// Круглая кнопка звонка: трубка или «положить трубку».
fn call_button(ui: &mut egui::Ui, color: egui::Color32, hang_up: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(56.0), egui::Sense::click());
    let alpha = if response.hovered() { 90 } else { 50 };
    let painter = ui.painter();
    painter.circle_filled(rect.center(), 28.0, color.gamma_multiply_u8(alpha));
    painter.circle_stroke(
        rect.center(),
        28.0,
        egui::Stroke::new(1.0, color.gamma_multiply_u8(80)),
    );
    let icon = if hang_up {
        crate::icons::Icon::HangUp
    } else {
        crate::icons::Icon::Phone
    };
    crate::icons::paint(painter, rect.center(), 26.0, icon, color);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Тот же входящий звонок: общий id федеративного звонка или та же беседа.
/// Беседа сверяется и через другую сессию: если в клиенте две сессии к
/// одному серверу (свой инстанс и он же среди пиров, второй аккаунт), звонок
/// приходит по обеим с одним id беседы — отказ «занято» по второй оборвал бы
/// его, и он стал бы «пропущенным».
fn same_incoming(
    known: &IncomingCall,
    origin: usize,
    dm_id: Option<&String>,
    federated_id: Option<&String>,
) -> bool {
    let other_call = matches!(
        (known.federated_id.as_ref(), federated_id),
        (Some(a), Some(b)) if a != b
    );
    (known.federated_id.is_some() && known.federated_id.as_ref() == federated_id)
        || (known.dm_id.is_some()
            && known.dm_id.as_ref() == dm_id
            && (known.origin == origin || !other_call))
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

    #[test]
    fn the_same_call_through_another_instance_is_not_busy() {
        let known = super::IncomingCall {
            origin: 0,
            dm_id: Some("d1".into()),
            federated_id: Some("f1".into()),
            caller: "Bob".into(),
            avatar: None,
            since: web_time::Instant::now(),
            token: None,
        };
        let fed = Some("f1".to_owned());
        let other_dm = Some("d9".to_owned());
        assert!(super::same_incoming(
            &known,
            1,
            other_dm.as_ref(),
            fed.as_ref()
        ));
        let dm = Some("d1".to_owned());
        assert!(super::same_incoming(&known, 0, dm.as_ref(), None));
        let fed2 = Some("f2".to_owned());
        assert!(!super::same_incoming(&known, 1, dm.as_ref(), fed2.as_ref()));
        // Тот же сервер через вторую сессию: та же беседа, без id федерации.
        assert!(super::same_incoming(&known, 1, dm.as_ref(), None));
    }
}
