//! REST-клиент Backspace (`/api/*`).
//!
//! Крейт предоставляет [`ApiClient`] — тонкую обёртку над `reqwest` для
//! одного инстанса Backspace, и типизированные методы эндпоинтов.
//!
//! # Пример
//! ```no_run
//! # async fn run() -> Result<(), bs_api::ApiError> {
//! use bs_api::ApiClient;
//!
//! let mut client = ApiClient::home("https://chat.example.com")?;
//! let auth = client.login_and_attach("alice", "secret").await?;
//! let me = client.me().await?;
//! # let _ = (auth, me);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod auth;
pub mod client;
pub mod error;
pub mod federation;
pub mod read;
pub mod write;

pub use auth::AuthResponse;
pub use bs_types::{LiveKitTokenRequest, LiveKitTokenResponse};
pub use client::{ApiClient, DEFAULT_TIMEOUT, USER_AGENT};
pub use error::ApiError;
pub use federation::{
    AttachProofResponse, ChangePasswordRequest, ChangePasswordResponse,
    FederationCredentialRequest, FederationCredentialResponse, ReattachRequest, ReattachResponse,
};
pub use read::{
    MESSAGES_LIMIT_DEFAULT, MESSAGES_LIMIT_MAX, MESSAGES_LIMIT_MIN, MessagePage, PagingCursor,
};
pub use write::{
    CreateDmRequest, FriendRequestDecision, MessageSearchQuery, SEARCH_LIMIT_DEFAULT,
    SEARCH_LIMIT_MAX, SEARCH_LIMIT_MIN, SendFriendRequest,
};
