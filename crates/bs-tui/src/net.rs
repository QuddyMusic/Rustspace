//! Фоновая сессия: tokio-рантайм в отдельном потоке, общение с TUI через каналы.
//!
//! TUI-поток не должен блокироваться на сети, а терминал нельзя трогать из
//! tokio-задачи. Поэтому здесь живёт [`NetWorker`]: он держит не более
//! одного [`InstanceHandle`] (вход в несколько инстансов одновременно
//! запрещён), пересылает его события и состояния наружу, а внутрь принимает
//! команды.

use std::sync::mpsc as std_mpsc;
use std::thread;

use bs_identity::CredentialStore;
use bs_instances::{ConnectionState, InstanceEvent, InstanceHandle, ReconnectPolicy};
use bs_types::{ClientEvent, ClientKind, Origin, ServerEvent, TransportPolicy};
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
        /// Можно ли открытое `http://` вне loopback.
        transport: TransportPolicy,
    },
    /// Отключиться от инстанса.
    Disconnect,
    /// Отправить событие серверу инстанса.
    Send {
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
    ///
    /// Если инстанс уже подключён, воркер откажет событием
    /// [`NetEvent::Failed`]: параллельных входов не бывает.
    pub fn connect(&self, origin: Origin, creds: Credentials, transport: TransportPolicy) -> bool {
        self.commands
            .send(Command::Connect {
                origin,
                creds,
                transport,
            })
            .is_ok()
    }

    /// Отключается от инстанса. `false` — сессия уже мертва.
    pub fn disconnect(&self) -> bool {
        self.commands.send(Command::Disconnect).is_ok()
    }

    /// Отправляет команду серверу инстанса. `false` — сессия уже мертва.
    pub fn send(&self, event: ClientEvent) -> bool {
        self.commands.send(Command::Send { event }).is_ok()
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

/// Единственный подключённый инстанс фоновой сессии.
struct Live {
    origin: Origin,
    handle: InstanceHandle,
    sub: tokio::sync::broadcast::Receiver<InstanceEvent>,
}

/// Состояние фоновой сессии между командами.
struct Session {
    store: CredentialStore,
    /// Не более одного инстанса: это структурный запрет на параллельный вход.
    live: Option<Live>,
}

impl Session {
    fn new() -> Self {
        Self {
            store: CredentialStore::new(),
            live: None,
        }
    }

    /// Подключает инстанс: логин и запуск диспетчера.
    ///
    /// Если сессия уже есть, отказывает, не трогая ни её, ни сеть.
    async fn connect(
        &mut self,
        origin: Origin,
        creds: Credentials,
        transport: TransportPolicy,
        events: &std_mpsc::Sender<NetEvent>,
    ) {
        if self.live.is_some() {
            let _ = events.send(NetEvent::Failed {
                origin,
                message: "Сначала выйдите из текущего инстанса".to_owned(),
            });
            return;
        }

        let login = InstanceHandle::login_with_transport(
            &mut self.store,
            origin.clone(),
            &creds.base,
            &creds.username,
            &creds.password,
            Some(ClientKind::Desktop),
            ReconnectPolicy::default(),
            transport,
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
        self.live = Some(Live {
            origin: origin.clone(),
            handle,
            sub,
        });

        let _ = events.send(NetEvent::Ready {
            origin,
            event: Box::new(ready),
        });
    }

    /// Отключает инстанс, если он есть.
    async fn disconnect(&mut self, events: &std_mpsc::Sender<NetEvent>) {
        if let Some(live) = self.live.take() {
            let origin = live.origin.clone();
            let _ = live.handle.shutdown().await;
            let _ = events.send(NetEvent::Disconnected { origin });
        }
    }

    /// Гасит подключение при завершении сессии.
    async fn shutdown(&mut self) {
        if let Some(live) = self.live.take() {
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
                Some(Command::Connect { origin, creds, transport }) => {
                    session.connect(origin, creds, transport, events).await;
                }
                Some(Command::Disconnect) => {
                    session.disconnect(events).await;
                }
                Some(Command::Send { event }) => {
                    if let Some(live) = session.live.as_ref()
                        && let Err(err) = live.handle.send(event).await
                    {
                        tracing::warn!(%err, "не удалось отправить команду");
                    }
                }
                Some(Command::Request { origin, request }) => {
                    let Some(live) = session.live.as_ref() else {
                        let _ = responses.send(NetResponse {
                            origin,
                            payload: NetPayload::Failed("инстанс не подключён".to_owned()),
                        });
                        continue;
                    };
                    let payload = handle_request(live.handle.client(), request).await;
                    if responses.send(NetResponse { origin, payload }).is_err() {
                        session.shutdown().await;
                        return;
                    }
                }
                None => {
                    session.shutdown().await;
                    return;
                }
            },
            received = recv_live(&mut session.live) => {
                if relay_event(received.0, received.1, events).is_break() {
                    session.shutdown().await;
                    return;
                }
            }
        }
    }
}

/// Ждёт событие подключённого инстанса.
///
/// Пока инстанса нет, ветка «спит», чтобы `select!` не крутился вхолостую.
async fn recv_live(live: &mut Option<Live>) -> (Origin, InstanceEvent) {
    let Some(entry) = live.as_mut() else {
        return std::future::pending().await;
    };
    let event = entry.sub.recv().await.unwrap_or_else(|err| {
        // Закрытый/отставший поток трактуем как потерю связи: диспетчер
        // инстанса сам решит, переподключаться ли.
        tracing::warn!(?err, "поток событий инстанса прерван");
        InstanceEvent::StateChanged(ConnectionState::Stopped)
    });
    (entry.origin.clone(), event)
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
