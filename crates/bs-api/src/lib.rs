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
pub mod bots;
pub mod bridges;
pub mod client;
pub mod community;
pub mod compose;
pub mod error;
pub mod federation;
pub mod groups;
pub mod instance;
pub mod invites;
pub mod notifications;
pub mod peers;
pub mod personal_bridges;
pub mod profile;
pub mod read;
pub mod registry;
pub mod saved;
pub mod space_manage;
pub mod sticker_packs;
pub mod twofactor;
pub mod webhooks;
pub mod write;

pub use admin::{
    AdminUser, AdminUserPage, CleanupResult, InstanceSettings, InstanceSettingsPatch,
    PasswordReset, StorageBreakdown, StorageStats,
};
pub use admin_more::{
    BackupItem, Backups, CurrentVersion, DatabaseInfo, DiskInfo, HelperJob, LatestRelease,
    SelfUpdate, ServerHealth, ServerState, StreamingLimits, StreamingPatch, TelemetryStatus,
    UpdateStatus,
};
pub use assets::{
    AssetPath, GIF_HOSTS, PREVIEW_PREFIX, STICKER_PREFIX, fetch_external, resolve_asset,
};
pub use auth::AuthResponse;
pub use bots::{Bot, BotPatch, BotSpace, BotWithToken};
pub use bridges::{BridgeLink, ChannelBridges};
pub use bs_types::{LiveKitTokenRequest, LiveKitTokenResponse};
pub use client::{ApiClient, DEFAULT_TIMEOUT, MAX_RESPONSE_BYTES, USER_AGENT};
pub use community::{
    CreateSpaceRequest, DirectoryEntry, DirectoryFeed, ExplorePage, UpdateProfileRequest,
};
pub use compose::{GifItem, GifPage, MAX_UPLOAD_CHUNK, PersonalSticker, SendMessageBody};
pub use error::ApiError;
pub use federation::{
    AttachProofResponse, ChangePasswordRequest, ChangePasswordResponse,
    FederationCredentialRequest, FederationCredentialResponse, ReattachRequest, ReattachResponse,
};
pub use groups::{
    GROUP_MAX_MEMBERS, GROUP_NAME_MAX, GroupMember, GroupUpdate, InviteTarget, SpaceInviteRequest,
    SpaceInviteResponse,
};
pub use instance::{
    InstanceInfo, InviteCheck, PeerEnsureResponse, RegisterRequest, ReplicatedEntry, UsernameCheck,
    VerifyPasswordResponse,
};
pub use invites::{Invite, InvitePatch, NewInvite, Redemption};
pub use peers::{ApprovalRequest, Peer, Recheck};
pub use personal_bridges::{
    MatrixLogin, PersonalAccount, PersonalBridges, PersonalChat, RemoteChat, TelegramStep,
};
pub use profile::{MutualSpace, Mutuals};
pub use read::{
    DmMessagePage, MESSAGES_LIMIT_DEFAULT, MESSAGES_LIMIT_MAX, MESSAGES_LIMIT_MIN, MessagePage,
    PagingCursor,
};
pub use registry::{
    IdentityDeleteMode, IdentityDeleteResult, RegistryEntry, RegistryResponse, RegistryStatus,
};
pub use saved::{Bookmark, BookmarkAttachment, BookmarkPage, BookmarkStatus, ChatKind, Pin};
pub use space_manage::{
    Ban, ChannelUpdate, JoinRequest, NewChannel, NewRole, OverrideScope, PermissionOverride,
    RoleSide, RoleUpdate, SpaceUpdate, invite_url, parse_invite_input,
};
pub use sticker_packs::{StickerPack, StickerPacks};
pub use twofactor::{E2eeKey, TwoFactorSetup, TwoFactorStatus};
pub use webhooks::{
    CARD_MARK, CardDraft, CardImage, FORWARD_MARK, InstanceVoiceBitrate, VoiceBitrate, Webhook,
    card_as_markdown, forward_as_markdown, split_card_mark,
};
pub use write::{
    CreateDmRequest, FriendRequestDecision, MessageSearchQuery, SEARCH_LIMIT_DEFAULT,
    SEARCH_LIMIT_MAX, SEARCH_LIMIT_MIN, SendFriendRequest,
};
