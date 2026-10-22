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
                runtime.block_on(run_session(creds, &events_tx, commands_rx));
            })
            .ok();

        Self {
            events,
            commands,
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

async fn run_session(
    creds: Credentials,
    events: &std_mpsc::Sender<NetEvent>,
    mut commands: tokio_mpsc::UnboundedReceiver<ClientEvent>,
) {
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
