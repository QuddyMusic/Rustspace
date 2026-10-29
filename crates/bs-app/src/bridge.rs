//! Мост GUI ↔ tokio.
//!
//! Разделение обязанностей:
//! - фоновая tokio-задача держит [`InstanceHandle`] и читает события;
//! - GUI-поток владеет [`Cache`] и применяет к нему [`apply`] прямо в кадре.
//!
//! Так GUI никогда не блокируется на сети, а `Cache` (не `Clone`) не
//! приходится копировать между потоками.
//!
//! # Одна сессия за раз
//! Вход в инстанс — это переход `Idle → Connecting → Active`. Новый вход
//! возможен только из `Idle`, а выход (`Active → Closing`) прерывает
//! фоновую задачу и ждёт её фактического завершения (событие `Ended`), и
//! лишь потом возвращает мост в `Idle`. Поэтому два инстанса одновременно
//! не живут: ни повторным нажатием «Войти», ни входом в другой инстанс
//! сразу после выхода из прежнего.

use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;

use bs_core::{Cache, apply};
use bs_identity::CredentialStore;
use bs_instances::{ConnectionState, InstanceEvent, InstanceHandle, ReconnectPolicy};
use bs_types::{
    ClientEvent, ClientKind, DmMessageWithUser, MessageWithUser, Origin, ReadyPayload, ServerEvent,
    TransportPolicy,
};
use tokio::runtime::Handle;
use tokio::sync::{broadcast, mpsc as tokio_mpsc};
use zeroize::Zeroizing;

use crate::logic::describe_server_error;

/// Сколько сообщений просим у сервера за один запрос истории.
pub(crate) const HISTORY_PAGE: u16 = 50;

/// Вид канала: у каналов пространств и личных каналов разные события,
/// разные REST-маршруты и разные хранилища в кэше.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ChannelKind {
    /// Текстовый канал пространства.
    Space,
    /// Личный канал (1-на-1 или группа).
    Dm,
}

/// Состояние загрузки истории одного канала.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HistoryState {
    /// Страница запрошена, ответа ещё нет.
    pub(crate) loading: bool,
    /// Сервер вернул неполную страницу: старее ничего нет.
    pub(crate) exhausted: bool,
    /// Ошибка последнего запроса.
    pub(crate) error: Option<String>,
}

impl HistoryState {
    /// Можно ли запрашивать следующую страницу автоматически (при прокрутке).
    ///
    /// После ошибки автоповтора нет: иначе при сбое (например, `429`) клиент
    /// каждый кадр шлёт новый запрос и сам себя блокирует. Повтор — только по
    /// кнопке «Повторить».
    pub(crate) const fn can_auto_load(&self) -> bool {
        !self.loading && !self.exhausted && self.error.is_none()
    }
}

/// Страница истории из REST-ответа.
#[derive(Debug)]
pub(crate) enum HistoryItems {
    /// Сообщения канала пространства.
    Space(Vec<MessageWithUser>),
    /// Сообщения личного канала.
    Dm(Vec<DmMessageWithUser>),
}

impl HistoryItems {
    fn len(&self) -> usize {
        match self {
            Self::Space(items) => items.len(),
            Self::Dm(items) => items.len(),
        }
    }
}

/// REST-запрос, который GUI отправляет фоновой сессии.
#[derive(Debug, Clone)]
pub(crate) enum BridgeRequest {
    /// Список друзей.
    Friends,
    /// Список заявок в друзья.
    FriendRequests,
    /// Поиск пользователей по подстроке.
    SearchUsers(String),
    /// Список личных каналов.
    DmChannels,
    /// Открыть (или создать) личный канал с пользователем.
    OpenDm(String),
    /// Отправить заявку в друзья по имени пользователя.
    SendFriendRequest(String),
    /// Принять или отклонить входящую заявку.
    RespondFriendRequest(String, bool),
    /// Отозвать свою заявку.
    CancelFriendRequest(String),
    /// Удалить друга.
    RemoveFriend(String),
    /// Страница истории: самая новая или старее `before`.
    History {
        /// Канал или личный канал.
        channel_id: String,
        /// Какой это вид канала.
        kind: ChannelKind,
        /// Курсор «старее этого id»; `None` — самые новые.
        before: Option<String>,
    },
}

/// Ответ фоновой сессии на [`BridgeRequest`].
#[derive(Debug)]
pub(crate) enum BridgeResponse {
    /// Пришли друзья.
    Friends(Vec<bs_types::Friend>),
    /// Пришли заявки в друзья.
    FriendRequests(Vec<bs_types::FriendRequest>),
    /// Пришли результаты поиска пользователей.
    Users(Vec<bs_types::User>),
    /// Пришли личные каналы.
    DmChannels(Vec<bs_types::DmChannel>),
    /// Открыт личный канал.
    DmChannel(Box<bs_types::DmChannel>),
    /// Пришла страница истории.
    History {
        /// Канал, чью историю запрашивали.
        channel_id: String,
        /// Сами сообщения.
        items: HistoryItems,
    },
    /// История не загрузилась.
    HistoryFailed {
        /// Канал, чью историю запрашивали.
        channel_id: String,
        /// Текст ошибки.
        message: String,
    },
    /// Запись прошла успешно.
    Done,
    /// Запрос не удался.
    Failed(String),
}

/// Событие из фоновой задачи в GUI-поток.
#[derive(Debug)]
enum BridgeEvent {
    /// Первый снимок состояния инстанса.
    Ready {
        /// Сам снимок.
        payload: Box<ReadyPayload>,
    },
    /// Дельта состояния инстанса.
    Server {
        /// Само событие.
        event: Box<ServerEvent>,
    },
    /// Ошибка логина или соединения.
    Error {
        /// Текст ошибки.
        message: String,
    },
    /// Сменилось состояние соединения.
    State {
        /// Новое состояние.
        state: ConnectionState,
    },
    /// Поток событий инстанса закрыт.
    Disconnected,
    /// Фоновая задача сессии завершилась: штатно, по ошибке, после `abort`
    /// или паники. Это последнее событие сессии.
    Ended,
}

/// Присылает [`BridgeEvent::Ended`] при уничтожении, чем бы ни закончилась
/// задача сессии: возвратом, `abort()` или паникой.
struct EndGuard(std_mpsc::Sender<BridgeEvent>);

impl Drop for EndGuard {
    fn drop(&mut self) {
        let _ = self.0.send(BridgeEvent::Ended);
    }
}

/// Фаза жизненного цикла сессии.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Сессии нет: можно входить.
    Idle,
    /// Идёт логин и первое подключение.
    Connecting,
    /// Сессия работает.
    Active,
    /// Выход: задача прервана, ждём её завершения.
    Closing,
}

/// Мост GUI ↔ tokio.
#[derive(Debug)]
pub(crate) struct Bridge {
    runtime: Handle,
    phase: Phase,
    events: Option<std_mpsc::Receiver<BridgeEvent>>,
    /// Канал команд (WebSocket-события) живой сессии.
    commands: Option<tokio_mpsc::UnboundedSender<ClientEvent>>,
    /// Канал REST-запросов живой сессии.
    requests: Option<tokio_mpsc::UnboundedSender<BridgeRequest>>,
    responses: Option<std_mpsc::Receiver<BridgeResponse>>,
    /// Фоновая задача сессии: её `abort()` и есть выход.
    session: Option<tokio::task::JoinHandle<()>>,
    friends: Vec<bs_types::Friend>,
    friend_requests: Vec<bs_types::FriendRequest>,
    dm_channels: Vec<bs_types::DmChannel>,
    search_results: Vec<bs_types::User>,
    social_error: Option<String>,
    /// Канал, только что открытый по запросу `OpenDm` (нужно переключить чат).
    opened_dm: Option<String>,
    /// Загрузка истории по id канала.
    history: HashMap<String, HistoryState>,
    /// Кэш инстанса; `None` до первого `ready`.
    cache: Option<Cache>,
    ready: bool,
    /// Потеряна ли живая сессия: GUI должен вернуться на логин.
    session_lost: bool,
    /// Последнее известное состояние соединения.
    connection: ConnectionState,
    last_error: Option<String>,
    /// Отказ сервера на действие пользователя (например, `rate_limited`).
    notice: Option<String>,
}

/// Приводит адрес инстанса к абсолютному URL.
///
/// Пользователь обычно вводит только хост (`backspace.example`), поэтому
/// схема `https://` подставляется автоматически. Явно указанная схема
/// (`http://`, `ws://` и т.п.) сохраняется как есть, а допустимость
/// открытого текста решает [`TransportPolicy`].
fn normalize_base(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    Some(with_scheme)
}

impl Bridge {
    /// Создаёт мост над данным runtime.
    pub(crate) fn new(runtime: Handle) -> Self {
        Self {
            runtime,
            phase: Phase::Idle,
            events: None,
            commands: None,
            requests: None,
            responses: None,
            session: None,
            friends: Vec::new(),
            friend_requests: Vec::new(),
            dm_channels: Vec::new(),
            search_results: Vec::new(),
            social_error: None,
            opened_dm: None,
            history: HashMap::new(),
            cache: None,
            ready: false,
            session_lost: false,
            connection: ConnectionState::Connecting,
            last_error: None,
            notice: None,
        }
    }

    /// Текущая фаза сессии (нужна только тестам).
    #[cfg(test)]
    pub(crate) const fn phase(&self) -> Phase {
        self.phase
    }

    /// Забирает накопленные события из канала (без блокировки).
    pub(crate) fn poll(&mut self) {
        loop {
            let next = self.events.as_ref().map(std_mpsc::Receiver::try_recv);
            match next {
                Some(Ok(event)) => self.handle_event(event),
                Some(Err(std_mpsc::TryRecvError::Empty | std_mpsc::TryRecvError::Disconnected))
                | None => break,
            }
        }
        loop {
            let next = self.responses.as_ref().map(std_mpsc::Receiver::try_recv);
            match next {
                Some(Ok(response)) => self.handle_response(response),
                Some(Err(std_mpsc::TryRecvError::Empty | std_mpsc::TryRecvError::Disconnected))
                | None => break,
            }
        }
    }

    fn handle_event(&mut self, event: BridgeEvent) {
        match event {
            BridgeEvent::Ready { payload } => {
                self.cache = Some(Cache::from_ready(*payload));
                self.history.clear();
                self.phase = Phase::Active;
                self.connection = ConnectionState::Connected;
                self.ready = true;
                self.session_lost = false;
                self.last_error = None;
            }
            BridgeEvent::Server { event } => self.apply_server_event(*event),
            BridgeEvent::State { state } => self.connection = state,
            BridgeEvent::Error { message } => self.last_error = Some(message),
            BridgeEvent::Disconnected => {
                self.connection = ConnectionState::Stopped;
                self.last_error = Some("соединение закрыто".to_owned());
            }
            BridgeEvent::Ended => self.finish_session(),
        }
    }

    /// Применяет событие сервера к кэшу.
    fn apply_server_event(&mut self, event: ServerEvent) {
        match &event {
            // Отказ на действие пользователя: без этого сообщение, которое
            // сервер не принял, просто молча пропадало.
            ServerEvent::Error { message, code, .. } => {
                self.notice = Some(describe_server_error(code.as_deref(), message));
                return;
            }
            // `ready` после реконнекта пересобирает кэш без сообщений:
            // историю надо загрузить заново.
            ServerEvent::Ready(_) => self.history.clear(),
            _ => {}
        }
        if let Some(cache) = self.cache.as_mut() {
            let _ = apply(cache, event);
        }
    }

    /// Фоновая задача завершилась: сессия окончательно закрыта.
    fn finish_session(&mut self) {
        let was = self.phase;
        self.session = None;
        self.commands = None;
        self.requests = None;
        self.events = None;
        self.responses = None;
        self.cache = None;
        self.history.clear();
        self.friends.clear();
        self.friend_requests.clear();
        self.dm_channels.clear();
        self.search_results.clear();
        self.social_error = None;
        self.opened_dm = None;
        self.notice = None;
        self.ready = false;
        self.connection = ConnectionState::Connecting;
        self.phase = Phase::Idle;
        // Отказ логина оставляет пользователя на форме; потеря живой сессии
        // и выход возвращают на неё.
        self.session_lost = matches!(was, Phase::Active | Phase::Closing);
        if was == Phase::Active && self.last_error.is_none() {
            self.last_error = Some("соединение закрыто".to_owned());
        }
    }

    /// `true` ровно один раз — когда пришёл первый `ready`.
    pub(crate) fn take_ready(&mut self) -> bool {
        std::mem::take(&mut self.ready)
    }

    /// Идёт ли логин или выход: форму и кнопки надо заблокировать.
    pub(crate) const fn is_busy(&self) -> bool {
        matches!(self.phase, Phase::Connecting | Phase::Closing)
    }

    /// Последняя ошибка, если была.
    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Отказ сервера на последнее действие (показывается над лентой).
    pub(crate) fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// Скрывает уведомление.
    pub(crate) fn dismiss_notice(&mut self) {
        self.notice = None;
    }

    /// Кэш инстанса; `None` до первого `ready`.
    pub(crate) const fn cache(&self) -> Option<&Cache> {
        self.cache.as_ref()
    }

    /// Текущее состояние соединения (для строки статуса).
    pub(crate) const fn connection(&self) -> &ConnectionState {
        &self.connection
    }

    /// `true` ровно один раз — если сессия закрыта и надо вернуться на логин.
    pub(crate) fn take_session_lost(&mut self) -> bool {
        std::mem::take(&mut self.session_lost)
    }

    /// Запускает логин в фоне.
    ///
    /// Работает только из фазы [`Phase::Idle`]: пока прежняя сессия не
    /// закрыта до конца, второй вход (в тот же или другой инстанс) отклоняется.
    pub(crate) fn start_login(
        &mut self,
        base: &str,
        username: &str,
        password: &str,
        transport: TransportPolicy,
    ) {
        if self.phase != Phase::Idle {
            self.last_error = Some("Сначала выйдите из текущего инстанса".to_owned());
            return;
        }
        let Some(base) = normalize_base(base) else {
            self.last_error = Some("заполните адрес, логин и пароль".to_owned());
            return;
        };
        if username.trim().is_empty() || password.is_empty() {
            self.last_error = Some("заполните адрес, логин и пароль".to_owned());
            return;
        }
        // Проверяем адрес до сети: пользователь сразу видит причину отказа,
        // а пароль не уходит ни в какое соединение.
        match url::Url::parse(&base) {
            Ok(parsed) => {
                if let Err(err) = transport.check(&parsed) {
                    self.last_error = Some(err.to_string());
                    return;
                }
            }
            Err(err) => {
                self.last_error = Some(format!("некорректный адрес: {err}"));
                return;
            }
        }

        self.phase = Phase::Connecting;
        self.last_error = None;
        self.session_lost = false;
        self.connection = ConnectionState::Connecting;

        let (events_tx, events_rx) = std_mpsc::channel();
        let (commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
        let (requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
        let (responses_tx, responses_rx) = std_mpsc::channel();
        self.events = Some(events_rx);
        self.commands = Some(commands_tx);
        self.requests = Some(requests_tx);
        self.responses = Some(responses_rx);

        let params = SessionParams {
            base,
            username: username.trim().to_owned(),
            password: Zeroizing::new(password.to_owned()),
            transport,
        };
        self.session = Some(self.runtime.spawn(run_session(
            params,
            events_tx,
            commands_rx,
            requests_rx,
            responses_tx,
        )));
    }

    /// Выходит из инстанса: прерывает фоновую сессию.
    ///
    /// Мост остаётся занятым ([`Phase::Closing`]) до события `Ended`, так что
    /// новый вход невозможен, пока старое соединение не закрыто полностью.
    pub(crate) fn logout(&mut self) {
        if !matches!(self.phase, Phase::Connecting | Phase::Active) {
            return;
        }
        self.phase = Phase::Closing;
        self.commands = None;
        self.requests = None;
        if let Some(session) = self.session.as_ref() {
            session.abort();
        }
    }

    /// Отправляет сообщение в канал или личный канал (в фоне).
    ///
    /// Для ЛС сервер ждёт отдельное событие `dm_message_create`: обычный
    /// `message_create` с id личного канала он молча отбрасывает.
    pub(crate) fn send_message(&self, kind: ChannelKind, channel_id: String, content: String) {
        self.send(match kind {
            ChannelKind::Space => ClientEvent::MessageCreate {
                channel_id,
                content,
                reply_to_id: None,
            },
            ChannelKind::Dm => ClientEvent::DmMessageCreate {
                dm_channel_id: channel_id,
                content: Some(content),
                attachments: None,
                reply_to_id: None,
            },
        });
    }

    /// Отправляет команду серверу, если сессия жива.
    pub(crate) fn send(&self, event: ClientEvent) {
        if let Some(commands) = self.commands.as_ref() {
            let _ = commands.send(event);
        }
    }

    /// Правит своё сообщение.
    pub(crate) fn edit_message(&self, kind: ChannelKind, message_id: String, content: String) {
        self.send(match kind {
            ChannelKind::Space => ClientEvent::MessageEdit {
                message_id,
                content,
            },
            ChannelKind::Dm => ClientEvent::DmMessageEdit {
                message_id,
                content,
            },
        });
    }

    /// Удаляет своё сообщение.
    pub(crate) fn delete_message(&self, kind: ChannelKind, message_id: String) {
        self.send(match kind {
            ChannelKind::Space => ClientEvent::MessageDelete { message_id },
            ChannelKind::Dm => ClientEvent::DmMessageDelete { message_id },
        });
    }

    /// Ставит реакцию.
    pub(crate) fn add_reaction(&self, message_id: String, emoji: String) {
        self.send(ClientEvent::ReactionAdd { message_id, emoji });
    }

    /// Убирает реакцию.
    pub(crate) fn remove_reaction(&self, message_id: String, emoji: String) {
        self.send(ClientEvent::ReactionRemove { message_id, emoji });
    }

    /// Подтверждает прочтение канала до сообщения.
    pub(crate) fn ack_channel(&self, channel_id: String, message_id: String) {
        self.send(ClientEvent::ChannelAck {
            channel_id,
            message_id,
        });
    }

    /// Отправляет REST-запрос; `false`, если сессии нет.
    pub(crate) fn request(&self, request: BridgeRequest) -> bool {
        self.requests
            .as_ref()
            .is_some_and(|requests| requests.send(request).is_ok())
    }

    /// Последние полученные друзья.
    pub(crate) fn friends(&self) -> &[bs_types::Friend] {
        &self.friends
    }

    /// Последние полученные заявки.
    pub(crate) fn friend_requests(&self) -> &[bs_types::FriendRequest] {
        &self.friend_requests
    }

    /// Последние полученные личные каналы.
    pub(crate) fn dm_channels(&self) -> &[bs_types::DmChannel] {
        &self.dm_channels
    }

    /// Последние результаты поиска пользователей.
    pub(crate) fn search_results(&self) -> &[bs_types::User] {
        &self.search_results
    }

    /// `true` ровно один раз — если только что открыли личный канал.
    pub(crate) fn take_opened_dm(&mut self) -> Option<String> {
        self.opened_dm.take()
    }

    /// Ошибка последнего REST-запроса.
    pub(crate) fn social_error(&self) -> Option<&str> {
        self.social_error.as_deref()
    }

    /// Состояние загрузки истории канала.
    pub(crate) fn history_state(&self, channel_id: &str) -> Option<&HistoryState> {
        self.history.get(channel_id)
    }

    /// Просит самые новые сообщения канала, если ещё не просили.
    ///
    /// Безопасно вызывать каждый кадр: повторный запрос не уходит, пока
    /// состояние канала известно.
    pub(crate) fn ensure_history(&mut self, kind: ChannelKind, channel_id: &str) {
        if self.history.contains_key(channel_id) {
            return;
        }
        let request = BridgeRequest::History {
            channel_id: channel_id.to_owned(),
            kind,
            before: None,
        };
        if self.request(request) {
            self.history.insert(
                channel_id.to_owned(),
                HistoryState {
                    loading: true,
                    ..HistoryState::default()
                },
            );
        }
    }

    /// Подгружает страницу старее уже загруженного (или повторяет первую
    /// страницу после ошибки).
    pub(crate) fn load_older(&mut self, kind: ChannelKind, channel_id: &str) {
        let Some(state) = self.history.get(channel_id) else {
            return;
        };
        if state.loading || state.exhausted {
            return;
        }
        let oldest = self.cache.as_ref().and_then(|cache| match kind {
            ChannelKind::Space => cache
                .messages(channel_id)
                .first()
                .map(|m| m.message.id.clone()),
            ChannelKind::Dm => cache
                .dm_messages(channel_id)
                .first()
                .map(|m| m.message.id.clone()),
        });
        if oldest.is_none() && state.error.is_none() {
            return;
        }
        let request = BridgeRequest::History {
            channel_id: channel_id.to_owned(),
            kind,
            before: oldest,
        };
        if self.request(request)
            && let Some(state) = self.history.get_mut(channel_id)
        {
            state.loading = true;
            state.error = None;
        }
    }

    /// Вливает страницу истории в кэш и обновляет состояние канала.
    fn apply_history(&mut self, channel_id: &str, items: HistoryItems) {
        let Some(state) = self.history.get_mut(channel_id) else {
            // Пришла после `ready`, который уже сбросил кэш и состояние.
            return;
        };
        state.loading = false;
        state.error = None;
        state.exhausted = items.len() < usize::from(HISTORY_PAGE);
        let Some(cache) = self.cache.as_mut() else {
            return;
        };
        match items {
            HistoryItems::Space(page) => {
                cache.merge_history(channel_id, page);
            }
            HistoryItems::Dm(page) => {
                cache.merge_dm_history(channel_id, page);
            }
        }
    }

    /// Применяет ответ REST-запроса.
    fn handle_response(&mut self, response: BridgeResponse) {
        match response {
            BridgeResponse::History { channel_id, items } => {
                self.apply_history(&channel_id, items);
            }
            BridgeResponse::HistoryFailed {
                channel_id,
                message,
            } => {
                if let Some(state) = self.history.get_mut(&channel_id) {
                    state.loading = false;
                    state.error = Some(message);
                }
            }
            BridgeResponse::Friends(friends) => {
                self.friends = friends;
                self.social_error = None;
            }
            BridgeResponse::FriendRequests(requests) => {
                self.friend_requests = requests;
                self.social_error = None;
            }
            BridgeResponse::Users(users) => {
                self.search_results = users;
                self.social_error = None;
            }
            BridgeResponse::DmChannels(channels) => {
                self.dm_channels = channels;
                self.social_error = None;
            }
            BridgeResponse::DmChannel(channel) => {
                let id = channel.id.clone();
                self.opened_dm = Some(id.clone());
                // Сервер не обязан присылать создателю `dm_channel_created`,
                // а сайдбар и лента читают личные каналы из кэша.
                if let Some(cache) = self.cache.as_mut() {
                    cache.upsert_dm_channel((*channel).clone());
                }
                if !self.dm_channels.iter().any(|known| known.id == id) {
                    self.dm_channels.push(*channel);
                }
                self.social_error = None;
            }
            BridgeResponse::Done => {
                self.social_error = None;
                self.request(BridgeRequest::Friends);
                self.request(BridgeRequest::FriendRequests);
                self.request(BridgeRequest::DmChannels);
            }
            BridgeResponse::Failed(message) => self.social_error = Some(message),
        }
    }
}

/// Параметры входа, переданные фоновой сессии.
struct SessionParams {
    base: String,
    username: String,
    /// Пароль затирается в памяти, как только логин выполнен.
    password: Zeroizing<String>,
    transport: TransportPolicy,
}

/// Фоновая сессия: логин, затем пересылка событий и команд.
async fn run_session(
    params: SessionParams,
    events: std_mpsc::Sender<BridgeEvent>,
    mut commands: tokio_mpsc::UnboundedReceiver<ClientEvent>,
    mut requests: tokio_mpsc::UnboundedReceiver<BridgeRequest>,
    responses: std_mpsc::Sender<BridgeResponse>,
) {
    // Сработает при любом выходе из функции, включая `abort()` и панику.
    let _end = EndGuard(events.clone());

    let SessionParams {
        base,
        username,
        password,
        transport,
    } = params;
    let mut store = CredentialStore::new();
    let login = InstanceHandle::login_with_transport(
        &mut store,
        Origin::Home,
        &base,
        &username,
        &password,
        Some(ClientKind::Desktop),
        ReconnectPolicy::default(),
        transport,
    )
    .await;
    drop(password);

    let (handle, ready) = match login {
        Ok(pair) => pair,
        Err(err) => {
            let _ = events.send(BridgeEvent::Error {
                message: err.to_string(),
            });
            return;
        }
    };

    match ready {
        ServerEvent::Ready(payload) => {
            if events.send(BridgeEvent::Ready { payload }).is_err() {
                return;
            }
        }
        other => {
            let _ = events.send(BridgeEvent::Error {
                message: format!("ожидался ready, пришло {other:?}"),
            });
            return;
        }
    }

    let mut sub = handle.subscribe();
    loop {
        tokio::select! {
            event = sub.recv() => match event {
                Ok(InstanceEvent::Server(boxed)) => {
                    if events.send(BridgeEvent::Server { event: boxed }).is_err() {
                        return;
                    }
                }
                Ok(InstanceEvent::AuthRejected { failure, message, .. }) => {
                    let _ = events.send(BridgeEvent::Error {
                        message: format!("{message} ({failure:?})"),
                    });
                }
                Ok(InstanceEvent::StateChanged(state)) => {
                    if events.send(BridgeEvent::State { state }).is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "пропущены события инстанса");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    let _ = events.send(BridgeEvent::Disconnected);
                    return;
                }
            },
            request = requests.recv() => match request {
                Some(request) => {
                    let response = handle_request(handle.client(), request).await;
                    if responses.send(response).is_err() {
                        return;
                    }
                }
                None => return,
            },
            command = commands.recv() => match command {
                Some(event) => {
                    if let Err(err) = handle.send(event).await {
                        let _ = events.send(BridgeEvent::Error { message: err.to_string() });
                    }
                }
                None => return,
            },
        }
    }
}

/// Выполняет REST-запрос и превращает результат в [`BridgeResponse`].
async fn handle_request(client: &bs_api::ApiClient, request: BridgeRequest) -> BridgeResponse {
    match request {
        BridgeRequest::Friends => match client.friends().await {
            Ok(friends) => BridgeResponse::Friends(friends),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::FriendRequests => match client.friend_requests().await {
            Ok(requests) => BridgeResponse::FriendRequests(requests),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::SearchUsers(query) => match client.search_users(&query).await {
            Ok(users) => BridgeResponse::Users(users),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::DmChannels => match client.dm_channels().await {
            Ok(channels) => BridgeResponse::DmChannels(channels),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::OpenDm(user_id) => {
            let request = bs_api::CreateDmRequest::user(user_id);
            match client.open_dm(&request).await {
                Ok(channel) => BridgeResponse::DmChannel(Box::new(channel)),
                Err(err) => BridgeResponse::Failed(err.to_string()),
            }
        }
        BridgeRequest::SendFriendRequest(username) => {
            let request = bs_api::SendFriendRequest::username(username);
            match client.send_friend_request(&request).await {
                Ok(_) => BridgeResponse::Done,
                Err(err) => BridgeResponse::Failed(err.to_string()),
            }
        }
        BridgeRequest::RespondFriendRequest(id, accept) => {
            let decision = if accept {
                bs_api::FriendRequestDecision::Accept
            } else {
                bs_api::FriendRequestDecision::Decline
            };
            match client.respond_friend_request(&id, decision).await {
                Ok(()) => BridgeResponse::Done,
                Err(err) => BridgeResponse::Failed(err.to_string()),
            }
        }
        BridgeRequest::CancelFriendRequest(id) => match client.cancel_friend_request(&id).await {
            Ok(()) => BridgeResponse::Done,
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::RemoveFriend(user_id) => match client.remove_friend(&user_id).await {
            Ok(()) => BridgeResponse::Done,
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::History {
            channel_id,
            kind,
            before,
        } => {
            let cursor = before.map(bs_api::PagingCursor::Before);
            let items = match kind {
                ChannelKind::Space => client
                    .messages(&channel_id, cursor, Some(HISTORY_PAGE))
                    .await
                    .map(|page| HistoryItems::Space(page.messages)),
                ChannelKind::Dm => client
                    .dm_messages(&channel_id, cursor, Some(HISTORY_PAGE))
                    .await
                    .map(|page| HistoryItems::Dm(page.messages)),
            };
            match items {
                Ok(items) => BridgeResponse::History { channel_id, items },
                Err(err) => BridgeResponse::HistoryFailed {
                    channel_id,
                    message: err.to_string(),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bs_instances::ConnectionState;
    use bs_types::{ClientEvent, DmMessageWithUser, ReadyPayload, ServerEvent, TransportPolicy};
    use tokio::sync::mpsc as tokio_mpsc;

    use super::{
        Bridge, BridgeEvent, BridgeRequest, BridgeResponse, ChannelKind, HISTORY_PAGE,
        HistoryItems, Phase, SessionParams, normalize_base, run_session, std_mpsc,
    };

    /// Минимальный `ready`, достаточный для создания кэша.
    #[allow(clippy::expect_used, reason = "в тестах паника допустима")]
    fn ready_payload() -> ReadyPayload {
        serde_json::from_value(serde_json::json!({
            "user": {
                "id": "123456789012345678",
                "username": "alice",
                "displayName": null,
                "avatar": null,
                "banner": null,
                "accentColor": null,
                "avatarColor": "mint",
                "bio": null,
                "status": "online",
                "customStatus": null,
                "isAdmin": false,
                "createdAt": 1_750_000_000_000_i64,
                "homeInstance": null,
                "homeUserId": null,
                "replicatedInstances": []
            }
        }))
        .expect("ready payload должен разбираться")
    }

    /// Пустой runtime нужен только как держатель `Handle`; задачи в этих
    /// тестах не запускаются, сети нет.
    #[allow(clippy::expect_used, reason = "в тестах паника допустима")]
    fn new_bridge() -> Bridge {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let handle = runtime.handle().clone();
        // Runtime намеренно утекает: `Handle` должен оставаться валидным
        // на всё время теста, а дропать его вручную ради этого не нужно.
        std::mem::forget(runtime);
        Bridge::new(handle)
    }

    /// Мост с живой сессией: кэш и каналы есть, сети нет.
    fn active_bridge() -> (
        Bridge,
        tokio_mpsc::UnboundedReceiver<ClientEvent>,
        tokio_mpsc::UnboundedReceiver<BridgeRequest>,
    ) {
        let mut bridge = new_bridge();
        let (commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
        let (requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
        bridge.commands = Some(commands_tx);
        bridge.requests = Some(requests_tx);
        bridge.phase = Phase::Connecting;
        bridge.handle_event(BridgeEvent::Ready {
            payload: Box::new(ready_payload()),
        });
        (bridge, commands_rx, requests_rx)
    }

    fn params() -> SessionParams {
        SessionParams {
            base: "http://127.0.0.1:1".to_owned(),
            username: "alice".to_owned(),
            password: zeroize::Zeroizing::new("secret".to_owned()),
            transport: TransportPolicy::Strict,
        }
    }

    // ─── Жизненный цикл сессии ───────────────────────────────────────────

    #[test]
    fn ready_activates_session_and_clears_errors() {
        let mut bridge = new_bridge();
        bridge.phase = Phase::Connecting;
        bridge.last_error = Some("старое".to_owned());
        bridge.handle_event(BridgeEvent::Ready {
            payload: Box::new(ready_payload()),
        });
        assert_eq!(bridge.phase(), Phase::Active);
        assert_eq!(bridge.last_error(), None);
        assert!(bridge.cache().is_some());
        assert_eq!(bridge.connection(), &ConnectionState::Connected);
        assert!(!bridge.is_busy());
    }

    #[test]
    fn take_ready_is_one_shot() {
        let (mut bridge, _commands, _requests) = active_bridge();
        assert!(bridge.take_ready());
        assert!(!bridge.take_ready());
    }

    #[test]
    fn state_event_updates_connection() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.handle_event(BridgeEvent::State {
            state: ConnectionState::Reconnecting { attempt: 2 },
        });
        assert_eq!(
            bridge.connection(),
            &ConnectionState::Reconnecting { attempt: 2 }
        );
        assert!(!bridge.take_session_lost());
    }

    #[test]
    fn failed_login_returns_to_idle_without_session_loss() {
        let mut bridge = new_bridge();
        bridge.phase = Phase::Connecting;
        bridge.handle_event(BridgeEvent::Error {
            message: "неверный пароль".to_owned(),
        });
        bridge.handle_event(BridgeEvent::Ended);
        assert_eq!(bridge.phase(), Phase::Idle);
        assert!(
            !bridge.take_session_lost(),
            "пользователь остаётся на форме"
        );
        assert_eq!(bridge.last_error(), Some("неверный пароль"));
    }

    #[test]
    fn dropped_active_session_is_reported_as_lost() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.handle_event(BridgeEvent::Disconnected);
        assert_eq!(bridge.connection(), &ConnectionState::Stopped);
        bridge.handle_event(BridgeEvent::Ended);
        assert_eq!(bridge.phase(), Phase::Idle);
        assert!(bridge.take_session_lost());
        assert!(!bridge.take_session_lost(), "флаг одноразовый");
        assert_eq!(bridge.last_error(), Some("соединение закрыто"));
        assert!(bridge.cache().is_none(), "кэш чужой сессии не остаётся");
    }

    #[test]
    fn ended_clears_all_session_state() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        bridge.friends = Vec::new();
        bridge.notice = Some("отказ".to_owned());
        bridge.handle_event(BridgeEvent::Ended);
        assert!(bridge.cache().is_none());
        assert!(bridge.history_state("dm1").is_none());
        assert!(bridge.commands.is_none() && bridge.requests.is_none());
        assert!(bridge.notice().is_none());
        assert_eq!(bridge.connection(), &ConnectionState::Connecting);
    }

    // ─── Одна сессия за раз ──────────────────────────────────────────────

    #[test]
    fn start_login_rejects_empty_fields() {
        let cases = [
            ("", "user", "pass"),
            ("   ", "user", "pass"),
            ("https://example.com", "", "pass"),
            ("https://example.com", "user", ""),
        ];
        for (base, user, pass) in cases {
            let mut bridge = new_bridge();
            bridge.start_login(base, user, pass, TransportPolicy::Strict);
            assert_eq!(bridge.phase(), Phase::Idle, "base={base:?} user={user:?}");
            assert_eq!(bridge.last_error(), Some("заполните адрес, логин и пароль"));
            assert!(bridge.events.is_none() && bridge.commands.is_none());
        }
    }

    #[test]
    fn start_login_accepts_host_without_scheme() {
        let mut bridge = new_bridge();
        bridge.start_login("backspace.example", "user", "pass", TransportPolicy::Strict);
        assert_eq!(bridge.phase(), Phase::Connecting);
        assert!(bridge.is_busy());
        assert_eq!(bridge.last_error(), None);
        assert!(bridge.events.is_some() && bridge.commands.is_some());
        assert!(bridge.session.is_some());
    }

    #[test]
    fn start_login_refuses_plaintext_http_without_consent() {
        for base in ["http://chat.example.com", "http://192.168.1.10:3005"] {
            let mut bridge = new_bridge();
            bridge.start_login(base, "user", "pass", TransportPolicy::Strict);
            assert_eq!(bridge.phase(), Phase::Idle, "{base}");
            assert!(
                bridge
                    .last_error()
                    .is_some_and(|e| e.contains("без шифрования")),
                "{base}: {:?}",
                bridge.last_error()
            );
            assert!(bridge.session.is_none(), "пароль не должен уйти в сеть");
        }
    }

    #[test]
    fn start_login_allows_loopback_and_explicit_consent() {
        let mut bridge = new_bridge();
        bridge.start_login("http://localhost:3005", "u", "p", TransportPolicy::Strict);
        assert_eq!(bridge.phase(), Phase::Connecting);

        let mut bridge = new_bridge();
        bridge.start_login(
            "http://192.168.1.10:3005",
            "u",
            "p",
            TransportPolicy::AllowInsecure,
        );
        assert_eq!(bridge.phase(), Phase::Connecting);
    }

    #[test]
    fn second_login_is_refused_in_every_busy_phase() {
        for phase in [Phase::Connecting, Phase::Active, Phase::Closing] {
            let mut bridge = new_bridge();
            bridge.phase = phase;
            bridge.start_login("https://other.example", "u", "p", TransportPolicy::Strict);
            assert_eq!(bridge.phase(), phase, "{phase:?}");
            assert_eq!(
                bridge.last_error(),
                Some("Сначала выйдите из текущего инстанса")
            );
            assert!(bridge.events.is_none(), "второй сессии не создано");
            assert!(bridge.session.is_none());
        }
    }

    #[test]
    fn login_is_possible_again_only_after_the_old_session_ended() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.logout();
        assert_eq!(bridge.phase(), Phase::Closing);
        assert!(bridge.is_busy());
        bridge.start_login("https://other.example", "u", "p", TransportPolicy::Strict);
        assert_eq!(bridge.phase(), Phase::Closing, "старое ещё закрывается");

        bridge.handle_event(BridgeEvent::Ended);
        assert_eq!(bridge.phase(), Phase::Idle);
        assert!(
            bridge.take_session_lost(),
            "выход возвращает на форму входа"
        );
        bridge.start_login("https://other.example", "u", "p", TransportPolicy::Strict);
        assert_eq!(bridge.phase(), Phase::Connecting);
    }

    #[test]
    fn logout_closes_channels_and_aborts_the_task() {
        let mut bridge = new_bridge();
        bridge.start_login("backspace.example", "user", "pass", TransportPolicy::Strict);
        bridge.logout();
        assert_eq!(bridge.phase(), Phase::Closing);
        assert!(bridge.commands.is_none() && bridge.requests.is_none());
        // Команды после выхода никуда не уходят.
        bridge.send_message(ChannelKind::Space, "c".to_owned(), "x".to_owned());
        assert!(!bridge.request(BridgeRequest::Friends));
        // Повторный выход безвреден.
        bridge.logout();
        assert_eq!(bridge.phase(), Phase::Closing);
    }

    #[test]
    fn logout_without_session_is_noop() {
        let mut bridge = new_bridge();
        bridge.logout();
        assert_eq!(bridge.phase(), Phase::Idle);
        assert!(!bridge.take_session_lost());
    }

    #[test]
    fn normalize_base_adds_https_scheme() {
        assert_eq!(
            normalize_base("example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            normalize_base("  example.com  ").as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn normalize_base_keeps_explicit_scheme() {
        assert_eq!(
            normalize_base("http://localhost:3000").as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            normalize_base("https://example.com").as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn normalize_base_rejects_empty() {
        assert_eq!(normalize_base(""), None);
        assert_eq!(normalize_base("   "), None);
    }

    // ─── Фоновая сессия ──────────────────────────────────────────────────

    /// Ошибка HTTP-логина доезжает до GUI, затем приходит `Ended`.
    #[tokio::test]
    #[allow(clippy::expect_used, reason = "в тестах паника допустима")]
    async fn run_session_reports_http_login_error_then_ends() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/auth/login"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": "Invalid credentials",
                "code": "invalid_credentials"
            })))
            .mount(&server)
            .await;

        let (events_tx, events_rx) = std_mpsc::channel();
        let (_commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
        let (_requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
        let (responses_tx, _responses_rx) = std_mpsc::channel();

        let mut p = params();
        p.base = server.uri();
        run_session(p, events_tx, commands_rx, requests_rx, responses_tx).await;

        let event = events_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("run_session должен прислать событие об ошибке");
        let BridgeEvent::Error { message } = event else {
            unreachable!("ожидалась ошибка, пришло {event:?}");
        };
        assert!(message.contains("401"), "текст ошибки: {message}");
        assert!(message.contains("Invalid credentials"), "{message}");
        assert!(
            matches!(events_rx.try_recv(), Ok(BridgeEvent::Ended)),
            "последним событием сессии всегда `Ended`"
        );
        assert!(events_rx.try_recv().is_err());
    }

    /// Полный happy-path: логин, `auth`, `ready`, команда доходит до сервера.
    #[tokio::test]
    #[allow(clippy::expect_used, reason = "в тестах паника допустима")]
    async fn run_session_happy_path_relays_commands() {
        let ready = ServerEvent::Ready(Box::new(ready_payload()));
        let (base, mut server_events) = crate::fake_instance::spawn(ready).await;

        let (events_tx, events_rx) = std_mpsc::channel();
        let (commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
        let (_requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
        let (responses_tx, _responses_rx) = std_mpsc::channel();

        let mut p = params();
        p.base = base;
        let session = tokio::spawn(run_session(
            p,
            events_tx,
            commands_rx,
            requests_rx,
            responses_tx,
        ));

        let event = tokio::task::spawn_blocking({
            let events_rx = events_rx;
            move || events_rx.recv_timeout(Duration::from_secs(10))
        })
        .await
        .expect("join");
        assert!(
            matches!(&event, Ok(BridgeEvent::Ready { .. })),
            "ожидался ready, пришло {event:?}"
        );

        commands_tx
            .send(ClientEvent::MessageCreate {
                channel_id: "chan-1".to_owned(),
                content: "привет".to_owned(),
                reply_to_id: None,
            })
            .expect("команда должна уйти в канал");

        // Heartbeat шлёт `ping` в произвольный момент — пропускаем его.
        let received = loop {
            let event = tokio::time::timeout(Duration::from_secs(10), server_events.recv())
                .await
                .expect("сервер должен получить событие")
                .expect("канал событий сервера не должен закрыться");
            if matches!(event, ClientEvent::Ping) {
                continue;
            }
            break event;
        };
        assert!(
            matches!(
                &received,
                ClientEvent::MessageCreate { channel_id, content, .. }
                    if channel_id == "chan-1" && content == "привет"
            ),
            "сервер получил {received:?}"
        );

        drop(commands_tx);
        tokio::time::timeout(Duration::from_secs(10), session)
            .await
            .expect("run_session должен завершиться")
            .expect("задача не должна паниковать");
    }

    /// `abort()` (так работает выход) всё равно сообщает `Ended`:
    /// мост не зависнет в `Closing`, а соединение не останется жить.
    #[tokio::test]
    #[allow(clippy::expect_used, reason = "в тестах паника допустима")]
    async fn aborted_session_still_reports_ended() {
        let ready = ServerEvent::Ready(Box::new(ready_payload()));
        let (base, _server_events) = crate::fake_instance::spawn(ready).await;

        let (events_tx, events_rx) = std_mpsc::channel();
        let (_commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
        let (_requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
        let (responses_tx, _responses_rx) = std_mpsc::channel();

        let mut p = params();
        p.base = base;
        let session = tokio::spawn(run_session(
            p,
            events_tx,
            commands_rx,
            requests_rx,
            responses_tx,
        ));

        let events_rx = tokio::task::spawn_blocking(move || {
            let first = events_rx.recv_timeout(Duration::from_secs(10));
            assert!(matches!(first, Ok(BridgeEvent::Ready { .. })), "{first:?}");
            events_rx
        })
        .await
        .expect("join");

        session.abort();
        let _ = session.await;

        let events_rx = tokio::task::spawn_blocking(move || {
            // Допускаем промежуточные `State`, но `Ended` обязан прийти.
            loop {
                match events_rx.recv_timeout(Duration::from_secs(10)) {
                    Ok(BridgeEvent::Ended) => return true,
                    Ok(_) => {}
                    Err(_) => return false,
                }
            }
        })
        .await
        .expect("join");
        assert!(events_rx, "после abort должно прийти `Ended`");
    }

    // ─── Сообщения ЛС и каналов ──────────────────────────────────────────

    #[test]
    fn send_without_session_is_noop() {
        let bridge = new_bridge();
        bridge.send_message(ChannelKind::Space, "chan-1".to_owned(), "hello".to_owned());
        assert!(bridge.commands.is_none());
    }

    #[test]
    fn dm_send_uses_dm_message_create() {
        let (bridge, mut commands, _requests) = active_bridge();
        bridge.send_message(ChannelKind::Dm, "dm1".to_owned(), "привет".to_owned());
        match commands.try_recv() {
            Ok(ClientEvent::DmMessageCreate {
                dm_channel_id,
                content,
                attachments,
                reply_to_id,
            }) => {
                assert_eq!(dm_channel_id, "dm1");
                assert_eq!(content.as_deref(), Some("привет"));
                assert!(attachments.is_none() && reply_to_id.is_none());
            }
            other => panic!("ожидался dm_message_create, пришло {other:?}"),
        }
    }

    #[test]
    fn dm_edit_and_delete_use_dm_events() {
        let (bridge, mut commands, _requests) = active_bridge();
        bridge.edit_message(ChannelKind::Dm, "m1".to_owned(), "правка".to_owned());
        bridge.delete_message(ChannelKind::Dm, "m1".to_owned());
        assert!(matches!(
            commands.try_recv(),
            Ok(ClientEvent::DmMessageEdit { .. })
        ));
        assert!(matches!(
            commands.try_recv(),
            Ok(ClientEvent::DmMessageDelete { .. })
        ));
    }

    #[test]
    fn space_send_keeps_message_create() {
        let (bridge, mut commands, _requests) = active_bridge();
        bridge.send_message(ChannelKind::Space, "c1".to_owned(), "hi".to_owned());
        assert!(matches!(
            commands.try_recv(),
            Ok(ClientEvent::MessageCreate { .. })
        ));
    }

    #[test]
    fn server_error_becomes_a_visible_notice() {
        let (mut bridge, _commands, _requests) = active_bridge();
        let error: ServerEvent = serde_json::from_value(serde_json::json!({
            "type": "error", "message": "Too many messages", "code": "rate_limited"
        }))
        .unwrap_or_else(|err| panic!("error разбирается: {err}"));
        bridge.handle_event(BridgeEvent::Server {
            event: Box::new(error),
        });
        assert_eq!(
            bridge.notice(),
            Some("Слишком часто: подождите несколько секунд")
        );
        bridge.dismiss_notice();
        assert_eq!(bridge.notice(), None);
    }

    // ─── История ─────────────────────────────────────────────────────────

    #[allow(clippy::expect_used, reason = "в тестах паника допустима")]
    fn dm_msg(id: &str, created_at: i64) -> DmMessageWithUser {
        serde_json::from_value(serde_json::json!({
            "id": id, "dmChannelId": "dm1", "userId": "u2", "content": "привет",
            "createdAt": created_at,
            "user": {
                "id": "u2", "username": "bob", "displayName": null, "avatar": null,
                "banner": null, "accentColor": null, "avatarColor": null, "bio": null,
                "status": "online", "customStatus": null, "isAdmin": false,
                "createdAt": 1, "homeInstance": null, "homeUserId": null,
                "replicatedInstances": []
            },
            "attachments": [], "embeds": [], "reactions": []
        }))
        .expect("dm-сообщение разбирается")
    }

    fn page_of(count: usize) -> HistoryItems {
        // Сообщения 1..=count, id возрастают вместе со временем.
        HistoryItems::Dm(
            (1..=count)
                .map(|n| dm_msg(&format!("{n:03}"), i64::try_from(n).unwrap_or(0)))
                .collect(),
        )
    }

    #[test]
    fn ensure_history_requests_once() {
        let (mut bridge, _commands, mut requests) = active_bridge();
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        assert!(matches!(
            requests.try_recv(),
            Ok(BridgeRequest::History {
                before: None,
                kind: ChannelKind::Dm,
                ..
            })
        ));
        assert!(requests.try_recv().is_err(), "повторного запроса нет");
        assert!(
            bridge
                .history_state("dm1")
                .is_some_and(|state| state.loading)
        );
    }

    #[test]
    fn ensure_history_without_session_leaves_no_state() {
        let mut bridge = new_bridge();
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        assert!(bridge.history_state("dm1").is_none());
    }

    #[test]
    fn full_page_keeps_history_open_and_short_page_closes_it() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        bridge.handle_response(BridgeResponse::History {
            channel_id: "dm1".to_owned(),
            items: page_of(usize::from(HISTORY_PAGE)),
        });
        let state = bridge.history_state("dm1").cloned();
        assert_eq!(
            state.map(|s| (s.loading, s.exhausted)),
            Some((false, false))
        );
        assert_eq!(
            bridge.cache().map(|c| c.dm_messages("dm1").len()),
            Some(usize::from(HISTORY_PAGE))
        );

        bridge.handle_response(BridgeResponse::History {
            channel_id: "dm1".to_owned(),
            items: HistoryItems::Dm(vec![dm_msg("000", 0)]),
        });
        assert!(bridge.history_state("dm1").is_some_and(|s| s.exhausted));
    }

    #[test]
    fn load_older_uses_oldest_message_as_cursor() {
        let (mut bridge, _commands, mut requests) = active_bridge();
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        let _ = requests.try_recv();
        bridge.handle_response(BridgeResponse::History {
            channel_id: "dm1".to_owned(),
            items: page_of(usize::from(HISTORY_PAGE)),
        });
        bridge.load_older(ChannelKind::Dm, "dm1");
        match requests.try_recv() {
            Ok(BridgeRequest::History { before, .. }) => {
                assert_eq!(before.as_deref(), Some("001"));
            }
            other => panic!("ожидался запрос истории, пришло {other:?}"),
        }
        // Пока страница в пути, второй запрос не уходит.
        bridge.load_older(ChannelKind::Dm, "dm1");
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn failed_history_is_not_auto_retried() {
        let ok = super::HistoryState::default();
        assert!(ok.can_auto_load());
        for state in [
            super::HistoryState {
                loading: true,
                ..Default::default()
            },
            super::HistoryState {
                exhausted: true,
                ..Default::default()
            },
            super::HistoryState {
                error: Some("429".to_owned()),
                ..Default::default()
            },
        ] {
            assert!(!state.can_auto_load(), "{state:?}");
        }
    }

    #[test]
    fn failed_history_can_be_retried() {
        let (mut bridge, _commands, mut requests) = active_bridge();
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        let _ = requests.try_recv();
        bridge.handle_response(BridgeResponse::HistoryFailed {
            channel_id: "dm1".to_owned(),
            message: "нет сети".to_owned(),
        });
        let state = bridge.history_state("dm1").cloned();
        assert_eq!(state.and_then(|s| s.error).as_deref(), Some("нет сети"));
        bridge.load_older(ChannelKind::Dm, "dm1");
        assert!(matches!(
            requests.try_recv(),
            Ok(BridgeRequest::History { before: None, .. })
        ));
    }

    #[test]
    fn ready_after_reconnect_resets_history() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        bridge.handle_response(BridgeResponse::History {
            channel_id: "dm1".to_owned(),
            items: page_of(3),
        });
        // Повторный `ready` приходит как обычное серверное событие.
        bridge.handle_event(BridgeEvent::Server {
            event: Box::new(ServerEvent::Ready(Box::new(ready_payload()))),
        });
        assert!(bridge.history_state("dm1").is_none(), "состояние сброшено");
        assert_eq!(
            bridge.cache().map(|c| c.dm_messages("dm1").len()),
            Some(0),
            "кэш пересобран без сообщений"
        );
    }

    #[test]
    fn late_history_response_after_reset_is_ignored() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.handle_response(BridgeResponse::History {
            channel_id: "dm1".to_owned(),
            items: page_of(3),
        });
        assert_eq!(bridge.cache().map(|c| c.dm_messages("dm1").len()), Some(0));
    }

    #[test]
    fn opened_dm_is_added_to_cache() {
        let (mut bridge, _commands, _requests) = active_bridge();
        let channel: bs_types::DmChannel = serde_json::from_value(serde_json::json!({
            "id": "dm9", "federatedId": null, "ownerId": null,
            "ownerHomeUserId": null, "ownerHomeInstance": null, "createdAt": 1,
            "members": [], "metadataUpdatedAt": 2
        }))
        .unwrap_or_else(|err| panic!("dm разбирается: {err}"));
        bridge.handle_response(BridgeResponse::DmChannel(Box::new(channel)));
        assert!(
            bridge
                .cache()
                .is_some_and(|c| c.dm_channel("dm9").is_some())
        );
        assert_eq!(bridge.take_opened_dm().as_deref(), Some("dm9"));
    }
}
