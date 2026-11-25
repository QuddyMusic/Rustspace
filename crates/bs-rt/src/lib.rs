//! Общий слой исполнения: на десктопе — tokio, в веб-версии (wasm32) —
//! очередь задач браузера, его таймеры и `performance.now()`.
//!
//! Каналы (`sync`) и `select!` берутся из tokio везде: в браузере они работают
//! без его рантайма. Всё, что зависит от потоков, таймеров и сети tokio, идёт
//! через этот крейт.

#![cfg_attr(
    target_arch = "wasm32",
    allow(
        clippy::future_not_send,
        reason = "в браузере задачи живут в одном потоке"
    )
)]
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

pub use tokio::select;
pub use tokio::sync;

/// `Send` на десктопе; в браузере задачи живут в одном потоке, и он не нужен.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}
/// `Send` на десктопе; в браузере задачи живут в одном потоке, и он не нужен.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

/// Задача завершилась не значением: её отменили или она упала.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinError {
    cancelled: bool,
}

impl JoinError {
    /// Задачу отменили ([`JoinHandle::abort`]).
    pub const fn is_cancelled(&self) -> bool {
        self.cancelled
    }
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.cancelled {
            "задача отменена"
        } else {
            "задача завершилась аварийно"
        })
    }
}

impl std::error::Error for JoinError {}

/// Ручка фоновой задачи: ждать результат, отменить, узнать, завершилась ли.
#[derive(Debug)]
pub struct JoinHandle<T> {
    #[cfg(not(target_arch = "wasm32"))]
    inner: tokio::task::JoinHandle<T>,
    #[cfg(target_arch = "wasm32")]
    result: sync::oneshot::Receiver<T>,
    #[cfg(target_arch = "wasm32")]
    abort: futures_util::future::AbortHandle,
}

impl<T> JoinHandle<T> {
    /// Отменяет задачу (ожидающий получит [`JoinError::is_cancelled`]).
    pub fn abort(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.inner.abort();
        #[cfg(target_arch = "wasm32")]
        self.abort.abort();
    }

    /// Задача уже завершилась (значением, отменой или аварийно).
    pub fn is_finished(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.inner.is_finished()
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.abort.is_aborted() || self.result.is_terminated()
        }
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Pin::new(&mut self.inner).poll(cx).map_err(|err| JoinError {
                cancelled: err.is_cancelled(),
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let cancelled = self.abort.is_aborted();
            Pin::new(&mut self.result)
                .poll(cx)
                .map_err(|_| JoinError { cancelled })
        }
    }
}

/// Запускает задачу в фоне.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + MaybeSend + 'static,
    F::Output: MaybeSend + 'static,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        JoinHandle {
            inner: tokio::spawn(future),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let (tx, result) = sync::oneshot::channel();
        let (abort, registration) = futures_util::future::AbortHandle::new_pair();
        let task = futures_util::future::Abortable::new(future, registration);
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(value) = task.await {
                let _ = tx.send(value);
            }
        });
        JoinHandle { result, abort }
    }
}

/// Время: монотонные часы, паузы и сроки.
pub mod time {
    use super::{Duration, Future};

    /// Монотонное время (`performance.now()` в браузере).
    #[cfg(not(target_arch = "wasm32"))]
    pub use std::time::Instant;
    /// Монотонное время (`performance.now()` в браузере).
    #[cfg(target_arch = "wasm32")]
    pub use web_time::Instant;

    /// Срок истёк раньше, чем будущее завершилось.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Elapsed;

    impl std::fmt::Display for Elapsed {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("время ожидания истекло")
        }
    }

    impl std::error::Error for Elapsed {}

    /// Пауза.
    pub async fn sleep(duration: Duration) {
        #[cfg(not(target_arch = "wasm32"))]
        tokio::time::sleep(duration).await;
        #[cfg(target_arch = "wasm32")]
        gloo_timers::future::sleep(duration).await;
    }

    /// Пауза до момента `deadline` (прошедший — сразу).
    pub async fn sleep_until(deadline: Instant) {
        sleep(deadline.saturating_duration_since(Instant::now())).await;
    }

    /// Ждёт `future`, но не дольше `duration`.
    pub async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output, Elapsed> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            tokio::time::timeout(duration, future)
                .await
                .map_err(|_| Elapsed)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let future = std::pin::pin!(future);
            let timer = std::pin::pin!(sleep(duration));
            match futures_util::future::select(future, timer).await {
                futures_util::future::Either::Left((value, _)) => Ok(value),
                futures_util::future::Either::Right(((), _)) => Err(Elapsed),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawned_task_returns_and_abort_cancels() {
        assert_eq!(spawn(async { 2 + 2 }).await, Ok(4));
        let slow = spawn(async {
            time::sleep(Duration::from_secs(30)).await;
        });
        slow.abort();
        assert!(slow.await.is_err_and(|err| err.is_cancelled()));
        assert_eq!(
            time::timeout(
                Duration::from_millis(10),
                time::sleep(Duration::from_secs(5))
            )
            .await,
            Err(time::Elapsed)
        );
    }
}
