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

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod client;
pub mod error;

pub use client::{CLIENT_PING_INTERVAL, READY_TIMEOUT, WsConnection, WsParts, WsSender};
pub use error::WsError;
