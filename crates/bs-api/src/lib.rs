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

pub mod admin;
pub mod admin_more;
pub mod assets;
pub mod auth;
pub mod client;
pub mod community;
pub mod compose;
pub mod error;
pub mod federation;
pub mod instance;
pub mod peers;
pub mod profile;
pub mod read;
pub mod registry;
pub mod write;

pub use admin::{
    AdminUser, AdminUserPage, CleanupResult, InstanceSettings, InstanceSettingsPatch,
    PasswordReset, StorageBreakdown, StorageStats,
};
pub use admin_more::{
    CurrentVersion, LatestRelease, StreamingLimits, StreamingPatch, TelemetryStatus, UpdateStatus,
};
pub use assets::{AssetPath, GIF_HOSTS, STICKER_PREFIX, fetch_external, resolve_asset};
pub use auth::AuthResponse;
pub use bs_types::{LiveKitTokenRequest, LiveKitTokenResponse};
pub use client::{ApiClient, DEFAULT_TIMEOUT, MAX_RESPONSE_BYTES, USER_AGENT};
pub use community::{CreateSpaceRequest, ExplorePage, UpdateProfileRequest};
pub use compose::{GifItem, GifPage, MAX_UPLOAD_CHUNK, PersonalSticker, SendMessageBody};
pub use error::ApiError;
pub use federation::{
    AttachProofResponse, ChangePasswordRequest, ChangePasswordResponse,
    FederationCredentialRequest, FederationCredentialResponse, ReattachRequest, ReattachResponse,
};
pub use instance::{
    InstanceInfo, PeerEnsureResponse, RegisterRequest, ReplicatedEntry, VerifyPasswordResponse,
};
pub use peers::{ApprovalRequest, Peer, Recheck};
pub use profile::{MutualSpace, Mutuals};
pub use read::{
    DmMessagePage, MESSAGES_LIMIT_DEFAULT, MESSAGES_LIMIT_MAX, MESSAGES_LIMIT_MIN, MessagePage,
    PagingCursor,
};
pub use registry::{
    IdentityDeleteMode, IdentityDeleteResult, RegistryEntry, RegistryResponse, RegistryStatus,
};
pub use write::{
    CreateDmRequest, FriendRequestDecision, MessageSearchQuery, SEARCH_LIMIT_DEFAULT,
    SEARCH_LIMIT_MAX, SEARCH_LIMIT_MIN, SendFriendRequest,
};
