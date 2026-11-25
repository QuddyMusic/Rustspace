//! WebSocket-клиент Backspace: соединение, рукопожатие `auth`, heartbeat.
//!
//! # Пример
//! ```no_run
//! # async fn run() -> Result<(), bs_ws::WsError> {
//! use bs_ws::WsConnection;
//!
//! let (mut conn, _ready) = WsConnection::connect("https://chat.example.com", "jwt", None).await?;
//! while let Some(event) = conn.recv().await {
//!     println!("{event:?}");
//! }
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
#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) в приватном модуле спорит с unreachable_pub"
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod auth;
pub mod client;
pub mod error;
mod transport;

pub use auth::AuthFailure;
pub use client::{CLIENT_PING_INTERVAL, READY_TIMEOUT, WsConnection, WsParts, WsSender};
pub use error::WsError;
