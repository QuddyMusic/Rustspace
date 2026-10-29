//! Состояние TUI: экраны, форма логина, кэши инстансов и подключение.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bs_core::{Cache, MultiCache};
use bs_instances::ConnectionState;
use bs_types::{ClientEvent, Origin, ServerEvent, TransportPolicy};
use bs_voice::VoiceEvent;
use bs_voice::protocol::{CallIdentity, ProtocolAction, VoiceCoordinator};

use crate::chat::ChatState;
use crate::login::LoginForm;
use crate::net::{NetEvent, NetPayload, NetRequest, NetResponse, NetWorker};
use crate::social::{self, SocialSelection, SocialState, SocialTab};
use crate::voice::{VoiceCtl, VoiceCtlEvent};

/// Какой экран сейчас показан.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    /// Ввод адреса, логина и пароля.
    #[default]
    Login,
    /// Список каналов и лента сообщений.
    Chat,
    /// Друзья, заявки, поиск и личные каналы.
    Social,
}

/// Состояние подключения с точки зрения интерфейса.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Status {
    /// Форма редактируется, сети ещё нет.
    #[default]
    Idle,
    /// Логин в процессе.
    Connecting,
    /// Подключились, события идут.
    Connected,
    /// Ошибка, требующая действия пользователя.
    Failed(String),
}

/// Статус для инстанса, о котором ничего не известно.
static IDLE: Status = Status::Idle;

/// Корневое состояние приложения.
///
/// Кэшей несколько: по одному на каждый подключённый инстанс. Активный
/// инстанс задаёт, что видно на экране, но события приходят от всех сразу.
pub struct App {
    screen: Screen,
    running: bool,
    form: LoginForm,
    worker: Option<NetWorker>,
    multi: MultiCache,
    active: Origin,
    transport: TransportPolicy,
    states: HashMap<Origin, Status>,
    chat: ChatState,
    social: SocialState,
    voice: Option<VoiceCtl>,
    coord: VoiceCoordinator,
    started: Instant,
    me_id: String,
    me_name: String,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("screen", &self.screen)
            .field("running", &self.running)
            .field("form", &self.form)
            .field("worker", &self.worker.is_some())
            .field("instances", &self.multi.len())
            .field("active", &self.active)
            .field("transport", &self.transport)
            .field("states", &self.states)
            .field("chat", &self.chat)
            .field("social", &self.social)
            .field("voice", &self.voice.is_some())
            .field("coord", &self.coord)
            .field("started", &self.started)
            .field("me_id", &self.me_id)
            .field("me_name", &self.me_name)
            .finish()
    }
}

impl App {
    /// Создаёт приложение на экране логина.
    pub fn new() -> Self {
        Self {
            screen: Screen::Login,
            running: true,
            form: LoginForm::new(),
            worker: None,
            multi: MultiCache::new(),
            active: Origin::Home,
            transport: TransportPolicy::Strict,
            states: HashMap::new(),
            chat: ChatState::new(),
            social: SocialState::new(),
            voice: None,
            coord: VoiceCoordinator::new(),
            started: Instant::now(),
            me_id: String::new(),
            me_name: String::new(),
        }
    }

    /// Разрешает открытый `http://` вне loopback (явное согласие пользователя).
    pub fn allow_insecure(&mut self) {
        self.transport = TransportPolicy::AllowInsecure;
    }

    /// Текущий экран.
    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// Форма логина.
    pub fn form(&self) -> &LoginForm {
        &self.form
    }

    /// Форма логина для правки.
    pub fn form_mut(&mut self) -> &mut LoginForm {
        &mut self.form
    }

    /// Инстанс, на который смотрит интерфейс.
    pub fn active(&self) -> &Origin {
        &self.active
    }

    /// Все известные инстансы: домашний первым, затем удалённые.
    pub fn instances(&self) -> Vec<Origin> {
        self.multi.origins()
    }

    /// Статус конкретного инстанса.
    pub fn status_of(&self, origin: &Origin) -> &Status {
        self.states.get(origin).unwrap_or(&IDLE)
    }

    /// Состояние подключения активного инстанса.
    pub fn status(&self) -> &Status {
        self.status_of(&self.active)
    }

    /// Кэш активного инстанса; `None` до первого `ready`.
    pub fn cache(&self) -> Option<&Cache> {
        self.multi.cache(&self.active)
    }

    /// Все кэши (для сводок и отладки).
    pub fn multi(&self) -> &MultiCache {
        &self.multi
    }

    /// Выбор канала и композер.
    pub fn chat(&self) -> &ChatState {
        &self.chat
    }

    /// Выбор канала и композер для правки.
    pub fn chat_mut(&mut self) -> &mut ChatState {
        &mut self.chat
    }

    /// Состояние социального экрана.
    pub fn social(&self) -> &SocialState {
        &self.social
    }

    /// Состояние социального экрана для правки.
    pub fn social_mut(&mut self) -> &mut SocialState {
        &mut self.social
    }

    /// Можно ли сейчас начинать логин.
    pub fn can_submit(&self) -> bool {
        self.form.is_submittable()
    }

    /// Запускает логин в фоне. Возвращает `false`, если это неуместно.
    pub fn submit_login(&mut self) -> bool {
        if !self.can_submit() {
            return false;
        }
        let Some(creds) = self.form.credentials() else {
            return false;
        };
        // Один инстанс за раз: пока есть сессия или идёт вход, второй не стартует.
        if self.has_instances() || self.status() == &Status::Connecting {
            return false;
        }
        let origin = Origin::Home;
        let worker = self.worker.get_or_insert_with(NetWorker::spawn);
        if !worker.connect(origin.clone(), creds, self.transport) {
            return false;
        }
        self.states.insert(origin.clone(), Status::Connecting);
        self.active = origin;
        self.screen = Screen::Chat;
        true
    }

    /// Подключён ли хотя бы один инстанс.
    pub fn has_instances(&self) -> bool {
        !self.multi.is_empty()
    }

    /// Общие действия после смены инстанса: пересчёт курсоров и подгрузка.
    fn after_switch(&mut self) {
        let active = self.active.clone();
        self.chat.follow(&active);
        self.chat.clamp_multi(&self.multi);
        if let Some(cache) = self.multi.cache(&self.active) {
            self.social.clamp(cache);
        }
        self.screen = Screen::Chat;
        self.refresh_social();
    }

    /// Отключается от активного инстанса и убирает его из списка.
    pub fn disconnect_active(&mut self) {
        if !self.multi.contains(&self.active) && self.multi.is_empty() {
            return;
        }
        let origin = self.active.clone();
        self.multi.remove(&origin);
        self.states.remove(&origin);
        if let Some(worker) = self.worker.as_ref() {
            worker.disconnect();
        }
        if let Some(next) = self.multi.origins().first().cloned() {
            self.active = next;
            self.after_switch();
        } else {
            self.active = Origin::Home;
            self.screen = Screen::Login;
            self.chat = ChatState::new();
            self.social = SocialState::new();
        }
    }

    /// Забирает накопившиеся события сети и применяет их к состоянию.
    pub fn pump_network(&mut self) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        let events = worker.drain();
        let responses = worker.drain_responses();
        for event in events {
            self.apply_net_event(event);
        }
        for response in responses {
            self.apply_net_response(response);
        }
        self.pump_voice();
    }

    /// Применяет ответ фоновой сессии на REST-запрос.
    pub fn apply_net_response(&mut self, response: NetResponse) {
        if response.origin != self.active {
            // Ответ от фонового инстанса не должен подменять экран.
            return;
        }
        match response.payload {
            NetPayload::Friends(_) | NetPayload::FriendRequests(_) | NetPayload::DmChannels(_) => {
                if let Some(cache) = self.multi.cache(&self.active) {
                    self.social.clamp(cache);
                }
            }
            NetPayload::Users(users) => {
                self.social.set_search_results(users);
                self.social.set_message(None);
            }
            NetPayload::DmChannel(channel) => {
                self.social
                    .set_message(Some(format!("личный канал: {}", channel.id)));
            }
            NetPayload::LiveKitToken(token) => {
                self.start_voice_session(token.url, token.token);
            }
            NetPayload::Done => {
                self.social.set_message(None);
                self.refresh_social();
            }
            NetPayload::Failed(message) => {
                self.social.set_message(Some(message));
            }
        }
    }

    /// Применяет одно событие фоновой сессии.
    #[allow(clippy::too_many_lines)]
    pub fn apply_net_event(&mut self, event: NetEvent) {
        match event {
            NetEvent::Ready { origin, event } => {
                if let ServerEvent::Ready(payload) = *event {
                    self.me_id.clone_from(&payload.user.id);
                    self.me_name = payload
                        .user
                        .display_name
                        .clone()
                        .unwrap_or_else(|| payload.user.username.clone());
                    self.multi.insert_ready(origin.clone(), *payload);
                }
                self.states.insert(origin.clone(), Status::Connected);
                self.active = origin.clone();
                self.screen = Screen::Chat;
                self.chat.follow(&origin);
                self.chat.clamp_multi(&self.multi);
                if let Some(cache) = self.multi.cache(&self.active) {
                    self.social.clamp(cache);
                }
                self.refresh_social();
            }
            NetEvent::Server { origin, event } => {
                if let ServerEvent::Ready(payload) = *event {
                    // Повторный `ready` (переподключение): пересобираем кэш.
                    self.me_id.clone_from(&payload.user.id);
                    self.me_name = payload
                        .user
                        .display_name
                        .clone()
                        .unwrap_or_else(|| payload.user.username.clone());
                    self.multi.insert_ready(origin.clone(), *payload);
                    self.states.insert(origin.clone(), Status::Connected);
                } else {
                    match *event {
                        ServerEvent::DmCallIncoming {
                            dm_channel_id,
                            federated_call_id,
                            caller_id,
                            caller_name,
                            livekit_url,
                            livekit_token,
                            ..
                        } => {
                            self.on_call_incoming(
                                origin.as_web_str(),
                                dm_channel_id,
                                federated_call_id,
                                caller_id,
                                caller_name,
                                livekit_url,
                                livekit_token,
                            );
                        }
                        ServerEvent::DmCallAccepted {
                            dm_channel_id,
                            federated_call_id,
                            ..
                        } => {
                            let identity = CallIdentity {
                                dm_channel_id,
                                federated_call_id,
                                origin: Some(origin.as_web_str()),
                            };
                            self.coord.server_accepted(&identity);
                        }
                        ServerEvent::DmCallRejected {
                            dm_channel_id,
                            federated_call_id,
                        }
                        | ServerEvent::DmCallEnded {
                            dm_channel_id,
                            federated_call_id,
                        } => {
                            let identity = CallIdentity {
                                dm_channel_id,
                                federated_call_id,
                                origin: Some(origin.as_web_str()),
                            };
                            self.call_ended(&identity);
                        }
                        ServerEvent::DmCallUndeliverable {
                            dm_channel_id,
                            federated_call_id,
                            ..
                        } => {
                            let identity = CallIdentity {
                                dm_channel_id,
                                federated_call_id: Some(federated_call_id),
                                origin: Some(origin.as_web_str()),
                            };
                            self.call_ended(&identity);
                        }
                        other => {
                            if self.multi.apply(&origin, other).is_accepted()
                                && origin == self.active
                                && let Some(cache) = self.multi.cache(&self.active)
                            {
                                self.chat.clamp_multi(&self.multi);
                                self.social.clamp(cache);
                            }
                        }
                    }
                }
            }
            NetEvent::State { origin, state } => {
                let status = match state {
                    ConnectionState::Connecting | ConnectionState::Reconnecting { .. } => {
                        Status::Connecting
                    }
                    ConnectionState::Connected => Status::Connected,
                    ConnectionState::Stopped => Status::Failed("соединение остановлено".to_owned()),
                };
                self.states.insert(origin, status);
            }
            NetEvent::Disconnected { origin } => {
                self.multi.remove(&origin);
                self.states.remove(&origin);
                if origin == self.active {
                    if let Some(next) = self.multi.origins().first().cloned() {
                        self.active = next;
                        self.after_switch();
                    } else {
                        self.active = Origin::Home;
                        self.screen = Screen::Login;
                    }
                }
            }
            NetEvent::Failed { origin, message } => {
                self.states.insert(origin, Status::Failed(message));
            }
        }
    }

    /// Переключает микрофон: обновляет координатор и шлёт команду исполнителю.
    pub fn toggle_mute(&mut self) {
        let Some(voice) = self.voice.as_ref() else {
            self.social
                .set_message(Some("голос: нет активного сеанса".to_owned()));
            return;
        };
        let muted = !self.coord.is_muted();
        let deafened = self.coord.is_deafened();
        self.coord.set_mute_state(muted, deafened);
        voice.set_muted(muted);
        self.social.set_message(Some(format!(
            "микрофон: {}",
            if muted {
                "выключен"
            } else {
                "включён"
            }
        )));
    }

    /// Переключает глушение звука: обновляет координатор и шлёт команду исполнителю.
    pub fn toggle_deafen(&mut self) {
        let Some(voice) = self.voice.as_ref() else {
            self.social
                .set_message(Some("голос: нет активного сеанса".to_owned()));
            return;
        };
        let deafened = !self.coord.is_deafened();
        let muted = self.coord.is_muted();
        self.coord.set_mute_state(muted, deafened);
        voice.set_deafened(deafened);
        self.social.set_message(Some(format!(
            "звук: {}",
            if deafened {
                "заглушён"
            } else {
                "включён"
            }
        )));
    }

    /// Забирает события голосового исполнителя и тикает координатор звонков.
    fn pump_voice(&mut self) {
        let now = self.started.elapsed();
        let events = match self.voice.as_ref() {
            Some(voice) => voice.drain(),
            None => return,
        };
        for event in events {
            match event {
                VoiceCtlEvent::Connected => {
                    self.social
                        .set_message(Some("голос: подключено к комнате".to_owned()));
                }
                VoiceCtlEvent::ConnectFailed { message } => {
                    self.coord.clear();
                    self.social
                        .set_message(Some(format!("голос: ошибка подключения: {message}")));
                }
                VoiceCtlEvent::Disconnected => {
                    self.coord.clear();
                    self.social.set_message(Some("голос: отключено".to_owned()));
                }
                VoiceCtlEvent::Voice(voice_event) => self.apply_voice_event(voice_event, now),
            }
        }
        let actions = self.coord.tick(now);
        for action in actions {
            self.apply_protocol_action(action);
        }
    }

    /// Реагирует на события комнаты LiveKit.
    fn apply_voice_event(&mut self, event: VoiceEvent, now: Duration) {
        match event {
            VoiceEvent::ParticipantJoined { name, .. } => {
                self.social
                    .set_message(Some(format!("голос: {name} в комнате")));
            }
            VoiceEvent::ParticipantLeft { .. } => {
                self.social
                    .set_message(Some("голос: участник вышел".to_owned()));
            }
            VoiceEvent::ActiveSpeakers { identities } => {
                if let Some(first) = identities.first() {
                    self.social.set_message(Some(format!("говорит: {first}")));
                }
            }
            VoiceEvent::MuteChanged { identity, muted } => {
                if identity == self.me_id {
                    let deafened = self.coord.is_deafened();
                    self.coord.set_mute_state(muted, deafened);
                }
            }
            VoiceEvent::Reconnecting | VoiceEvent::Disconnected { .. } => {
                self.coord.transport_lost(now);
            }
            VoiceEvent::Reconnected => {
                self.coord.rebind();
            }
            VoiceEvent::Connected { .. }
            | VoiceEvent::TrackSubscribed { .. }
            | VoiceEvent::TrackUnsubscribed { .. } => {}
        }
    }

    /// Исполняет решение протокола звонков через сетевого воркера.
    fn apply_protocol_action(&mut self, action: ProtocolAction) {
        match action {
            ProtocolAction::SendAccept(identity) => {
                self.send_event(ClientEvent::DmCallAccept {
                    dm_channel_id: identity.dm_channel_id,
                    federated_call_id: identity.federated_call_id,
                });
            }
            ProtocolAction::SendReject(identity) => {
                self.send_event(ClientEvent::DmCallReject {
                    dm_channel_id: identity.dm_channel_id,
                    federated_call_id: identity.federated_call_id,
                });
            }
            ProtocolAction::SendEnd(identity) | ProtocolAction::IncomingTimedOut(identity) => {
                self.send_event(ClientEvent::DmCallEnd {
                    dm_channel_id: identity.dm_channel_id,
                    federated_call_id: identity.federated_call_id,
                });
            }
            ProtocolAction::RebindExpired(identity) => {
                self.call_ended(&identity);
            }
            ProtocolAction::IgnoreStaleEvent => {}
        }
    }

    /// Входящий звонок: регистрируем в координаторе и сразу подключаемся
    /// к комнате LiveKit, если токен уже в событии.
    #[allow(clippy::too_many_arguments)]
    fn on_call_incoming(
        &mut self,
        origin: String,
        dm_channel_id: Option<String>,
        federated_call_id: Option<String>,
        caller_id: String,
        caller_name: String,
        livekit_url: Option<String>,
        livekit_token: Option<String>,
    ) {
        let identity = CallIdentity {
            dm_channel_id,
            federated_call_id,
            origin: Some(origin),
        };
        let now = self.started.elapsed();
        if !self
            .coord
            .incoming_call(identity, caller_id, caller_name, now)
        {
            return;
        }
        if let (Some(url), Some(token)) = (livekit_url, livekit_token) {
            self.start_voice_session(url, token);
        }
    }

    /// Сервер подтвердил завершение звонка: чистим координатор и комнату.
    fn call_ended(&mut self, identity: &CallIdentity) {
        // Чужое или устаревшее событие не должно отключать текущую комнату.
        if self.coord.server_ended(identity) {
            self.hangup_voice();
        }
    }

    /// Кладёт трубку: рвём LiveKit-сессию, если она была.
    fn hangup_voice(&self) {
        if let Some(voice) = self.voice.as_ref() {
            voice.disconnect();
        }
    }

    /// Подключает голосовую сессию к комнате LiveKit по готовому токену.
    fn start_voice_session(&mut self, url: String, token: String) {
        if self.me_id.is_empty() {
            return;
        }
        let voice = self.voice.get_or_insert_with(VoiceCtl::spawn);
        voice.connect(url, token, self.me_id.clone(), self.me_name.clone());
    }

    /// Отправляет набранный текст в текущий канал.
    ///
    /// В режиме правки вместо нового сообщения уходит `message_edit`.
    /// Возвращает `false`, если отправлять некуда или нечего.
    pub fn send_composer(&mut self) -> bool {
        if self.cache().is_none() {
            return false;
        }
        let editing = self.chat.editing_message_id().map(str::to_owned);
        let Some(content) = self.chat.take_composer() else {
            return false;
        };
        let event = if let Some(message_id) = editing {
            self.chat.finish_edit();
            ClientEvent::MessageEdit {
                message_id,
                content,
            }
        } else {
            let Some(channel_id) = self.current_channel_id() else {
                return false;
            };
            ClientEvent::MessageCreate {
                channel_id,
                content,
                reply_to_id: None,
            }
        };
        self.send_event(event)
    }

    /// id текущего канала по кэшу.
    fn current_channel_id(&self) -> Option<String> {
        let cache = self.cache()?;
        self.chat.current_channel_id(cache)
    }

    /// Отправляет событие в фоновую сессию активного инстанса.
    fn send_event(&self, event: ClientEvent) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| worker.send(event))
    }

    /// Сдвигает курсор по ленте сообщений текущего канала.
    pub fn nav_message(&mut self, delta: isize) {
        let active = self.active.clone();
        let Some(cache) = self.multi.cache(&active) else {
            return;
        };
        self.chat.move_selection(cache, delta);
    }

    /// Начинает правку выбранного своего сообщения.
    ///
    /// Возвращает `false`, если сообщение не выбрано или написано не нами.
    pub fn edit_selected(&mut self) -> bool {
        let Some(cache) = self.cache() else {
            return false;
        };
        let Some(message) = self.chat.selected_message(cache) else {
            return false;
        };
        if message.user.id != cache.me().id {
            return false;
        }
        let message_id = message.message.id.clone();
        let content = message.message.content.clone().unwrap_or_default();
        self.chat.begin_edit(message_id, content);
        true
    }

    /// Отменяет режим правки, если он активен.
    pub fn cancel_edit(&mut self) {
        self.chat.cancel_edit();
    }

    /// Удаляет выбранное своё сообщение.
    ///
    /// Возвращает `false`, если сообщение не выбрано или написано не нами.
    pub fn delete_selected(&mut self) -> bool {
        let Some(cache) = self.cache() else {
            return false;
        };
        let Some(message) = self.chat.selected_message(cache) else {
            return false;
        };
        if message.user.id != cache.me().id {
            return false;
        }
        let message_id = message.message.id.clone();
        if self.chat.editing_message_id() == Some(message_id.as_str()) {
            self.chat.cancel_edit();
        }
        self.send_event(ClientEvent::MessageDelete { message_id })
    }

    /// Ставит реакцию на выбранное сообщение.
    pub fn react_selected(&mut self, emoji: &str) -> bool {
        let Some(message_id) = self.selected_message_id() else {
            return false;
        };
        self.send_event(ClientEvent::ReactionAdd {
            message_id,
            emoji: emoji.to_owned(),
        })
    }

    /// Убирает реакцию с выбранного сообщения.
    pub fn unreact_selected(&mut self, emoji: &str) -> bool {
        let Some(message_id) = self.selected_message_id() else {
            return false;
        };
        self.send_event(ClientEvent::ReactionRemove {
            message_id,
            emoji: emoji.to_owned(),
        })
    }

    /// id выбранного сообщения в текущем канале.
    fn selected_message_id(&self) -> Option<String> {
        let cache = self.cache()?;
        self.chat
            .selected_message(cache)
            .map(|message| message.message.id.clone())
    }

    /// Подтверждает прочтение текущего канала до последнего сообщения.
    ///
    /// Повторный вызов для той же пары «канал, сообщение» ничего не шлёт.
    pub fn ack_current_channel(&mut self) -> bool {
        let Some(cache) = self.cache() else {
            return false;
        };
        let Some(channel_id) = self.chat.current_channel_id(cache) else {
            return false;
        };
        let Some(last) = cache.messages(&channel_id).last() else {
            return false;
        };
        let message_id = last.message.id.clone();
        if !self.chat.should_ack(&channel_id, &message_id) {
            return false;
        }
        self.send_event(ClientEvent::ChannelAck {
            channel_id,
            message_id,
        })
    }

    /// Посылает `typing_start`, если в этом канале ещё не посылали.
    pub fn notify_typing(&mut self) {
        let Some(cache) = self.cache() else {
            return;
        };
        let Some(channel_id) = self.chat.current_channel_id(cache) else {
            return;
        };
        if !self.chat.should_send_typing(&channel_id) {
            return;
        }
        if let Some(worker) = self.worker.as_ref() {
            worker.send(ClientEvent::TypingStart { channel_id });
        }
    }

    /// Сдвигает выбор канала на `delta`.
    pub fn nav_channel(&mut self, delta: isize) {
        self.chat.move_channel_multi(&self.multi, delta);
        self.sync_active();
        self.ack_current_channel();
    }

    /// Сдвигает выбор пространства на `delta`.
    pub fn nav_space(&mut self, delta: isize) {
        self.chat.move_space_multi(&self.multi, delta);
        self.sync_active();
        self.ack_current_channel();
    }

    /// Держит `active` в согласии с инстансом, который выбран в сайдбаре.
    ///
    /// Объединённый сайдбар позволяет уйти на пространство другого инстанса
    /// стрелками — тогда меняется и `active`, и все запросы/события уходят
    /// уже туда.
    fn sync_active(&mut self) {
        let origin = self.chat.origin().clone();
        if origin != self.active && self.multi.contains(&origin) {
            self.active = origin;
            if let Some(cache) = self.multi.cache(&self.active) {
                self.social.clamp(cache);
            }
            self.refresh_social();
        }
    }

    /// Отправляет REST-запрос фоновой сессии.
    fn request(&self, request: NetRequest) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| worker.request(&self.active, request))
    }

    /// Перезапрашивает друзей и заявки.
    fn refresh_social(&self) {
        let _ = self.request(NetRequest::Friends);
        let _ = self.request(NetRequest::FriendRequests);
    }

    /// Открывает социальный экран и подгружает данные вкладки.
    pub fn open_social(&mut self) {
        self.screen = Screen::Social;
        self.load_tab();
    }

    /// Возвращает с чата на социальный экран.
    pub fn close_social(&mut self) {
        self.screen = Screen::Chat;
    }

    /// Запрашивает данные для активной вкладки.
    pub fn load_tab(&self) {
        match self.social.tab() {
            SocialTab::Friends => {
                let _ = self.request(NetRequest::Friends);
            }
            SocialTab::Requests => {
                let _ = self.request(NetRequest::FriendRequests);
            }
            SocialTab::Dms => {
                let _ = self.request(NetRequest::DmChannels);
            }
            SocialTab::Search => self.run_search(),
        }
    }

    /// Переключает вкладку на следующую и подгружает её данные.
    pub fn social_next_tab(&mut self) {
        self.social.next_tab();
        self.load_tab();
    }

    /// Переключает вкладку на предыдущую и подгружает её данные.
    pub fn social_prev_tab(&mut self) {
        self.social.prev_tab();
        self.load_tab();
    }

    /// Сдвигает курсор активного списка.
    pub fn social_move(&mut self, delta: isize) {
        {
            let active = self.active.clone();
            let Some(cache) = self.multi.cache(&active) else {
                return;
            };
            self.social.move_cursor(cache, delta);
        }
    }

    /// Запускает поиск по текущей строке запроса.
    pub fn run_search(&self) {
        let query = self.social.query().trim();
        if query.is_empty() {
            return;
        }
        let _ = self.request(NetRequest::SearchUsers(query.to_owned()));
    }

    /// Ввод символа в строку поиска.
    pub fn social_push_char(&mut self, c: char) {
        self.social.push_query(c);
    }

    /// Удаление символа из строки поиска.
    pub fn social_backspace(&mut self) {
        self.social.backspace_query();
    }

    /// Что сейчас выбрано на социальном экране.
    ///
    /// Возвращает пару «вид элемента, его id» — этого хватает, чтобы
    /// отличить действие с другом от действия с заявкой.
    pub fn social_selection(&self) -> Option<SocialSelection> {
        let cache = self.cache()?;
        let cursor = self.social.cursor();
        match self.social.tab() {
            SocialTab::Friends => social::friend_rows(cache)
                .get(cursor)
                .map(|row| SocialSelection::Friend(row.id.clone())),
            SocialTab::Requests => social::request_rows(cache).get(cursor).map(|row| {
                if row.incoming {
                    SocialSelection::IncomingRequest(row.id.clone())
                } else {
                    SocialSelection::OutgoingRequest(row.id.clone())
                }
            }),
            SocialTab::Search => social::user_rows(cache, self.social.search_results())
                .get(cursor)
                .map(|row| SocialSelection::User {
                    id: row.id.clone(),
                    friend: row.friend,
                }),
            SocialTab::Dms => social::dm_rows(cache)
                .get(cursor)
                .map(|row| SocialSelection::Dm(row.id.clone())),
        }
    }

    /// Открывает личный канал с выбранным другом или пользователем.
    pub fn social_open_dm(&mut self) -> bool {
        let request = match self.social_selection() {
            Some(SocialSelection::Friend(id) | SocialSelection::User { id, .. }) => {
                bs_api::CreateDmRequest::user(id)
            }
            _ => return false,
        };
        self.request(NetRequest::OpenDm(request))
    }

    /// Шлёт заявку в друзья выбранному пользователю из поиска.
    ///
    /// Для уже добавленного в друзья пользователя и для чужих вкладок
    /// возвращает `false`.
    pub fn social_send_request(&mut self) -> bool {
        match self.social_selection() {
            Some(SocialSelection::User { id, friend: false }) => self.request(
                NetRequest::SendFriendRequest(bs_api::SendFriendRequest::username(id)),
            ),
            _ => false,
        }
    }

    /// Принимает выбранную входящую заявку.
    pub fn social_accept_request(&mut self) -> bool {
        self.respond_selected(bs_api::FriendRequestDecision::Accept)
    }

    /// Отклоняет выбранную входящую заявку.
    pub fn social_decline_request(&mut self) -> bool {
        self.respond_selected(bs_api::FriendRequestDecision::Decline)
    }

    fn respond_selected(&self, decision: bs_api::FriendRequestDecision) -> bool {
        match self.social_selection() {
            Some(SocialSelection::IncomingRequest(id)) => {
                self.request(NetRequest::RespondFriendRequest(id, decision))
            }
            _ => false,
        }
    }

    /// Отзывает выбранную исходящую заявку.
    pub fn social_cancel_request(&mut self) -> bool {
        match self.social_selection() {
            Some(SocialSelection::OutgoingRequest(id)) => {
                self.request(NetRequest::CancelFriendRequest(id))
            }
            _ => false,
        }
    }

    /// Удаляет выбранного друга.
    pub fn social_remove_friend(&mut self) -> bool {
        match self.social_selection() {
            Some(SocialSelection::Friend(id)) => self.request(NetRequest::RemoveFriend(id)),
            _ => false,
        }
    }

    /// Отключается от всех инстансов и возвращает на экран логина.
    pub fn logout(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.join();
        }
        self.multi = MultiCache::new();
        self.states.clear();
        self.active = Origin::Home;
        self.screen = Screen::Login;
    }

    /// Пора ли выходить из цикла событий.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Просит цикл событий завершиться.
    pub fn quit(&mut self) {
        self.running = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_instance_status_is_idle() {
        let app = App::new();
        assert_eq!(app.status(), &Status::Idle);
        assert_eq!(app.status_of(&Origin::Home), &Status::Idle);
    }

    #[test]
    fn login_is_refused_while_connecting() {
        let mut app = App::new();
        app.form_mut().focus_next();
        "alice".chars().for_each(|c| app.form_mut().push_char(c));
        app.form_mut().focus_next();
        "secret".chars().for_each(|c| app.form_mut().push_char(c));
        assert!(app.can_submit(), "форма заполнена");
        app.states.insert(Origin::Home, Status::Connecting);
        assert!(
            !app.submit_login(),
            "второй вход во время первого невозможен"
        );
        assert!(app.worker.is_none(), "воркер не поднимался");
    }

    #[test]
    fn insecure_transport_is_opt_in() {
        let mut app = App::new();
        assert_eq!(app.transport, TransportPolicy::Strict);
        app.allow_insecure();
        assert_eq!(app.transport, TransportPolicy::AllowInsecure);
    }
}
