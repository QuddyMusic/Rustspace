//! Голосовое общение: состояние текущего звонка и панель управления.
//!
//! Сервер учитывает присутствие в голосе по событиям WebSocket (`voice_join`,
//! `voice_leave`, `voice_status`); токен LiveKit запрашивается отдельно и
//! нужен только самому медиа-слою.

use bs_types::{ClientEvent, LiveKitTokenResponse};
use egui::{Align, Layout, Margin, RichText};

use crate::bridge::BridgeRequest;
use crate::fleet::Bridge;
use crate::i18n::tr;
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
    /// Идентификатор пространства.
    pub(crate) space_id: String,
    /// Модератор заглушил нас: микрофон не включить.
    pub(crate) forced_mute: bool,
    /// Модератор отключил нам звук.
    pub(crate) forced_deafen: bool,
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
    /// Исходящий или идущий DM-звонок.
    pub(crate) dm: Option<crate::dmcall::DmCall>,
    /// Входящий звонок, на который ещё не ответили.
    pub(crate) incoming: Option<crate::dmcall::IncomingCall>,
    /// Микрофон выключен вне звонка (применяется при входе).
    pub(crate) idle_muted: bool,
    /// Звук выключен вне звонка.
    pub(crate) idle_deafened: bool,
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
    /// Проигрыватель звуков интерфейса.
    pub(crate) sounds: crate::sounds::Sounds,
    /// Каким было состояние комнаты на прошлом кадре (для звуков).
    room_view: crate::sounds::RoomView,
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
    /// Текущие флаги (микрофон, звук): из звонка или заданные заранее.
    pub(crate) fn flags(&self) -> (bool, bool) {
        self.dm.as_ref().map_or_else(
            || {
                self.call
                    .as_ref()
                    .map_or((self.idle_muted, self.idle_deafened), |call| {
                        (call.muted, call.deafened)
                    })
            },
            |dm| (dm.muted, dm.deafened),
        )
    }

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
        space_id: &str,
    ) {
        self.leave(bridge);
        self.end_dm(bridge);
        self.notice = None;
        let previous = bridge.focus_index();
        bridge.focus_on(origin);
        bridge.send(ClientEvent::VoiceJoin {
            channel_id: channel_id.to_owned(),
        });
        bridge.request(BridgeRequest::VoiceToken(channel_id.to_owned()));
        bridge.focus_on(previous);
        let flags = (self.idle_muted, self.idle_deafened);
        self.call = Some(VoiceCall {
            origin,
            channel_id: channel_id.to_owned(),
            channel_name: channel_name.to_owned(),
            space_name: space_name.to_owned(),
            space_id: space_id.to_owned(),
            forced_mute: false,
            forced_deafen: false,
            muted: self.idle_muted,
            deafened: self.idle_deafened,
            token: None,
            confirmed: false,
            media_started: false,
            flags_synced: false,
        });
        if flags != (false, false)
            && let Some(call) = self.call.as_ref()
        {
            send_status(bridge, call);
        }
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
        if self.dm.is_some() {
            self.apply_dm(action, bridge);
            return;
        }
        if self.call.is_none() {
            match action {
                DockAction::ToggleMute => {
                    (self.idle_muted, self.idle_deafened) =
                        after_mute_toggle(self.idle_muted, self.idle_deafened);
                }
                DockAction::ToggleDeafen => {
                    (self.idle_muted, self.idle_deafened) = after_deafen_toggle(self.idle_deafened);
                }
                DockAction::Leave | DockAction::None => {}
            }
            return;
        }
        match action {
            DockAction::None => {}
            DockAction::Leave => self.leave(bridge),
            DockAction::ToggleMute => {
                if let Some(call) = self.call.as_mut()
                    && !call.forced_mute
                    && !call.forced_deafen
                {
                    (call.muted, call.deafened) = after_mute_toggle(call.muted, call.deafened);
                    send_status(bridge, call);
                    if let Some(media) = self.media.as_ref() {
                        media.set_flags(call.muted, call.deafened);
                    }
                }
            }
            DockAction::ToggleDeafen => {
                if let Some(call) = self.call.as_mut()
                    && !call.forced_deafen
                {
                    (call.muted, call.deafened) = after_deafen_toggle(call.deafened);
                    send_status(bridge, call);
                    if let Some(media) = self.media.as_ref() {
                        media.set_flags(call.muted, call.deafened);
                    }
                }
            }
        }
    }

    /// Играет звуки по изменениям в комнате и держит вызов, пока звонят.
    pub(crate) fn cue(&mut self, bridge: &mut Bridge, focused: bool) {
        let calling = self
            .dm
            .as_ref()
            .is_some_and(|dm| dm.phase == crate::dmcall::DmPhase::Ringing);
        let do_not_disturb = self.incoming.as_ref().is_some_and(|incoming| {
            bridge.cache_at(incoming.origin).is_some_and(|cache| {
                let me = cache.me();
                cache.user_status(&me.id).unwrap_or(me.status) == bs_types::UserStatus::DoNotDisturb
            })
        });
        self.cue_messages(bridge, focused);
        self.sounds.set_loop(crate::sounds::ringtone(
            self.incoming.is_some(),
            calling,
            do_not_disturb,
        ));

        let next = if let Some(dm) = &self.dm {
            // В личном звонке чужие входы и выходы — это сам звонок: без звука.
            crate::sounds::RoomView {
                in_room: dm.phase == crate::dmcall::DmPhase::Active,
                muted: dm.muted,
                deafened: dm.deafened,
                others: std::collections::BTreeSet::new(),
            }
        } else {
            self.call.as_ref().map_or_else(Default::default, |call| {
                let cache = bridge.cache_at(call.origin);
                let me = cache.map(|c| c.me().id.clone());
                crate::sounds::RoomView {
                    in_room: call.confirmed,
                    muted: call.muted,
                    deafened: call.deafened,
                    others: cache
                        .map(|c| {
                            c.users_in_voice(&call.channel_id)
                                .iter()
                                .filter(|id| Some(*id) != me.as_ref())
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            })
        };
        if next == self.room_view {
            return;
        }
        for sound in crate::sounds::cues(&self.room_view, &next) {
            self.sounds.play(sound);
        }
        self.room_view = next;
    }

    /// Звук и системное уведомление о новых сообщениях: звук один на кадр,
    /// как бы много их ни пришло. «Не беспокоить» гасит и то, и другое
    /// (свой статус берётся с того же инстанса, что и сообщение).
    fn cue_messages(&mut self, bridge: &mut Bridge, focused: bool) {
        let arrived = bridge.take_new_messages();
        let mut loud = false;
        for (origin, message) in &arrived {
            let Some(cache) = bridge.cache_at(*origin) else {
                continue;
            };
            if !message_alerts_user(cache, message) {
                continue;
            }
            let me = cache.me();
            if cache.user_status(&me.id).unwrap_or(me.status) == bs_types::UserStatus::DoNotDisturb
            {
                continue;
            }
            loud = true;
            crate::notify::message(cache, message, focused);
        }
        if loud {
            self.sounds.play(crate::sounds::Sound::Message);
        }
    }

    /// Разбирает ответы моста и следит, что сервер нас не отключил.
    pub(crate) fn poll(&mut self, bridge: &mut Bridge) {
        let mut dm_tokens: Vec<_> = Vec::new();
        for (origin, event) in bridge.take_voice_tokens() {
            dm_tokens.push((origin, event.channel_id.clone(), event.result.clone()));
            if let Some(call) = self.call.as_mut()
                && call.origin == origin
                && call.channel_id == event.channel_id
            {
                call.token = Some(event.result);
            }
        }
        self.poll_dm(bridge, &mut dm_tokens);
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
            let me = cache.me().id.clone();
            let moved = cache
                .all_channels()
                .find(|c| cache.users_in_voice(&c.id).contains(&me))
                .map(|c| (c.id.clone(), c.name.clone()));
            if let Some((id, name)) = moved {
                self.follow_move(bridge, &id, &name);
            } else {
                self.leave_quietly();
                self.notice = Some(tr("Вы отключены от голосового канала").to_owned());
                return;
            }
        }
        self.sync_forced(bridge);
        self.drive_media(bridge);
    }

    /// Нас переместили в другой голосовой канал: переподключаем медиа.
    fn follow_move(&mut self, bridge: &mut Bridge, channel_id: &str, name: &str) {
        let Some(call) = self.call.as_mut() else {
            return;
        };
        channel_id.clone_into(&mut call.channel_id);
        name.clone_into(&mut call.channel_name);
        call.token = None;
        call.media_started = false;
        call.flags_synced = false;
        let origin = call.origin;
        if let Some(media) = self.media.as_mut() {
            media.disconnect();
        }
        let previous = bridge.focus_index();
        bridge.focus_on(origin);
        bridge.request(BridgeRequest::VoiceToken(channel_id.to_owned()));
        bridge.focus_on(previous);
        self.notice = Some(crate::tf!("Вас переместили в «{name}»", name = name));
    }

    /// Применяет принудительные заглушки от модератора.
    fn sync_forced(&mut self, bridge: &mut Bridge) {
        let Some(call) = self.call.as_mut() else {
            return;
        };
        let Some(flags) = bridge
            .cache_at(call.origin)
            .and_then(|cache| cache.space_voice_state(&call.space_id))
            .copied()
        else {
            call.forced_mute = false;
            call.forced_deafen = false;
            return;
        };
        call.forced_mute = flags.space_muted || flags.permission_muted == Some(true);
        call.forced_deafen = flags.space_deafened;
        let want_muted = call.muted || call.forced_mute || call.forced_deafen;
        let want_deafened = call.deafened || call.forced_deafen;
        if (want_muted, want_deafened) != (call.muted, call.deafened) {
            call.muted = want_muted;
            call.deafened = want_deafened;
            send_status(bridge, call);
            call.flags_synced = false;
        }
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
        if self.status.is_live()
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

/// Панель над профилем: канал, состояние соединения и «Отключиться».
///
/// Микрофон и наушники живут в нижней панели профиля (как в оригинале), здесь
/// их нет — иначе кнопки дублируются.
pub(crate) fn show_dock(ctx_ui: &mut egui::Ui, state: &VoiceState) -> DockAction {
    let mut action = DockAction::None;
    let Some(info) = state.dock_info() else {
        return action;
    };
    egui::Panel::bottom("voice_dock")
        .exact_size(74.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(theme::bg_sidebar())
                .inner_margin(Margin::symmetric(10, 4)),
        )
        .show(ctx_ui, |ui| {
            egui::Frame::new()
                .fill(theme::bg_rail())
                .stroke(egui::Stroke::new(1.0, theme::border()))
                .corner_radius(egui::CornerRadius::same(12))
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            let status = ui.label(
                                RichText::new(info.status.0).size(12.5).color(info.status.1),
                            );
                            if let Some(detail) = &info.detail {
                                status.on_hover_text(detail);
                            }
                            ui.label(
                                theme::muted(theme::truncate_chars(&info.subtitle, 13)).size(11.5),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::icon_button(ui, Icon::HangUp, 30.0)
                                .on_hover_text(tr("Отключиться"))
                                .clicked()
                            {
                                action = DockAction::Leave;
                            }
                        });
                    });
                });
        });
    action
}

/// Что показать на панели.
struct DockInfo {
    status: (&'static str, egui::Color32),
    /// Подробности (причина сбоя, что делать) — во всплывающей подсказке.
    detail: Option<String>,
    subtitle: String,
}

impl VoiceState {
    fn dock_info(&self) -> Option<DockInfo> {
        if let Some(dm) = &self.dm {
            let status = match (dm.phase, &dm.token, &self.status) {
                (crate::dmcall::DmPhase::Ringing, _, _) => (tr("Вызываем…"), theme::amber()),
                (_, Some(Err(_)), _) => (tr("Нет токена LiveKit"), theme::rose()),
                (_, _, MediaStatus::Failed(_)) => (tr("Нет звукового соединения"), theme::rose()),
                (_, _, MediaStatus::ListenOnly(_)) => (tr("Звонок: нет микрофона"), theme::amber()),
                (_, _, MediaStatus::Live) => (tr("Звонок идёт"), theme::green()),
                (_, _, MediaStatus::Unavailable) => (tr("Сборка без голоса"), theme::text_muted()),
                _ => (tr("Подключение…"), theme::amber()),
            };
            return Some(DockInfo {
                status,
                detail: status_detail(dm.token.as_ref(), &self.status),
                subtitle: theme::truncate_chars(&dm.peer, 13),
            });
        }
        let call = self.call.as_ref()?;
        Some(DockInfo {
            status: status_line(call, &self.status),
            detail: status_detail(call.token.as_ref(), &self.status),
            subtitle: theme::truncate_chars(
                &format!("{} / {}", call.channel_name, call.space_name),
                13,
            ),
        })
    }
}

fn status_line(call: &VoiceCall, media: &MediaStatus) -> (&'static str, egui::Color32) {
    match (&call.token, media, call.confirmed) {
        _ if call.forced_deafen => (tr("Звук отключён модератором"), theme::amber()),
        _ if call.forced_mute => (tr("Вас заглушил модератор"), theme::amber()),
        (Some(Err(_)), _, _) => (tr("Нет токена LiveKit"), theme::rose()),
        (_, MediaStatus::Failed(_), _) => (tr("Нет звукового соединения"), theme::rose()),
        (_, MediaStatus::ListenOnly(_), _) => (tr("Нет микрофона"), theme::amber()),
        (_, MediaStatus::Live, _) => (tr("Голос подключён"), theme::green()),
        (_, MediaStatus::Unavailable, true) => (tr("Сборка без голоса"), theme::text_muted()),
        _ => (tr("Подключение…"), theme::amber()),
    }
}

/// Пояснение к статусу: почему так и что делать.
fn status_detail(
    token: Option<&Result<LiveKitTokenResponse, String>>,
    media: &MediaStatus,
) -> Option<String> {
    match (token, media) {
        (Some(Err(err)), _) => Some(crate::tf!(
            "Сервер не выдал токен LiveKit: {err}. Проверьте, что на инстансе настроен LiveKit.",
            err = err
        )),
        (_, MediaStatus::Failed(err)) => Some(crate::tf!(
            "Не удалось подключиться к LiveKit: {err}",
            err = err
        )),
        (_, MediaStatus::ListenOnly(err)) => Some(crate::tf!(
            "Вы слышите других, но вас не слышат: микрофон не открылся ({err}). \
             Проверьте, что микрофон подключён и не занят другой программой, \
             и выберите его в настройках голоса.",
            err = err
        )),
        (_, MediaStatus::Unavailable) => Some(
            tr("Приложение собрано без голосового движка. Запустите: \
             cargo run -p bs-app --features voice")
            .to_owned(),
        ),
        _ => None,
    }
}

/// Действие модератора над участником голоса.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModOp {
    /// Заглушить или снять заглушку на уровне пространства.
    Mute(String, bool),
    /// Отключить или вернуть звук на уровне пространства.
    Deafen(String, bool),
    /// Переместить в другой голосовой канал.
    Move(String, String),
    /// Отключить от голоса.
    Disconnect(String),
}

/// Меню модерации участника; сервер сам проверяет права.
pub(crate) fn moderation_menu(
    ui: &mut egui::Ui,
    user_id: &str,
    other_channels: &[(String, String)],
) -> Option<ModOp> {
    let uid = user_id.to_owned();
    let mut op = ui
        .button(tr("Заглушить микрофон (сервер)"))
        .clicked()
        .then(|| ModOp::Mute(uid.clone(), true));
    let mut pick = |clicked: bool, value: ModOp| {
        if clicked {
            op = Some(value);
        }
    };
    pick(
        ui.button(tr("Снять заглушку микрофона")).clicked(),
        ModOp::Mute(uid.clone(), false),
    );
    pick(
        ui.button(tr("Отключить звук (сервер)")).clicked(),
        ModOp::Deafen(uid.clone(), true),
    );
    pick(
        ui.button(tr("Вернуть звук")).clicked(),
        ModOp::Deafen(uid.clone(), false),
    );
    if !other_channels.is_empty() {
        ui.menu_button(tr("Переместить в…"), |ui| {
            for (id, name) in other_channels {
                pick(
                    ui.button(name).clicked(),
                    ModOp::Move(uid.clone(), id.clone()),
                );
            }
        });
    }
    pick(
        ui.button(tr("Отключить от канала")).clicked(),
        ModOp::Disconnect(uid),
    );
    op
}

/// Отправляет действие модерации на нужный инстанс.
pub(crate) fn moderate(bridge: &mut Bridge, origin: usize, op: ModOp) {
    let event = match op {
        ModOp::Mute(user_id, muted) => ClientEvent::VoiceSpaceMute { user_id, muted },
        ModOp::Deafen(user_id, deafened) => ClientEvent::VoiceSpaceDeafen { user_id, deafened },
        ModOp::Move(user_id, target_channel_id) => ClientEvent::VoiceMove {
            user_id,
            target_channel_id,
        },
        ModOp::Disconnect(user_id) => ClientEvent::VoiceDisconnect { user_id },
    };
    let previous = bridge.focus_index();
    bridge.focus_on(origin);
    bridge.send(event);
    bridge.focus_on(previous);
}

/// Нужно ли сообщить об этом сообщении (правило оригинала, см. `sounds`).
pub(crate) fn message_alerts_user(
    cache: &bs_core::Cache,
    message: &crate::bridge::NewMessage,
) -> bool {
    let me = cache.me();
    let own = message.author_id == me.id;
    if message.dm {
        return crate::sounds::message_alerts(&crate::sounds::MessageAlert {
            own,
            dm: true,
            muted: false,
            level: bs_types::NotificationLevel::All,
            direct: false,
            everyone: false,
            role: false,
            suppress_everyone: false,
            suppress_roles: false,
            all_channels: false,
        });
    }
    let Some(channel) = cache.channel(&message.channel_id) else {
        return false;
    };
    let policy = cache.channel_policy(
        &channel.space_id,
        &message.channel_id,
        crate::registry::now_ms(),
    );
    let my_roles: Vec<&str> = cache
        .space(&channel.space_id)
        .and_then(|space| space.members.iter().find(|m| m.user.id == me.id))
        .map(|member| member.roles.iter().map(|r| r.id.as_str()).collect())
        .unwrap_or_default();
    crate::sounds::message_alerts(&crate::sounds::MessageAlert {
        own,
        dm: false,
        muted: policy.muted,
        level: policy.level,
        direct: message.content.contains(&format!("<@{}>", me.id)),
        everyone: message.mention_everyone,
        role: message
            .mention_roles
            .iter()
            .any(|id| my_roles.contains(&id.as_str())),
        suppress_everyone: policy.suppress_everyone,
        suppress_roles: policy.suppress_roles,
        all_channels: crate::prefs::get().sounds.message_all_channels,
    })
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
