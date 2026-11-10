//! Типы данных протокола Backspace.
//!
//! Все типы здесь повторяют контракты `@backspace/shared` из TypeScript-версии.
//! Правила сериализации:
//!
//! - Snowflake ID — **строки** (не числа).
//! - Права и permission-versions — **decimal string**, не JSON-число.
//! - Поля в JSON — `camelCase`.
//! - Время — **epoch-миллисекунды** (`i64`), не ISO-строки: сервер отдаёт числа.
//! - Неизвестные варианты WS-событий не должны ломать парсинг: используем
//!   `#[serde(other)]` fallback-варианты.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod channel;
pub mod dm_system;
pub mod error;
pub mod events;
pub mod id;
pub mod message;
pub mod origin;
pub mod search;
pub mod social;
pub mod space;
pub mod transport;
pub mod user;

pub use channel::*;
pub use dm_system::{DmSystemEvent, GENERIC_SYSTEM_TEXT, SpaceInvite, SpaceInviteSnapshot};
pub use error::*;
pub use events::*;
pub use id::*;
pub use message::*;
pub use origin::*;
pub use search::*;
pub use social::*;
pub use space::*;
pub use transport::{TransportError, TransportPolicy};
pub use user::*;

/// Максимальная длина одного сообщения в символах.
pub const MAX_MESSAGE_LENGTH: usize = 4000;

/// Палитра цветов аватара (`AVATAR_COLORS` в TS) — имена, не hex-коды.
pub const AVATAR_COLORS: [&str; 7] = ["mint", "sky", "lavender", "coral", "rose", "teal", "amber"];
