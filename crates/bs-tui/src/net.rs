//! Фоновая сессия: tokio-рантайм в отдельном потоке, общение с TUI через каналы.
//!
//! TUI-поток не должен блокироваться на сети, а терминал нельзя трогать из
//! tokio-задачи. Поэтому здесь живёт [`NetWorker`]: он держит `InstanceHandle`,
//! пересылает события сервера и состояния соединения наружу, а внутрь
//! принимает [`ClientEvent`].

use std::sync::mpsc as std_mpsc;
use std::thread;

use bs_identity::CredentialStore;
use bs_instances::{ConnectionState, InstanceEvent, InstanceHandle, ReconnectPolicy};
use bs_types::{ClientEvent, ClientKind, Origin, ServerEvent};
use tokio::sync::mpsc as tokio_mpsc;

use crate::login::Credentials;

/// Запрос к REST API, который TUI отправляет фоновой сессии.
///
/// WebSocket-события [`ClientEvent`] идут отдельным каналом: у них разная
/// природа — команда серверу против запроса данных.
#[derive(Debug, Clone)]
pub enum NetRequest {
    /// Список друзей.
    Friends,
    /// Список заявок в друзья.
    FriendRequests,
    /// Поиск пользователей по подстроке.
    SearchUsers(String),
    /// Список личных каналов.
    DmChannels,
    /// Отправить заявку в друзья.
    SendFriendRequest(bs_api::SendFriendRequest),
    /// Принять или отклонить заявку.
    RespondFriendRequest(String, bs_api::FriendRequestDecision),
    /// Отозвать свою заявку.
    CancelFriendRequest(String),
    /// Удалить друга.
    RemoveFriend(String),
    /// Открыть (или создать) личный канал с пользователем.
    OpenDm(bs_api::CreateDmRequest),
}

/// Ответ фоновой сессии на [`NetRequest`].
#[derive(Debug)]
pub enum NetResponse {
    /// Пришли друзья.
    Friends(Vec<bs_types::Friend>),
    /// Пришли заявки в друзья.
    FriendRequests(Vec<bs_types::FriendRequest>),
    /// Пришли результаты поиска пользователей.
    Users(Vec<bs_types::User>),
    /// Пришли личные каналы.
    DmChannels(Vec<bs_types::DmChannel>),
    /// Пришёл личный канал (после открытия).
    DmChannel(Box<bs_types::DmChannel>),
    /// Запись прошла успешно, данных нет.
    Done,
    /// Запрос не удался.
    Failed(String),
}

/// Что фоновая сессия сообщает TUI.
#[derive(Debug)]
pub enum NetEvent {
    /// Логин прошёл, пришёл первый `ready`.
    Ready(Box<ServerEvent>),
    /// Очередное событие сервера.
    Server(Box<ServerEvent>),
    /// Сменилось состояние соединения.
    State(ConnectionState),
    /// Ошибка, после которой сессия бесполезна.
    Failed(String),
}

/// Ручка TUI-потока на фоновую сессию.
#[derive(Debug)]
pub struct NetWorker {
    events: std_mpsc::Receiver<NetEvent>,
    commands: tokio_mpsc::UnboundedSender<ClientEvent>,
    requests: tokio_mpsc::UnboundedSender<NetRequest>,
    responses: std_mpsc::Receiver<NetResponse>,
    join: Option<thread::JoinHandle<()>>,
}

impl NetWorker {
    /// Поднимает рантайм, логинится и начинает пересылку событий.
    ///
    /// Возвращается сразу: результат логина придёт как [`NetEvent::Ready`]
    /// или [`NetEvent::Failed`].
    pub fn spawn(creds: Credentials) -> Self {
        let (events_tx, events) = std_mpsc::channel();
        let (commands, commands_rx) = tokio_mpsc::unbounded_channel();
        let (requests, requests_rx) = tokio_mpsc::unbounded_channel();
        let (responses_tx, responses) = std_mpsc::channel();

        let join = thread::Builder::new()
            .name("bs-tui-net".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = events_tx.send(NetEvent::Failed(format!(
                            "не удалось поднять рантайм: {err}"
                        )));
                        return;
                    }
                };
                runtime.block_on(run_session(
                    creds,
                    &events_tx,
                    commands_rx,
                    requests_rx,
                    &responses_tx,
                ));
            })
            .ok();

        Self {
            events,
            commands,
            requests,
            responses,
            join,
        }
    }

    /// Неблокирующе забирает всё, что накопилось с прошлого кадра.
    pub fn drain(&self) -> Vec<NetEvent> {
        self.events.try_iter().collect()
    }

    /// Отправляет команду серверу. `false` — сессия уже мертва.
    pub fn send(&self, event: ClientEvent) -> bool {
        self.commands.send(event).is_ok()
    }

    /// Отправляет REST-запрос. `false` — сессия уже мертва.
    pub fn request(&self, request: NetRequest) -> bool {
        self.requests.send(request).is_ok()
    }

    /// Неблокирующе забирает готовые ответы REST.
    pub fn drain_responses(&self) -> Vec<NetResponse> {
        self.responses.try_iter().collect()
    }

    /// Дожидается завершения фонового потока.
    ///
    /// Канал команд закрывается до ожидания: иначе фоновый `select!` никогда
    /// не увидит `None` и поток не завершится.
    pub fn join(self) {
        let Self {
            commands,
            requests,
            join,
            ..
        } = self;
        drop(commands);
        drop(requests);
        if let Some(join) = join {
            let _ = join.join();
        }
    }
}

async fn run_session(
    creds: Credentials,
    events: &std_mpsc::Sender<NetEvent>,
    mut commands: tokio_mpsc::UnboundedReceiver<ClientEvent>,
    mut requests: tokio_mpsc::UnboundedReceiver<NetRequest>,
    responses: &std_mpsc::Sender<NetResponse>,
) {
    let mut requests_open = true;
    let mut store = CredentialStore::new();
    let login = InstanceHandle::login(
        &mut store,
        Origin::Home,
        &creds.base,
        &creds.username,
        &creds.password,
        Some(ClientKind::Desktop),
        ReconnectPolicy::default(),
    )
    .await;

    let (handle, ready) = match login {
        Ok(pair) => pair,
        Err(err) => {
            let _ = events.send(NetEvent::Failed(err.to_string()));
            return;
        }
    };

    if events.send(NetEvent::Ready(Box::new(ready))).is_err() {
        return;
    }

    let mut sub = handle.subscribe();
    loop {
        tokio::select! {
            received = sub.recv() => match received {
                Ok(InstanceEvent::Server(event)) => {
                    if events.send(NetEvent::Server(event)).is_err() {
                        return;
                    }
                }
                Ok(InstanceEvent::StateChanged(state)) => {
                    if events.send(NetEvent::State(state)).is_err() {
                        return;
                    }
                }
                Ok(InstanceEvent::AuthRejected { message, code }) => {
                    let code = code.map_or_else(String::new, |c| format!(" [{c}]"));
                    let _ = events.send(NetEvent::Failed(format!("{message}{code}")));
                    return;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "TUI отстал от потока событий");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    let _ = events.send(NetEvent::Failed("поток событий закрыт".to_owned()));
                    return;
                }
            },
            request = recv_request(&mut requests, &mut requests_open) => {
                if let Some(request) = request {
                    let response = handle_request(handle.client(), request).await;
                    if responses.send(response).is_err() {
                        return;
                    }
                }
            },
            command = commands.recv() => {
                let Some(event) = command else {
                    let _ = handle.shutdown().await;
                    return;
                };
                if let Err(err) = handle.send(event).await {
                    tracing::warn!(%err, "не удалось отправить команду");
                }
            }
        }
    }
}

/// Ждёт следующий REST-запрос; при закрытом канале «засыпает» навсегда.
///
/// Возвращать [`std::future::pending`] нужно, чтобы закрытый канал не давал
/// `select!` мгновенно готовую ветку и не крутил цикл вхолостую.
async fn recv_request(
    requests: &mut tokio_mpsc::UnboundedReceiver<NetRequest>,
    open: &mut bool,
) -> Option<NetRequest> {
    if !*open {
        return std::future::pending().await;
    }
    let request = requests.recv().await;
    if request.is_none() {
        *open = false;
    }
    request
}

/// Выполняет REST-запрос и превращает результат в [`NetResponse`].
async fn handle_request(client: &bs_api::ApiClient, request: NetRequest) -> NetResponse {
    match request {
        NetRequest::Friends => match client.friends().await {
            Ok(friends) => NetResponse::Friends(friends),
            Err(err) => NetResponse::Failed(err.to_string()),
        },
        NetRequest::FriendRequests => match client.friend_requests().await {
            Ok(requests) => NetResponse::FriendRequests(requests),
            Err(err) => NetResponse::Failed(err.to_string()),
        },
        NetRequest::SearchUsers(query) => match client.search_users(&query).await {
            Ok(users) => NetResponse::Users(users),
            Err(err) => NetResponse::Failed(err.to_string()),
        },
        NetRequest::DmChannels => match client.dm_channels().await {
            Ok(channels) => NetResponse::DmChannels(channels),
            Err(err) => NetResponse::Failed(err.to_string()),
        },
        NetRequest::OpenDm(request) => match client.open_dm(&request).await {
            Ok(channel) => NetResponse::DmChannel(Box::new(channel)),
            Err(err) => NetResponse::Failed(err.to_string()),
        },
        NetRequest::SendFriendRequest(request) => {
            match client.send_friend_request(&request).await {
                Ok(_) => NetResponse::Done,
                Err(err) => NetResponse::Failed(err.to_string()),
            }
        }
        NetRequest::RespondFriendRequest(id, decision) => {
            match client.respond_friend_request(&id, decision).await {
                Ok(()) => NetResponse::Done,
                Err(err) => NetResponse::Failed(err.to_string()),
            }
        }
        NetRequest::CancelFriendRequest(id) => match client.cancel_friend_request(&id).await {
            Ok(()) => NetResponse::Done,
            Err(err) => NetResponse::Failed(err.to_string()),
        },
        NetRequest::RemoveFriend(user_id) => match client.remove_friend(&user_id).await {
            Ok(()) => NetResponse::Done,
            Err(err) => NetResponse::Failed(err.to_string()),
        },
    }
}
