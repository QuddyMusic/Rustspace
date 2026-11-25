//! Сессия инстанса: живое соединение + кэш состояния.
//!
//! [`CoreSession`] — тонкая обёртка над [`InstanceHandle`], которая держит
//! [`Cache`] в актуальном состоянии, применяя события из broadcast-потока.
//! UI-слой вызывает [`CoreSession::drain`] (неблокирующе, из кадра) или
//! [`CoreSession::pump_one`] (блокирующе, из фоновой задачи).

use bs_instances::{InstanceEvent, InstanceHandle};
use bs_rt::sync::broadcast;
use bs_types::{ClientEvent, ServerEvent};

use crate::apply::apply;
use crate::error::CoreError;
use crate::state::Cache;

/// Сколько событий максимум обрабатывается за один `drain`.
///
/// Защита от вечного цикла, если события идут быстрее, чем UI рисует кадр.
pub const DRAIN_LIMIT: usize = 512;

/// Подключённая сессия инстанса с кэшем.
#[derive(Debug)]
pub struct CoreSession {
    handle: InstanceHandle,
    cache: Cache,
    events: broadcast::Receiver<InstanceEvent>,
}

impl CoreSession {
    /// Оборачивает уже подключённый [`InstanceHandle`].
    ///
    /// `ready` — первый снимок состояния, полученный при подключении
    /// (его возвращает [`InstanceHandle::login`]). Из него строится кэш;
    /// сам `ready` повторно из потока не приходит.
    #[must_use]
    pub fn new(handle: InstanceHandle, ready: ServerEvent) -> Self {
        let events = handle.subscribe();
        let cache = match ready {
            ServerEvent::Ready(payload) => Cache::from_ready(*payload),
            // Контракт: сюда передают именно `Ready`. Прочие события просто
            // подкладываются в пустой кэш, который затем заполнится дельтами.
            _ => Cache::from_ready(empty_ready(handle.user().clone())),
        };
        Self {
            handle,
            cache,
            events,
        }
    }

    /// Кэш состояния (только чтение).
    pub const fn cache(&self) -> &Cache {
        &self.cache
    }

    /// Нижележащее соединение.
    #[must_use]
    pub const fn handle(&self) -> &InstanceHandle {
        &self.handle
    }

    /// Отправляет событие на сервер.
    pub async fn send(&self, event: ClientEvent) -> Result<(), CoreError> {
        self.handle.send(event).await.map_err(CoreError::from)
    }

    /// Обрабатывает одно событие из потока (блокирующе ждёт следующего).
    ///
    /// Возвращает `true`, если кэш изменился.
    ///
    /// # Errors
    /// [`CoreError::EventStreamClosed`], если соединение окончательно закрыто.
    pub async fn pump_one(&mut self) -> Result<bool, CoreError> {
        loop {
            match self.events.recv().await {
                Ok(InstanceEvent::Server(event)) => return Ok(self.apply(*event)),
                Ok(InstanceEvent::StateChanged(_) | InstanceEvent::AuthRejected { .. }) => {}
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "пропущены события инстанса");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(CoreError::EventStreamClosed);
                }
            }
        }
    }

    /// Неблокирующе обрабатывает все доступные события (для кадра UI).
    ///
    /// Возвращает `true`, если кэш изменился. Не считает закрытый поток
    /// ошибкой — просто перестаёт читать.
    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        for _ in 0..DRAIN_LIMIT {
            match self.events.try_recv() {
                Ok(InstanceEvent::Server(event)) => changed |= self.apply(*event),
                Ok(InstanceEvent::StateChanged(_) | InstanceEvent::AuthRejected { .. }) => {}
                Err(
                    broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed,
                ) => break,
                Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "пропущены события инстанса");
                }
            }
        }
        changed
    }

    /// Применяет одно событие к кэшу.
    fn apply(&mut self, event: ServerEvent) -> bool {
        apply(&mut self.cache, event)
    }

    /// Завершает соединение.
    ///
    /// # Errors
    /// Ошибка диспетчера инстанса при остановке.
    pub async fn shutdown(self) -> Result<(), CoreError> {
        self.handle.shutdown().await.map_err(CoreError::from)
    }
}

/// Пустой `ready`-payload — фолбэк для [`CoreSession::new`].
fn empty_ready(user: bs_types::User) -> bs_types::ReadyPayload {
    bs_types::ReadyPayload {
        user,
        spaces: Vec::new(),
        dm_channels: Vec::new(),
        folders: None,
        space_layout: None,
        layout_updated_at: None,
        voice_states: None,
        voice_channel_elapsed_seconds: None,
        voice_user_states: None,
        supports_poke: None,
        unread_counts: None,
        read_states: None,
        active_calls: None,
        space_voice_states: None,
        user_activities: None,
        user_activity_identities: None,
        rejected_peer_origins: None,
        awaiting_approval_peer_origins: None,
        active_peer_origins: None,
        pending_approval_count: None,
    }
}
