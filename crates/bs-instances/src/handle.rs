//! `InstanceHandle` — живое подключение к одному инстансу.
//!
//! Держит авторизованный HTTP-клиент, WebSocket-соединение и фоновую
//! задачу, которая автоматически переподключается с backoff.

use std::time::Duration;

use bs_api::ApiClient;
use bs_identity::{AccessToken, CredentialStore, IdentityError, InstanceSession};
use bs_types::{ClientEvent, ClientKind, Origin, ServerEvent, User};
use bs_ws::{WsConnection, WsError};
use tokio::sync::{broadcast, mpsc, watch};

use crate::error::InstanceError;
use crate::reconnect::ReconnectPolicy;

/// Сколько ждать первого `ready` (с учётом возможных ретраев).
pub const INITIAL_READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Размер буфера широковещательных событий.
const EVENT_BUFFER: usize = 256;

/// Размер очереди исходящих команд.
const COMMAND_BUFFER: usize = 64;

/// Текущее состояние соединения инстанса.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// Идёт первичное подключение.
    Connecting,
    /// Соединение активно.
    Connected,
    /// Разрыв; идёт ожидание перед попыткой номер `attempt`.
    Reconnecting {
        /// Номер предстоящей попытки (с нуля).
        attempt: u32,
    },
    /// Диспетчер остановлен (shutdown или фатальная ошибка auth).
    Stopped,
}

/// Событие, которое `InstanceHandle` рассылает подписчикам.
#[derive(Debug, Clone)]
pub enum InstanceEvent {
    /// Сменилось состояние соединения.
    StateChanged(ConnectionState),
    /// Пришло событие сервера (включая первый `ready`).
    Server(Box<ServerEvent>),
    /// Сервер отклонил токен; переподключение не поможет.
    AuthRejected {
        /// Сообщение сервера.
        message: String,
        /// Машинный код, если был.
        code: Option<String>,
    },
}

/// Команды фоновому диспетчеру.
enum Command {
    Send(ClientEvent),
    Shutdown,
}

/// Живое подключение к инстансу Backspace.
#[derive(Debug)]
pub struct InstanceHandle {
    origin: Origin,
    base: String,
    user: User,
    token: AccessToken,
    client: ApiClient,
    commands: mpsc::Sender<Command>,
    events: broadcast::Sender<InstanceEvent>,
    state: watch::Receiver<ConnectionState>,
    task: tokio::task::JoinHandle<()>,
}

impl InstanceHandle {
    /// Логинится и поднимает живое подключение.
    ///
    /// Комбинирует [`InstanceSession::login`] (сохраняет токен в `store`)
    /// с запуском диспетчера WebSocket. Возвращает handle и первый `ready`.
    pub async fn login(
        store: &mut CredentialStore,
        origin: Origin,
        base: &str,
        username: &str,
        password: &str,
        client_kind: Option<ClientKind>,
        policy: ReconnectPolicy,
    ) -> Result<(Self, ServerEvent), InstanceError> {
        let session = InstanceSession::login(store, origin, base, username, password).await?;
        Self::from_session(session, client_kind, policy).await
    }

    /// Поднимает подключение из уже готовой сессии.
    pub async fn from_session(
        session: InstanceSession,
        client_kind: Option<ClientKind>,
        policy: ReconnectPolicy,
    ) -> Result<(Self, ServerEvent), InstanceError> {
        let origin = session.origin().clone();
        let base = session.client().base().to_owned();
        let user = session.user().clone();
        let token = session.token().clone();
        let client = session.client().clone();

        Self::start(origin, base, user, token, client, client_kind, policy).await
    }

    async fn start(
        origin: Origin,
        base: String,
        user: User,
        token: AccessToken,
        client: ApiClient,
        client_kind: Option<ClientKind>,
        policy: ReconnectPolicy,
    ) -> Result<(Self, ServerEvent), InstanceError> {
        let (event_tx, _) = broadcast::channel(EVENT_BUFFER);
        // Подписываемся ДО запуска диспетчера, чтобы не потерять первый `ready`.
        let mut initial_rx = event_tx.subscribe();
        let (state_tx, state_rx) = watch::channel(ConnectionState::Connecting);
        let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_BUFFER);

        let base_for_task = base.clone();
        let token_for_task = token.expose().to_owned();
        let evt_for_task = event_tx.clone();
        let task = tokio::spawn(async move {
            run_dispatcher(
                base_for_task,
                token_for_task,
                client_kind,
                policy,
                cmd_rx,
                evt_for_task,
                state_tx,
            )
            .await;
        });

        let ready = wait_for_ready(&mut initial_rx).await?;

        let handle = Self {
            origin,
            base,
            user,
            token,
            client,
            commands: cmd_tx,
            events: event_tx,
            state: state_rx,
            task,
        };
        Ok((handle, ready))
    }

    /// Инстанс подключения.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// Базовый URL инстанса.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Профиль пользователя на момент логина.
    pub fn user(&self) -> &User {
        &self.user
    }

    /// Токен доступа.
    pub fn token(&self) -> &AccessToken {
        &self.token
    }

    /// Авторизованный HTTP-клиент.
    pub fn client(&self) -> &ApiClient {
        &self.client
    }

    /// Текущее состояние соединения.
    pub fn state(&self) -> ConnectionState {
        self.state.borrow().clone()
    }

    /// Подписка на поток событий (включая смены состояния).
    pub fn subscribe(&self) -> broadcast::Receiver<InstanceEvent> {
        self.events.subscribe()
    }

    /// Отправляет событие на сервер.
    ///
    /// Если соединение сейчас в реконнекте, команда будет отброшена и
    /// вернётся [`InstanceError::DispatcherClosed`].
    pub async fn send(&self, event: ClientEvent) -> Result<(), InstanceError> {
        self.commands
            .send(Command::Send(event))
            .await
            .map_err(|_| InstanceError::DispatcherClosed)
    }

    /// Останавливает диспетчер и дожидается его завершения.
    pub async fn shutdown(self) -> Result<(), InstanceError> {
        // Игнорируем ошибку, если диспетчер уже завершился сам.
        let _ = self.commands.send(Command::Shutdown).await;
        self.task.await?;
        Ok(())
    }
}

/// Ждёт первый `ready`, отсекая фатальные ошибки аутентификации.
async fn wait_for_ready(
    rx: &mut broadcast::Receiver<InstanceEvent>,
) -> Result<ServerEvent, InstanceError> {
    let fut = async {
        loop {
            match rx.recv().await {
                Ok(InstanceEvent::Server(boxed)) if matches!(*boxed, ServerEvent::Ready { .. }) => {
                    return Ok(*boxed);
                }
                Ok(InstanceEvent::AuthRejected { message, code }) => {
                    tracing::warn!(%message, ?code, "server rejected ws auth");
                    return Err(InstanceError::Identity(IdentityError::TokenRejected));
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(InstanceError::DispatcherClosed);
                }
            }
        }
    };
    tokio::time::timeout(INITIAL_READY_TIMEOUT, fut)
        .await
        .map_err(|_| InstanceError::DispatcherClosed)?
}

/// Устанавливает состояние через watch-канал (ошибки отправки игнорируются).
fn set_state(tx: &watch::Sender<ConnectionState>, state: ConnectionState) {
    let _ = tx.send(state);
}

#[allow(clippy::too_many_arguments)]
async fn run_dispatcher(
    base: String,
    token: String,
    client_kind: Option<ClientKind>,
    policy: ReconnectPolicy,
    mut commands: mpsc::Receiver<Command>,
    events: broadcast::Sender<InstanceEvent>,
    state: watch::Sender<ConnectionState>,
) {
    let mut attempt: u32 = 0;
    loop {
        set_state(
            &state,
            if attempt == 0 {
                ConnectionState::Connecting
            } else {
                ConnectionState::Reconnecting { attempt }
            },
        );

        match WsConnection::connect(&base, &token, client_kind).await {
            Ok((connection, ready)) => {
                attempt = 0;
                set_state(&state, ConnectionState::Connected);
                let _ = events.send(InstanceEvent::Server(Box::new(ready)));

                let parts = connection.into_parts();
                let sender = parts.sender;
                let mut receiver = parts.receiver;
                let task = parts.task;

                loop {
                    tokio::select! {
                        cmd = commands.recv() => match cmd {
                            Some(Command::Send(event)) => {
                                if sender.send(event).await.is_err() {
                                    break;
                                }
                            }
                            Some(Command::Shutdown) | None => {
                                drop(sender);
                                let _ = task.await;
                                set_state(&state, ConnectionState::Stopped);
                                return;
                            }
                        },
                        event = receiver.recv() => match event {
                            Some(event) => {
                                if events.send(InstanceEvent::Server(Box::new(event))).is_err() {
                                    // Нет подписчиков — не повод рвать соединение.
                                }
                            }
                            None => break,
                        },
                    }
                }
                let _ = task.await;
            }
            Err(WsError::AuthRejected { message, code }) => {
                let _ = events.send(InstanceEvent::AuthRejected { message, code });
                set_state(&state, ConnectionState::Stopped);
                return;
            }
            Err(err) => {
                tracing::warn!(%err, base = %base, "ws connect failed; will retry");
            }
        }

        // Backoff-пауза с возможностью прерваться по shutdown.
        let delay = policy.delay(attempt);
        attempt = attempt.saturating_add(1);
        set_state(&state, ConnectionState::Reconnecting { attempt });
        wait_backoff(delay, &mut commands, &state).await;
        if state.borrow().eq(&ConnectionState::Stopped) {
            return;
        }
    }
}

/// Ждёт `delay`, реагируя на shutdown; прочие команды отбрасывает.
async fn wait_backoff(
    delay: Duration,
    commands: &mut mpsc::Receiver<Command>,
    state: &watch::Sender<ConnectionState>,
) {
    let deadline = tokio::time::Instant::now() + delay;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => return,
            cmd = commands.recv() => match cmd {
                Some(Command::Shutdown) | None => {
                    set_state(state, ConnectionState::Stopped);
                    return;
                }
                Some(Command::Send(_)) => {}
            },
        }
    }
}
