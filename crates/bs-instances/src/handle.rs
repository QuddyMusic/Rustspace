//! `InstanceHandle` — живое подключение к одному инстансу.
//!
//! Держит авторизованный HTTP-клиент, WebSocket-соединение и фоновую
//! задачу, которая автоматически переподключается с backoff.
//!
//! Токен можно заменить на лету ([`InstanceHandle::replace_token`]):
//! диспетчер увидит новое значение, сбросит текущее соединение и
//! переподключится с новым JWT. Это путь для `change_password` на
//! удалённом инстансе и для `reattach` после отсоединения.

use std::time::Duration;

use bs_api::ApiClient;
use bs_identity::{AccessToken, CredentialStore, IdentityError, InstanceSession};
use bs_rt::sync::{broadcast, mpsc, watch};
use bs_types::{ClientEvent, ClientKind, Origin, ServerEvent, TransportPolicy, User};
use bs_ws::{AuthFailure, WsConnection, WsError};

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
    /// Сервер отклонил токен.
    ///
    /// Для восстановимых причин ([`AuthFailure::is_recoverable`]) диспетчер
    /// остаётся живым и ждёт [`InstanceHandle::replace_token`]; для
    /// фатальных ([`AuthFailure::is_fatal`]) он останавливается.
    AuthRejected {
        /// Классифицированная причина отказа.
        failure: AuthFailure,
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
    token: watch::Sender<AccessToken>,
    client: ApiClient,
    commands: mpsc::Sender<Command>,
    events: broadcast::Sender<InstanceEvent>,
    state: watch::Receiver<ConnectionState>,
    task: bs_rt::JoinHandle<()>,
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
        Self::login_with_transport(
            store,
            origin,
            base,
            username,
            password,
            client_kind,
            policy,
            TransportPolicy::Strict,
        )
        .await
    }

    /// Как [`InstanceHandle::login`], но с явной политикой транспорта
    /// (разрешить ли открытый `http://` вне loopback).
    #[allow(clippy::too_many_arguments)]
    pub async fn login_with_transport(
        store: &mut CredentialStore,
        origin: Origin,
        base: &str,
        username: &str,
        password: &str,
        client_kind: Option<ClientKind>,
        policy: ReconnectPolicy,
        transport: TransportPolicy,
    ) -> Result<(Self, ServerEvent), InstanceError> {
        Self::login_with_factor(
            store,
            origin,
            base,
            username,
            password,
            None,
            client_kind,
            policy,
            transport,
        )
        .await
    }

    /// Как [`InstanceHandle::login_with_transport`], но со вторым фактором
    /// для аккаунтов с двухфакторной защитой.
    ///
    /// # Errors
    /// Как у входа без второго фактора.
    #[allow(clippy::too_many_arguments)]
    pub async fn login_with_factor(
        store: &mut CredentialStore,
        origin: Origin,
        base: &str,
        username: &str,
        password: &str,
        second_factor: Option<&str>,
        client_kind: Option<ClientKind>,
        policy: ReconnectPolicy,
        transport: TransportPolicy,
    ) -> Result<(Self, ServerEvent), InstanceError> {
        let session = InstanceSession::login_with_factor(
            store,
            origin,
            base,
            username,
            password,
            second_factor,
            transport,
        )
        .await?;
        Self::from_session(session, client_kind, policy).await
    }

    /// Поднимает подключение по сохранённому токену из `store` (без пароля).
    ///
    /// Токен проверяется через `GET /api/users/@me`; при `401` он удаляется
    /// из `store`, а вызывающий получает [`InstanceError::Identity`].
    #[allow(clippy::too_many_arguments)]
    pub async fn resume_with_transport(
        store: &mut CredentialStore,
        origin: Origin,
        base: &str,
        client_kind: Option<ClientKind>,
        policy: ReconnectPolicy,
        transport: TransportPolicy,
    ) -> Result<(Self, ServerEvent), InstanceError> {
        let session = InstanceSession::resume_with_policy(store, origin, base, transport).await?;
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
        let transport = client.policy();

        Self::start(
            origin,
            base,
            user,
            token,
            client,
            client_kind,
            policy,
            transport,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn start(
        origin: Origin,
        base: String,
        user: User,
        token: AccessToken,
        client: ApiClient,
        client_kind: Option<ClientKind>,
        policy: ReconnectPolicy,
        transport: TransportPolicy,
    ) -> Result<(Self, ServerEvent), InstanceError> {
        let (event_tx, _) = broadcast::channel(EVENT_BUFFER);
        // Подписываемся ДО запуска диспетчера, чтобы не потерять первый `ready`.
        let mut initial_rx = event_tx.subscribe();
        let (state_tx, state_rx) = watch::channel(ConnectionState::Connecting);
        let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_BUFFER);
        let (token_tx, token_rx) = watch::channel(token.clone());

        let base_for_task = base.clone();
        let evt_for_task = event_tx.clone();
        let task = bs_rt::spawn(async move {
            run_dispatcher(
                base_for_task,
                client_kind,
                policy,
                transport,
                cmd_rx,
                token_rx,
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
            token: token_tx,
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

    /// Текущий токен доступа (копия).
    #[must_use]
    pub fn token(&self) -> AccessToken {
        self.token.borrow().clone()
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

    /// Заменяет токен и заставляет диспетчер переподключиться с ним.
    ///
    /// Нужен после `POST /api/users/@me/change-password` на удалённом
    /// инстансе (сервер отзывает прежние сессии и выдаёт новый JWT) и
    /// после `POST /api/users/@me/reattach`.
    ///
    /// HTTP-клиент обновляется сразу; WebSocket переподключается, как
    /// только диспетчер увидит новое значение.
    ///
    /// # Ошибки
    /// [`InstanceError::DispatcherClosed`] — диспетчер уже завершился
    /// (например, после фатального отказа auth), и токен принять некому.
    pub fn replace_token(&mut self, token: AccessToken) -> Result<(), InstanceError> {
        self.client.set_token(Some(token.expose().to_owned()));
        self.token
            .send(token)
            .map_err(|_| InstanceError::DispatcherClosed)
    }

    /// Перепривязывает отсоединённый аккаунт к домашнему инстансу.
    ///
    /// Вызывает `POST /api/users/@me/reattach` на этом инстансе под его
    /// сессией, обновляет профиль и переподключает WebSocket с новым self-view.
    /// `token` — одноразовый токен из `POST /api/auth/attach-proof` домашнего
    /// инстанса.
    ///
    /// # Ошибки
    /// - [`InstanceError::Api`] — сервер отклонил перепривязку (403 — аккаунт
    ///   не отсоединён, 404 — не найден, 400 — плохой токен);
    /// - [`InstanceError::DispatcherClosed`] — диспетчер уже завершился.
    pub async fn reattach(&mut self, token: &str) -> Result<(), InstanceError> {
        let response = self.client.reattach(token).await?;
        self.user = response.user;
        // Токен сессии не меняется, но соединению нужен свежий `ready`
        // с обновлённым self-view — переподключаемся, переотправив тот же JWT.
        let current = self.token.borrow().clone();
        self.replace_token(current)
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
    pub async fn shutdown(mut self) -> Result<(), InstanceError> {
        // Игнорируем ошибку, если диспетчер уже завершился сам.
        let _ = self.commands.send(Command::Shutdown).await;
        (&mut self.task).await?;
        Ok(())
    }
}

impl Drop for InstanceHandle {
    /// Уничтожение handle завершает диспетчер немедленно.
    ///
    /// Иначе выход из инстанса (или `abort()` задачи, владеющей handle) оставил
    /// бы фоновое соединение жить до ближайшей проверки токена или backoff:
    /// формально сессия закрыта, а сокет ещё открыт.
    fn drop(&mut self) {
        self.task.abort();
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
                Ok(InstanceEvent::AuthRejected {
                    failure,
                    message,
                    code,
                }) => {
                    tracing::warn!(%message, ?code, ?failure, "server rejected ws auth");
                    return Err(InstanceError::Identity(IdentityError::TokenRejected));
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(InstanceError::DispatcherClosed);
                }
            }
        }
    };
    bs_rt::time::timeout(INITIAL_READY_TIMEOUT, fut)
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
    client_kind: Option<ClientKind>,
    policy: ReconnectPolicy,
    transport: TransportPolicy,
    mut commands: mpsc::Receiver<Command>,
    mut tokens: watch::Receiver<AccessToken>,
    events: broadcast::Sender<InstanceEvent>,
    state: watch::Sender<ConnectionState>,
) {
    let mut token = tokens.borrow_and_update().clone();
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

        match WsConnection::connect_with_policy(&base, token.expose(), client_kind, transport).await
        {
            Ok((connection, ready)) => {
                attempt = 0;
                set_state(&state, ConnectionState::Connected);
                let _ = events.send(InstanceEvent::Server(Box::new(ready)));

                let parts = connection.into_parts();
                let sender = parts.sender;
                let mut receiver = parts.receiver;
                let task = parts.task;
                let mut rotate = false;

                loop {
                    bs_rt::select! {
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
                        changed = tokens.changed() => {
                            if changed.is_err() {
                                // Владелец handle пропал — работать не для кого.
                                drop(sender);
                                let _ = task.await;
                                set_state(&state, ConnectionState::Stopped);
                                return;
                            }
                            token = tokens.borrow_and_update().clone();
                            rotate = true;
                            break;
                        }
                        event = receiver.recv() => match event {
                            Some(event) => {
                                // Нет подписчиков — не повод рвать соединение.
                                let _ = events.send(InstanceEvent::Server(Box::new(event)));
                            }
                            None => break,
                        },
                    }
                }
                let _ = task.await;
                if rotate {
                    // Новый токен: переподключаемся сразу, без backoff.
                    attempt = 0;
                    continue;
                }
            }
            Err(WsError::AuthFailed { failure, message }) => {
                let _ = events.send(InstanceEvent::AuthRejected {
                    failure,
                    message: message.clone(),
                    code: None,
                });
                if failure.is_fatal() {
                    set_state(&state, ConnectionState::Stopped);
                    return;
                }
                if failure.is_recoverable() {
                    // Ждём новый токен (reattach / смена пароля) или shutdown.
                    set_state(&state, ConnectionState::Reconnecting { attempt });
                    if !wait_for_token(&mut tokens, &mut commands, &state).await {
                        return;
                    }
                    token = tokens.borrow_and_update().clone();
                    attempt = 0;
                    continue;
                }
                tracing::warn!(%message, base = %base, "ws auth failed; will retry");
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

/// Ждёт новый токен или shutdown.
///
/// Возвращает `true`, если пришёл новый токен и пора переподключаться;
/// `false` — если нужно завершить диспетчер (shutdown или пропал владелец).
async fn wait_for_token(
    tokens: &mut watch::Receiver<AccessToken>,
    commands: &mut mpsc::Receiver<Command>,
    state: &watch::Sender<ConnectionState>,
) -> bool {
    loop {
        bs_rt::select! {
            changed = tokens.changed() => {
                if changed.is_err() {
                    set_state(state, ConnectionState::Stopped);
                    return false;
                }
                return true;
            }
            cmd = commands.recv() => match cmd {
                Some(Command::Shutdown) | None => {
                    set_state(state, ConnectionState::Stopped);
                    return false;
                }
                Some(Command::Send(_)) => {}
            },
        }
    }
}

/// Ждёт `delay`, реагируя на shutdown; прочие команды отбрасывает.
async fn wait_backoff(
    delay: Duration,
    commands: &mut mpsc::Receiver<Command>,
    state: &watch::Sender<ConnectionState>,
) {
    let deadline = bs_rt::time::Instant::now() + delay;
    loop {
        bs_rt::select! {
            () = bs_rt::time::sleep_until(deadline) => return,
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
