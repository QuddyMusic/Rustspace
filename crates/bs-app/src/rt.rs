//! Где запускать фоновые задачи: на десктопе — в runtime tokio, в браузере —
//! в очереди задач страницы (через `bs-rt`).

use std::future::Future;

/// Ручка рантайма, через которую GUI запускает фоновые задачи.
#[derive(Debug, Clone)]
pub(crate) struct Handle {
    #[cfg(not(target_arch = "wasm32"))]
    inner: tokio::runtime::Handle,
}

impl Handle {
    /// Ручка рантайма tokio (десктоп).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) const fn new(inner: tokio::runtime::Handle) -> Self {
        Self { inner }
    }

    /// Ручка очереди задач страницы (браузер).
    #[cfg(target_arch = "wasm32")]
    pub(crate) const fn new() -> Self {
        Self {}
    }

    /// Сам runtime tokio — для библиотек, которым нужен именно он.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) const fn tokio(&self) -> &tokio::runtime::Handle {
        &self.inner
    }

    /// Запускает задачу в фоне.
    pub(crate) fn spawn<F>(&self, future: F) -> bs_rt::JoinHandle<F::Output>
    where
        F: Future + bs_rt::MaybeSend + 'static,
        F::Output: bs_rt::MaybeSend + 'static,
    {
        #[cfg(not(target_arch = "wasm32"))]
        let _inside = self.inner.enter();
        bs_rt::spawn(future)
    }
}
