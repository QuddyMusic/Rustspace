//! Идентичность: логин, хранение токенов, сессии по инстансам.
//!
//! Крейт связывает [`bs_api`] (HTTP) и [`bs_types`] (доменные типы),
//! добавляя безопасное хранение JWT и per-origin сессии для
//! мультиинстансного клиента.
//!
//! # Пример
//! ```no_run
//! # async fn run() -> Result<(), bs_identity::IdentityError> {
//! use bs_identity::{CredentialStore, InstanceSession};
//! use bs_types::Origin;
//!
//! let mut store = CredentialStore::new();
//! let session = InstanceSession::login(
//!     &mut store,
//!     Origin::Home,
//!     "https://chat.example.com",
//!     "alice",
//!     "secret",
//! )
//! .await?;
//! let me = session.user();
//! # let _ = me;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod error;
pub mod identity;
pub mod persist;
pub mod secret;
pub mod session;
pub mod store;

pub use error::IdentityError;
pub use identity::{
    delivering_host, home_host_of, home_identity_of, is_issued_by_home, is_mine,
    normalize_origin_to_host, own_row_at, parse_federated_username, self_identity_of,
    user_display_name, user_key, user_key_of, HomeIdentity, IdentityFields, SelfIdentity,
};
pub use persist::PersistError;
pub use secret::AccessToken;
pub use session::InstanceSession;
pub use store::CredentialStore;

#[cfg(test)]
mod tests;
