//! Фоновая сессия: tokio-рантайм в отдельном потоке, общение с TUI через каналы.
//!
//! TUI-поток не должен блокироваться на сети, а терминал нельзя трогать из
//! tokio-задачи. Поэтому здесь живёт [`NetWorker`]: он держит реестр
//! [`InstanceHandle`] по инстансам, пересылает их события и состояния
//! наружу, а внутрь принимает команды с указанием origin.

use std::collections::HashMap;
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
    /// Получить токен LiveKit для голосового канала.
    LiveKitTokenChannel(String),
    /// Получить токен LiveKit для личного звонка.
    LiveKitTokenDm(String),
}

/// Ответ фоновой сессии на [`NetRequest`], помеченный origin-ом.
#[derive(Debug)]
pub struct NetResponse {
    /// Инстанс, к которому относился запрос.
    pub origin: Origin,
    /// Собственно ответ.
    pub payload: NetPayload,
}

/// Полезная нагрузка ответа на REST-запрос.
#[derive(Debug)]
pub enum NetPayload {
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
    /// Получен URL и токен для подключения к LiveKit.
    LiveKitToken(bs_api::LiveKitTokenResponse),
    /// Запись прошла успешно, данных нет.
    Done,
    /// Запрос не удался.
    Failed(String),
}

/// Что фоновая сессия сообщает TUI.
#[derive(Debug)]
pub enum NetEvent {
    /// Новый инстанс подключён, пришёл первый `ready`.
    Ready {
        /// Инстанс.
        origin: Origin,
        /// Первое событие сервера (`ready`).
        event: Box<ServerEvent>,
    },
    /// Очередное событие сервера инстанса.
    Server {
        /// Инстанс.
        origin: Origin,
        /// Событие сервера.
        event: Box<ServerEvent>,
    },
    /// Сменилось состояние соединения инстанса.
    State {
        /// Инстанс.
        origin: Origin,
        /// Новое состояние.
        state: ConnectionState,
    },
    /// Инстанс отключён и удалён из реестра.
    Disconnected {
        /// Инстанс.
        origin: Origin,
    },
    /// Ошибка, после которой сессия инстанса бесполезна.
    Failed {
        /// Инстанс (для ошибки рантайма — [`Origin::Home`]).
        origin: Origin,
        /// Текст ошибки.
        message: String,
    },
}

/// Команда фоновой сессии.
enum Command {
    /// Подключиться (или переподключиться) к инстансу.
    Connect {
        /// Инстанс.
        origin: Origin,
        /// Учётные данные для логина.
        creds: Credentials,
    },
    /// Отключиться от инстанса.
    Disconnect {
        /// Инстанс.
        origin: Origin,
    },
    /// Отправить событие серверу инстанса.
    Send {
        /// Инстанс.
        origin: Origin,
        /// Событие клиента.
        event: ClientEvent,
    },
    /// Выполнить REST-запрос на инстансе.
    Request {
        /// Инстанс.
        origin: Origin,
        /// Запрос.
        request: NetRequest,
    },
}

/// Ручка TUI-потока на фоновую сессию.
#[derive(Debug)]
pub struct NetWorker {
    events: std_mpsc::Receiver<NetEvent>,
    commands: tokio_mpsc::UnboundedSender<Command>,
    responses: std_mpsc::Receiver<NetResponse>,
    join: Option<thread::JoinHandle<()>>,
}

impl NetWorker {
    /// Поднимает рантайм и запускает фоновую сессию.
    ///
    /// Возвращается сразу: результат логина придёт как [`NetEvent::Ready`]
    /// или [`NetEvent::Failed`].
    pub fn spawn() -> Self {
        let (events_tx, events) = std_mpsc::channel();
        let (commands, commands_rx) = tokio_mpsc::unbounded_channel();
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
                        let _ = events_tx.send(NetEvent::Failed {
                            origin: Origin::Home,
                            message: format!("не удалось поднять рантайм: {err}"),
                        });
                        return;
                    }
                };
                runtime.block_on(run_session(commands_rx, &events_tx, &responses_tx));
            })
            .ok();

        Self {
            events,
            commands,
            responses,
            join,
        }
    }

    /// Неблокирующе забирает всё, что накопилось с прошлого кадра.
    pub fn drain(&self) -> Vec<NetEvent> {
        self.events.try_iter().collect()
    }

    /// Подключается к инстансу. `false` — сессия уже мертва.
    pub fn connect(&self, origin: Origin, creds: Credentials) -> bool {
        self.commands
            .send(Command::Connect { origin, creds })
            .is_ok()
    }

    /// Отключается от инстанса. `false` — сессия уже мертва.
    pub fn disconnect(&self, origin: Origin) -> bool {
        self.commands.send(Command::Disconnect { origin }).is_ok()
    }

    /// Отправляет команду серверу инстанса. `false` — сессия уже мертва.
    pub fn send(&self, origin: &Origin, event: ClientEvent) -> bool {
        self.commands
            .send(Command::Send {
                origin: origin.clone(),
                event,
            })
            .is_ok()
    }

    /// Отправляет REST-запрос инстансу. `false` — сессия уже мертва.
    pub fn request(&self, origin: &Origin, request: NetRequest) -> bool {
        self.commands
            .send(Command::Request {
                origin: origin.clone(),
                request,
            })
            .is_ok()
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
        let Self { commands, join, .. } = self;
        drop(commands);
        if let Some(join) = join {
            let _ = join.join();
        }
    }
}

/// Один подключённый инстанс в фоновой сессии.
struct Live {
    handle: InstanceHandle,
    sub: tokio::sync::broadcast::Receiver<InstanceEvent>,
}

/// Состояние фоновой сессии между командами.
struct Session {
    store: CredentialStore,
    live: HashMap<Origin, Live>,
}

impl Session {
    fn new() -> Self {
        Self {
            store: CredentialStore::new(),
            live: HashMap::new(),
        }
    }

    /// Подключает инстанс: логин и запуск диспетчера.
    async fn connect(
        &mut self,
        origin: Origin,
        creds: Credentials,
        events: &std_mpsc::Sender<NetEvent>,
    ) {
        // Переподключение к тому же origin: старый handle корректно гасим.
        if let Some(previous) = self.live.remove(&origin) {
            let _ = previous.handle.shutdown().await;
        }

        let login = InstanceHandle::login(
            &mut self.store,
            origin.clone(),
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
                let _ = events.send(NetEvent::Failed {
                    origin,
                    message: err.to_string(),
                });
                return;
            }
        };

        let sub = handle.subscribe();
        self.live.insert(origin.clone(), Live { handle, sub });

        let _ = events.send(NetEvent::Ready {
            origin,
            event: Box::new(ready),
        });
    }

    /// Отключает инстанс, если он есть.
    async fn disconnect(&mut self, origin: &Origin, events: &std_mpsc::Sender<NetEvent>) {
        if let Some(live) = self.live.remove(origin) {
            let _ = live.handle.shutdown().await;
            let _ = events.send(NetEvent::Disconnected {
                origin: origin.clone(),
            });
        }
    }

    /// Гасит все подключения при завершении сессии.
    async fn shutdown_all(&mut self) {
        let drained: Vec<Live> = self.live.drain().map(|(_, live)| live).collect();
        for live in drained {
            let _ = live.handle.shutdown().await;
        }
    }
}

async fn run_session(
    mut commands: tokio_mpsc::UnboundedReceiver<Command>,
    events: &std_mpsc::Sender<NetEvent>,
    responses: &std_mpsc::Sender<NetResponse>,
) {
    let mut session = Session::new();

    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Connect { origin, creds }) => {
                    session.connect(origin, creds, events).await;
                }
                Some(Command::Disconnect { origin }) => {
                    session.disconnect(&origin, events).await;
                }
                Some(Command::Send { origin, event }) => {
                    if let Some(live) = session.live.get(&origin) {
                        if let Err(err) = live.handle.send(event).await {
                            tracing::warn!(%err, "не удалось отправить команду");
                        }
                    }
                }
                Some(Command::Request { origin, request }) => {
                    let Some(live) = session.live.get(&origin) else {
                        let _ = responses.send(NetResponse {
                            origin,
                            payload: NetPayload::Failed(
                                "инстанс не подключён".to_owned(),
                            ),
                        });
                        continue;
                    };
                    let payload = handle_request(live.handle.client(), request).await;
                    if responses.send(NetResponse { origin, payload }).is_err() {
                        session.shutdown_all().await;
                        return;
                    }
                }
                None => {
                    session.shutdown_all().await;
                    return;
                }
            },
            received = recv_any(&mut session.live) => {
                let Some((origin, event)) = received else {
                    continue;
                };
                if relay_event(origin, event, events).is_break() {
                    session.shutdown_all().await;
                    return;
                }
            }
        }
    }
}

/// Ждёт событие от любого подключённого инстанса.
///
/// Возвращает `None`, если реестр пуст (ветка «спит», чтобы `select!` не
/// крутился вхолостую).
async fn recv_any(live: &mut HashMap<Origin, Live>) -> Option<(Origin, InstanceEvent)> {
    if live.is_empty() {
        return std::future::pending::<Option<(Origin, InstanceEvent)>>().await;
    }
    Some(recv_from_any(live).await)
}

/// Опрашивает все подписки; для этого используется `select_all` поверх
/// фьючей `recv`, каждый из которых помечен origin-ом.
async fn recv_from_any(live: &mut HashMap<Origin, Live>) -> (Origin, InstanceEvent) {
    let futures: Vec<_> = live
        .iter_mut()
        .map(|(origin, entry)| {
            let origin = origin.clone();
            Box::pin(async move {
                let event = entry.sub.recv().await;
                (origin, event)
            })
        })
        .collect();
    let ((origin, event), _, _) = futures_util::future::select_all(futures).await;
    let event = event.unwrap_or_else(|err| {
        // Закрытый/отставший поток трактуем как потерю связи: диспетчер
        // инстанса сам решит, переподключаться ли.
        tracing::warn!(?err, "поток событий инстанса прерван");
        InstanceEvent::StateChanged(ConnectionState::Stopped)
    });
    (origin, event)
}

/// Пересылает событие инстанса в TUI. [`ControlFlow::Break`] — пора завершаться.
fn relay_event(
    origin: Origin,
    event: InstanceEvent,
    events: &std_mpsc::Sender<NetEvent>,
) -> std::ops::ControlFlow<()> {
    let outgoing = match event {
        InstanceEvent::Server(event) => NetEvent::Server { origin, event },
        InstanceEvent::StateChanged(state) => NetEvent::State { origin, state },
        InstanceEvent::AuthRejected {
            failure, message, ..
        } => NetEvent::Failed {
            origin,
            message: format!("{message} ({failure:?})"),
        },
    };
    if events.send(outgoing).is_err() {
        return std::ops::ControlFlow::Break(());
    }
    std::ops::ControlFlow::Continue(())
}

/// Выполняет REST-запрос и превращает результат в [`NetPayload`].
async fn handle_request(client: &bs_api::ApiClient, request: NetRequest) -> NetPayload {
    match request {
        NetRequest::Friends => match client.friends().await {
            Ok(friends) => NetPayload::Friends(friends),
            Err(err) => NetPayload::Failed(err.to_string()),
        },
        NetRequest::FriendRequests => match client.friend_requests().await {
            Ok(requests) => NetPayload::FriendRequests(requests),
            Err(err) => NetPayload::Failed(err.to_string()),
        },
        NetRequest::SearchUsers(query) => match client.search_users(&query).await {
            Ok(users) => NetPayload::Users(users),
            Err(err) => NetPayload::Failed(err.to_string()),
        },
        NetRequest::DmChannels => match client.dm_channels().await {
            Ok(channels) => NetPayload::DmChannels(channels),
            Err(err) => NetPayload::Failed(err.to_string()),
        },
        NetRequest::OpenDm(request) => match client.open_dm(&request).await {
            Ok(channel) => NetPayload::DmChannel(Box::new(channel)),
            Err(err) => NetPayload::Failed(err.to_string()),
        },
        NetRequest::LiveKitTokenChannel(channel_id) => {
            match client.livekit_token_channel(&channel_id).await {
                Ok(token) => NetPayload::LiveKitToken(token),
                Err(err) => NetPayload::Failed(err.to_string()),
            }
        }
        NetRequest::LiveKitTokenDm(dm_channel_id) => {
            match client.livekit_token_dm(&dm_channel_id).await {
                Ok(token) => NetPayload::LiveKitToken(token),
                Err(err) => NetPayload::Failed(err.to_string()),
            }
        }
        NetRequest::SendFriendRequest(request) => {
            match client.send_friend_request(&request).await {
                Ok(_) => NetPayload::Done,
                Err(err) => NetPayload::Failed(err.to_string()),
            }
        }
        NetRequest::RespondFriendRequest(id, decision) => {
            match client.respond_friend_request(&id, decision).await {
                Ok(()) => NetPayload::Done,
                Err(err) => NetPayload::Failed(err.to_string()),
            }
        }
        NetRequest::CancelFriendRequest(id) => match client.cancel_friend_request(&id).await {
            Ok(()) => NetPayload::Done,
            Err(err) => NetPayload::Failed(err.to_string()),
        },
        NetRequest::RemoveFriend(user_id) => match client.remove_friend(&user_id).await {
            Ok(()) => NetPayload::Done,
            Err(err) => NetPayload::Failed(err.to_string()),
        },
    }
}
