//! Склейка: состояние приложения, стор, команды.
//!
//! Этап 1: кэш состояния, применение событий и живая сессия инстанса.
//!
//! # Пример
//!
//! Кэш строится из `ready`-payload, затем к нему применяются события:
//!
//! ```no_run
//! use bs_core::{apply, Cache};
//! use bs_types::{ReadyPayload, ServerEvent};
//!
//! # fn demo(ready: ReadyPayload, event: ServerEvent) {
//! let mut cache = Cache::from_ready(ready);
//! let changed = apply(&mut cache, event);
//! if changed {
//!     println!("каналов: {}", cache.all_channels().count());
//! }
//! # }
//! ```

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod apply;
mod error;
mod federation;
mod multicache;
pub mod notifications;
mod session;
mod state;

pub use apply::{apply, apply_at};
pub use error::CoreError;
pub use federation::{
    ReattachDecision, RecoveryAction, ensure_remote_credential, maybe_auto_reattach,
    peer_home_with_remote, recovery_action,
};
pub use multicache::{DrainStats, MultiCache, Routed};
pub use session::{CoreSession, DRAIN_LIMIT};
pub use state::{Cache, TYPING_TTL_MS, TypingEntry};
