//! Мост между GUI-потоком и tokio-runtime.
//!
//! Разделение обязанностей:
//! - фоновая tokio-задача держит [`InstanceHandle`] и читает события;
//! - GUI-поток владеет [`Cache`] и применяет к нему [`apply`] прямо в кадре.
//!
//! Так GUI никогда не блокируется на сети, а `Cache` (не `Clone`) не
//! приходится копировать между потоками.

use std::sync::mpsc as std_mpsc;

use bs_core::{Cache, apply};
use bs_identity::CredentialStore;
use bs_instances::{ConnectionState, InstanceEvent, InstanceHandle, ReconnectPolicy};
use bs_types::{ClientEvent, ClientKind, Origin, ReadyPayload, ServerEvent};
use tokio::runtime::Handle;
use tokio::sync::{broadcast, mpsc as tokio_mpsc};

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
    /// Запись прошла успешно.
    Done,
    /// Запрос не удался.
    Failed(String),
}

/// Событие из фоновой задачи в GUI-поток.
#[derive(Debug)]
enum BridgeEvent {
    /// Первый снимок состояния.
    Ready(Box<ReadyPayload>),
    /// Дельта состояния.
    Server(Box<ServerEvent>),
    /// Ошибка логина или соединения.
    Error(String),
    /// Сменилось состояние соединения (connecting/connected/reconnecting/stopped).
    State(ConnectionState),
    /// Соединение окончательно закрыто.
    Disconnected,
}

/// Мост GUI ↔ tokio.
#[derive(Debug)]
pub(crate) struct Bridge {
    runtime: Handle,
    events: Option<std_mpsc::Receiver<BridgeEvent>>,
    commands: Option<tokio_mpsc::UnboundedSender<ClientEvent>>,
    requests: Option<tokio_mpsc::UnboundedSender<BridgeRequest>>,
    responses: Option<std_mpsc::Receiver<BridgeResponse>>,
    friends: Vec<bs_types::Friend>,
    friend_requests: Vec<bs_types::FriendRequest>,
    dm_channels: Vec<bs_types::DmChannel>,
    search_results: Vec<bs_types::User>,
    social_error: Option<String>,
    /// Канал, только что открытый по запросу `OpenDm` (нужно переключить чат).
    opened_dm: Option<String>,
    cache: Option<Cache>,
    ready: bool,
    busy: bool,
    /// Потеряна ли живая сессия: GUI должен вернуться на логин.
    session_lost: bool,
    /// Последнее известное состояние соединения.
    connection: ConnectionState,
    last_error: Option<String>,
}

/// Приводит адрес инстанса к абсолютному URL.
///
/// Пользователь обычно вводит только хост (`backspace.example`), поэтому
/// схема `https://` подставляется автоматически. Явно указанная схема
/// (`http://`, `ws://` и т.п.) сохраняется как есть.
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
            events: None,
            commands: None,
            requests: None,
            responses: None,
            friends: Vec::new(),
            friend_requests: Vec::new(),
            dm_channels: Vec::new(),
            search_results: Vec::new(),
            social_error: None,
            opened_dm: None,
            cache: None,
            ready: false,
            busy: false,
            session_lost: false,
            connection: ConnectionState::Connecting,
            last_error: None,
        }
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
            BridgeEvent::Ready(payload) => {
                self.cache = Some(Cache::from_ready(*payload));
                self.ready = true;
                self.busy = false;
                self.session_lost = false;
                self.last_error = None;
            }
            BridgeEvent::Server(event) => {
                if let Some(cache) = self.cache.as_mut() {
                    let _ = apply(cache, *event);
                }
            }
            BridgeEvent::State(state) => {
                self.connection = state;
            }
            BridgeEvent::Error(message) => {
                self.last_error = Some(message);
                self.busy = false;
                // Ошибка до первого ready — отказ логина, на логине и остаёмся.
                self.session_lost = self.cache.is_some();
            }
            BridgeEvent::Disconnected => {
                self.last_error = Some("соединение закрыто".to_owned());
                self.busy = false;
                self.session_lost = true;
                self.connection = ConnectionState::Stopped;
            }
        }
    }

    /// `true` ровно один раз — когда пришёл первый `ready`.
    pub(crate) fn take_ready(&mut self) -> bool {
        std::mem::take(&mut self.ready)
    }

    /// Идёт ли логин/подключение.
    pub(crate) const fn is_busy(&self) -> bool {
        self.busy
    }

    /// Последняя ошибка, если была.
    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Текущий кэш состояния (если уже подключены).
    pub(crate) const fn cache(&self) -> Option<&Cache> {
        self.cache.as_ref()
    }

    /// Текущее состояние соединения (для строки статуса).
    pub(crate) const fn connection(&self) -> &ConnectionState {
        &self.connection
    }

    /// `true` ровно один раз — если живая сессия потеряна.
    pub(crate) fn take_session_lost(&mut self) -> bool {
        std::mem::take(&mut self.session_lost)
    }

    /// Сбрасывает сессию перед возвратом на экран логина.
    ///
    /// Кэш и каналы связи больше не нужны: старая задача уже завершилась.
    pub(crate) fn reset_session(&mut self) {
        self.events = None;
        self.commands = None;
        self.requests = None;
        self.responses = None;
        self.friends.clear();
        self.friend_requests.clear();
        self.dm_channels.clear();
        self.search_results.clear();
        self.social_error = None;
        self.opened_dm = None;
        self.cache = None;
        self.ready = false;
        self.session_lost = false;
        self.connection = ConnectionState::Connecting;
    }

    /// Запускает логин в фоне.
    pub(crate) fn start_login(&mut self, base: &str, username: &str, password: &str) {
        if self.busy {
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

        self.busy = true;
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

        let username = username.trim().to_owned();
        let password = password.to_owned();
        self.runtime.spawn(run_session(
            base,
            username,
            password,
            events_tx,
            commands_rx,
            requests_rx,
            responses_tx,
        ));
    }

    /// Отправляет сообщение в канал (в фоне).
    pub(crate) fn send_message(&self, channel_id: String, content: String) {
        self.send(ClientEvent::MessageCreate {
            channel_id,
            content,
            reply_to_id: None,
        });
    }

    /// Отправляет команду серверу, если сессия жива.
    pub(crate) fn send(&self, event: ClientEvent) {
        if let Some(commands) = self.commands.as_ref() {
            let _ = commands.send(event);
        }
    }

    /// Правит своё сообщение.
    pub(crate) fn edit_message(&self, message_id: String, content: String) {
        self.send(ClientEvent::MessageEdit {
            message_id,
            content,
        });
    }

    /// Удаляет своё сообщение.
    pub(crate) fn delete_message(&self, message_id: String) {
        self.send(ClientEvent::MessageDelete { message_id });
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

    /// Отправляет REST-запрос, если сессия жива.
    pub(crate) fn request(&self, request: BridgeRequest) {
        if let Some(requests) = self.requests.as_ref() {
            let _ = requests.send(request);
        }
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

    /// Применяет ответ REST-запроса к локальным спискам.
    fn handle_response(&mut self, response: BridgeResponse) {
        match response {
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

/// Фоновая сессия: логин, затем пересылка событий и команд.
async fn run_session(
    base: String,
    username: String,
    password: String,
    events: std_mpsc::Sender<BridgeEvent>,
    mut commands: tokio_mpsc::UnboundedReceiver<ClientEvent>,
    mut requests: tokio_mpsc::UnboundedReceiver<BridgeRequest>,
    responses: std_mpsc::Sender<BridgeResponse>,
) {
    let mut store = CredentialStore::new();
    let login = InstanceHandle::login(
        &mut store,
        Origin::Home,
        &base,
        &username,
        &password,
        Some(ClientKind::Desktop),
        ReconnectPolicy::default(),
    )
    .await;

    let (handle, ready) = match login {
        Ok(pair) => pair,
        Err(err) => {
            let _ = events.send(BridgeEvent::Error(err.to_string()));
            return;
        }
    };

    match ready {
        ServerEvent::Ready(payload) => {
            if events.send(BridgeEvent::Ready(payload)).is_err() {
                return;
            }
        }
        other => {
            let _ = events.send(BridgeEvent::Error(format!(
                "ожидался ready, пришло {other:?}"
            )));
            return;
        }
    }

    let mut sub = handle.subscribe();
    loop {
        tokio::select! {
            event = sub.recv() => match event {
                Ok(InstanceEvent::Server(boxed)) => {
                    if events.send(BridgeEvent::Server(boxed)).is_err() {
                        return;
                    }
                }
                Ok(InstanceEvent::AuthRejected { message, code }) => {
                    let code = code.map_or_else(String::new, |c| format!(" [{c}]"));
                    let _ = events.send(BridgeEvent::Error(format!("{message}{code}")));
                }
                Ok(InstanceEvent::StateChanged(state)) => {
                    if events.send(BridgeEvent::State(state)).is_err() {
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
                        let _ = events.send(BridgeEvent::Error(err.to_string()));
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
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bs_core::Cache;
    use bs_instances::ConnectionState;
    use bs_types::ReadyPayload;

    use super::{Bridge, BridgeEvent, normalize_base, run_session, std_mpsc};
    use tokio::sync::mpsc as tokio_mpsc;

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

    /// Ошибка HTTP-логина должна доехать до GUI как `BridgeEvent::Error`,
    /// после чего фоновая сессия завершается и ничего не ждёт.
    #[tokio::test]
    #[allow(clippy::expect_used, reason = "в тестах паника допустима")]
    async fn run_session_reports_http_login_error() {
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

        run_session(
            server.uri(),
            "alice".to_owned(),
            "secret".to_owned(),
            events_tx,
            commands_rx,
            requests_rx,
            responses_tx,
        )
        .await;

        let event = events_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("run_session должен прислать событие об ошибке");
        assert!(
            matches!(&event, BridgeEvent::Error(_)),
            "ожидалась ошибка, пришло {event:?}"
        );
        let BridgeEvent::Error(message) = event else {
            return;
        };
        assert!(message.contains("401"), "текст ошибки: {message}");
        assert!(
            message.contains("Invalid credentials"),
            "текст ошибки: {message}"
        );
        assert!(
            events_rx.try_recv().is_err(),
            "после ошибки логина сессия не должна продолжаться"
        );
    }

    #[test]
    fn state_event_updates_connection() {
        let mut bridge = new_bridge();
        bridge.handle_event(BridgeEvent::State(ConnectionState::Reconnecting {
            attempt: 2,
        }));
        assert_eq!(
            bridge.connection(),
            &ConnectionState::Reconnecting { attempt: 2 }
        );
        // Состояние само по себе не считается потерей сессии.
        assert!(!bridge.take_session_lost());
    }

    #[test]
    fn error_without_cache_is_login_error_not_session_loss() {
        let mut bridge = new_bridge();
        bridge.busy = true;
        bridge.handle_event(BridgeEvent::Error("неверный пароль".to_owned()));
        assert!(!bridge.is_busy());
        assert!(
            !bridge.take_session_lost(),
            "отказ логина оставляет пользователя на форме"
        );
    }

    #[test]
    fn error_with_cache_marks_session_lost() {
        let mut bridge = new_bridge();
        bridge.cache = Some(Cache::from_ready(ready_payload()));
        bridge.handle_event(BridgeEvent::Error("boom".to_owned()));
        assert!(bridge.take_session_lost(), "живая сессия потеряна");
    }

    #[test]
    fn disconnected_marks_session_lost_and_stops() {
        let mut bridge = new_bridge();
        bridge.handle_event(BridgeEvent::Disconnected);
        assert!(bridge.take_session_lost());
        assert_eq!(bridge.connection(), &ConnectionState::Stopped);
        assert_eq!(bridge.last_error(), Some("соединение закрыто"));
    }

    #[test]
    fn take_session_lost_is_one_shot() {
        let mut bridge = new_bridge();
        bridge.handle_event(BridgeEvent::Disconnected);
        assert!(bridge.take_session_lost());
        assert!(!bridge.take_session_lost());
    }

    #[test]
    fn reset_session_clears_cache_and_channels() {
        let mut bridge = new_bridge();
        bridge.cache = Some(Cache::from_ready(ready_payload()));
        bridge.ready = true;
        bridge.session_lost = true;
        bridge.connection = ConnectionState::Stopped;

        bridge.reset_session();

        assert!(bridge.cache().is_none());
        assert!(!bridge.ready, "ready должен быть сброшен");
        assert_eq!(bridge.connection(), &ConnectionState::Connecting);
        assert!(!bridge.session_lost);
    }

    #[test]
    fn ready_clears_session_lost_and_error() {
        let mut bridge = new_bridge();
        bridge.session_lost = true;
        bridge.last_error = Some("старое".to_owned());
        bridge.handle_event(BridgeEvent::Ready(Box::new(ready_payload())));
        assert!(!bridge.session_lost);
        assert_eq!(bridge.last_error(), None);
        assert!(bridge.cache().is_some());
    }

    /// Полный happy-path: HTTP-логин, WebSocket `auth`, `ready`, затем
    /// отправка команды доходит до сервера как `ClientEvent`.
    #[tokio::test]
    #[allow(clippy::expect_used, reason = "в тестах паника допустима")]
    async fn run_session_happy_path_relays_commands() {
        use bs_types::{ClientEvent, ServerEvent};

        let ready = ServerEvent::Ready(Box::new(ready_payload()));
        let (base, mut server_events) = crate::fake_instance::spawn(ready).await;

        let (events_tx, events_rx) = std_mpsc::channel();
        let (commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
        let (_requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
        let (responses_tx, _responses_rx) = std_mpsc::channel();

        let session = tokio::spawn(run_session(
            base,
            "alice".to_owned(),
            "secret".to_owned(),
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
            matches!(event, Ok(BridgeEvent::Ready(_))),
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

        // Закрываем канал команд — сессия должна корректно завершиться.
        drop(commands_tx);
        tokio::time::timeout(Duration::from_secs(10), session)
            .await
            .expect("run_session должен завершиться")
            .expect("задача не должна паниковать");
    }

    #[test]
    fn take_ready_is_one_shot() {
        let mut bridge = new_bridge();
        bridge.ready = true;
        assert!(bridge.take_ready(), "первый вызов должен вернуть true");
        assert!(!bridge.take_ready(), "второй вызов должен вернуть false");
    }

    #[test]
    fn take_ready_false_initially() {
        let mut bridge = new_bridge();
        assert!(!bridge.take_ready());
    }

    #[test]
    fn error_clears_busy_and_sets_message() {
        let mut bridge = new_bridge();
        bridge.busy = true;
        bridge.handle_event(BridgeEvent::Error("boom".to_owned()));
        assert!(!bridge.is_busy(), "ошибка должна снимать busy");
        assert_eq!(bridge.last_error(), Some("boom"));
    }

    #[test]
    fn disconnected_clears_busy_and_sets_message() {
        let mut bridge = new_bridge();
        bridge.busy = true;
        bridge.handle_event(BridgeEvent::Disconnected);
        assert!(!bridge.is_busy());
        assert_eq!(bridge.last_error(), Some("соединение закрыто"));
    }

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
            bridge.start_login(base, user, pass);
            assert!(!bridge.is_busy(), "пустые поля не должны взводить busy");
            assert_eq!(
                bridge.last_error(),
                Some("заполните адрес, логин и пароль"),
                "base={base:?} user={user:?} pass={pass:?}"
            );
            assert!(
                bridge.events.is_none(),
                "канал событий не должен создаваться"
            );
            assert!(
                bridge.commands.is_none(),
                "канал команд не должен создаваться"
            );
        }
    }

    #[test]
    fn normalize_base_adds_https_scheme() {
        assert_eq!(
            normalize_base("backspace.example").as_deref(),
            Some("https://backspace.example")
        );
        assert_eq!(
            normalize_base("  example.com:8443  ").as_deref(),
            Some("https://example.com:8443")
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
        assert_eq!(
            normalize_base("ws://example.com").as_deref(),
            Some("ws://example.com")
        );
    }

    #[test]
    fn normalize_base_rejects_empty() {
        assert_eq!(normalize_base(""), None);
        assert_eq!(normalize_base("   "), None);
    }

    #[test]
    fn start_login_accepts_host_without_scheme() {
        let mut bridge = new_bridge();
        bridge.start_login("backspace.example", "user", "pass");
        assert!(bridge.is_busy(), "адрес без схемы должен приниматься");
        assert_eq!(bridge.last_error(), None);
        assert!(bridge.events.is_some(), "канал событий должен быть создан");
        assert!(bridge.commands.is_some(), "канал команд должен быть создан");
    }

    #[test]
    fn start_login_ignored_while_busy() {
        let mut bridge = new_bridge();
        bridge.busy = true;
        bridge.start_login("https://example.com", "user", "pass");
        // Ранний выход: ни каналов, ни изменения ошибки.
        assert!(bridge.events.is_none());
        assert!(bridge.commands.is_none());
        assert_eq!(bridge.last_error(), None);
    }

    #[test]
    fn send_message_without_session_is_noop() {
        let bridge = new_bridge();
        // Не должно паниковать и не должно ничего отправлять.
        bridge.send_message("chan-1".to_owned(), "hello".to_owned());
        assert!(bridge.commands.is_none());
    }
}
