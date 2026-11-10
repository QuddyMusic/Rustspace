//! Объединённый поток событий по всем инстансам.
//!
//! В вебе `useWebSocket` держит `Map<origin, ConnectionState>` и складывает
//! события всех соединений в один обработчик. Здесь это [`InstanceRegistry`](crate::InstanceRegistry)
//! с мультиплексором: каждое событие помечается origin-ом, из которого оно
//! пришло, чтобы вызывающий код мог разложить его в кэш по инстансам.
//!
//! Таск мультиплексора завершается сам, когда реестр теряет подписки или
//! handle удалён.

use std::collections::HashMap;
use std::sync::Arc;

use bs_types::Origin;
use tokio::sync::broadcast;

use crate::handle::{ConnectionState, InstanceEvent};

/// Размер буфера мультиплексированного канала.
const MULTIPLEX_BUFFER: usize = 512;

/// Событие одного инстанса, помеченное origin-ом.
#[derive(Debug, Clone)]
pub struct OriginEvent {
    /// Инстанс, от которого пришло событие.
    pub origin: Origin,
    /// Само событие подключения.
    pub event: InstanceEvent,
}

/// Источник событий всех инстансов реестра.
#[derive(Debug)]
pub struct EventMultiplexer {
    tx: broadcast::Sender<OriginEvent>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl EventMultiplexer {
    /// Создаёт пустой мультиплексор.
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(MULTIPLEX_BUFFER);
        Self {
            tx,
            tasks: Vec::new(),
        }
    }

    /// Подписывается на события одного handle.
    pub fn attach(&mut self, origin: &Origin, mut rx: broadcast::Receiver<InstanceEvent>) {
        let tx = self.tx.clone();
        let origin = origin.clone();
        let task = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        // Нет подписчиков — не повод рвать пересылку.
                        let _ = tx.send(OriginEvent {
                            origin: origin.clone(),
                            event,
                        });
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        self.tasks.push(task);
    }

    /// Подписка на объединённый поток.
    pub fn subscribe(&self) -> broadcast::Receiver<OriginEvent> {
        self.tx.subscribe()
    }

    /// Число активных пересылок.
    #[must_use]
    pub fn attached(&self) -> usize {
        self.tasks.len()
    }

    /// Число подписчиков объединённого потока.
    #[must_use]
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Останавливает пересылку.
    ///
    /// Задачи пересылки прерываются, а не дожидаются: источник (`handle`)
    /// может оставаться живым, и его `broadcast::Receiver` никогда не
    /// закроется сам — ожидание привело бы к вечному зависанию.
    pub async fn shutdown(self) {
        drop(self.tx);
        for task in self.tasks {
            task.abort();
            // Отмена асинхронна; дожидаемся её, чтобы не оставлять «хвостов».
            let _ = task.await;
        }
    }
}

impl Default for EventMultiplexer {
    fn default() -> Self {
        Self::new()
    }
}

/// Сводка состояния подключений по инстансам.
///
/// Удобна для UI: переключатель инстансов и индикаторы связи.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryStatus {
    /// Состояние каждого инстанса.
    pub states: HashMap<Origin, ConnectionState>,
}

impl RegistryStatus {
    /// Число инстансов в состоянии `Connected`.
    #[must_use]
    pub fn connected_count(&self) -> usize {
        self.states
            .values()
            .filter(|s| **s == ConnectionState::Connected)
            .count()
    }

    /// Общее число известных инстансов.
    #[must_use]
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// Нет ни одного инстанса.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// Сводка по общему числу инстансов.
    #[must_use]
    pub fn summary(&self) -> String {
        format!("{}/{}", self.connected_count(), self.len())
    }
}

/// Общая ссылка на мультиплексор — реестр отдаёт её клонами.
pub type SharedMultiplexer = Arc<EventMultiplexer>;

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{Duration, timeout};

    #[test]
    fn registry_status_counts_connected() {
        let mut map = HashMap::new();
        map.insert(Origin::Home, ConnectionState::Connected);
        map.insert(
            Origin::parse_web_str("https://nova.ddns.net").unwrap(),
            ConnectionState::Reconnecting { attempt: 2 },
        );
        let status = RegistryStatus { states: map };
        assert_eq!(status.connected_count(), 1);
        assert_eq!(status.len(), 2);
        assert!(!status.is_empty());
        assert_eq!(status.summary(), "1/2");
    }

    #[tokio::test]
    async fn multiplexer_relays_with_origin() {
        let mut mux = EventMultiplexer::new();
        let (handle_tx, handle_rx) = broadcast::channel(8);
        let origin = Origin::parse_web_str("https://nova.ddns.net").unwrap();
        mux.attach(&origin, handle_rx);
        assert_eq!(mux.attached(), 1);

        let mut rx = mux.subscribe();
        handle_tx
            .send(InstanceEvent::StateChanged(ConnectionState::Connected))
            .unwrap();

        let got = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("event arrives")
            .expect("not lagged");
        assert_eq!(got.origin, origin);
        assert!(matches!(
            got.event,
            InstanceEvent::StateChanged(ConnectionState::Connected)
        ));

        mux.shutdown().await;
    }

    #[tokio::test]
    async fn multiplexer_shutdown_finishes_while_source_alive() {
        // Регресс-тест: источник (handle) остаётся живым, его Receiver не
        // закрывается — shutdown обязан прервать пересылку, а не ждать её.
        let mut mux = EventMultiplexer::new();
        let (_handle_tx, handle_rx) = broadcast::channel::<InstanceEvent>(8);
        mux.attach(&Origin::Home, handle_rx);
        let _rx = mux.subscribe();

        timeout(Duration::from_secs(1), mux.shutdown())
            .await
            .expect("shutdown must not hang on a live source");
    }
}
