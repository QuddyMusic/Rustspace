//! Мультиинстансность: живые подключения к домашнему и федеративным
//! инстансам Backspace, автоматическое переподключение.
//!
//! Ключевые типы:
//! - [`InstanceHandle`] — подключение к одному инстансу: HTTP-клиент,
//!   WebSocket-соединение и фоновая задача с backoff-переподключением.
//! - [`InstanceRegistry`] — набор активных подключений по [`Origin`](bs_types::Origin).
//! - [`EventMultiplexer`] — объединённый поток событий всех инстансов
//!   с пометкой origin-ом (аналог общего обработчика в `useWebSocket.ts`).
//! - [`ReconnectPolicy`] — параметры экспоненциального backoff.
//!
//! # Пример
//! ```no_run
//! # async fn run() -> Result<(), bs_instances::InstanceError> {
//! use bs_instances::{InstanceHandle, ReconnectPolicy};
//! use bs_identity::CredentialStore;
//! use bs_types::{ClientKind, Origin};
//!
//! let mut store = CredentialStore::new();
//! let (handle, ready) = InstanceHandle::login(
//!     &mut store,
//!     Origin::Home,
//!     "https://chat.example.com",
//!     "alice",
//!     "secret",
//!     Some(ClientKind::Desktop),
//!     ReconnectPolicy::default(),
//! )
//! .await?;
//! # let _ = (handle, ready);
//! # Ok(())
//! # }
//! ```

#![cfg_attr(
    target_arch = "wasm32",
    allow(
        clippy::future_not_send,
        reason = "в браузере задачи живут в одном потоке"
    )
)]
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod error;
pub mod handle;
pub mod multiplex;
pub mod reconnect;
pub mod registry;

pub use error::InstanceError;
pub use handle::{ConnectionState, INITIAL_READY_TIMEOUT, InstanceEvent, InstanceHandle};
pub use multiplex::{EventMultiplexer, OriginEvent, RegistryStatus, SharedMultiplexer};
pub use reconnect::ReconnectPolicy;
pub use registry::InstanceRegistry;

#[cfg(test)]
mod tests;
