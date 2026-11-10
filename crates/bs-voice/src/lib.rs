//! Голос и видео через LiveKit.
//!
//! Крейт делится на два слоя:
//!
//! - **доменный** ([`state`], [`events`], [`volume`], [`vad`], [`device`], [`mixer`]) —
//!   чистые типы без зависимости от LiveKit, покрыты юнит-тестами;
//! - **транспортный** ([`session`], [`audio`]) — тонкая обёртка над `livekit::Room`,
//!   `PlatformAudio` и `libwebrtc`.
//!
//! Громкость участников реализована на доменном уровне: libwebrtc не даёт
//! API громкости на трек, поэтому 0 % — это отключение подписки, а
//! промежуточные значения — собственный миксдаун ([`mixer`]).

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod audio;
pub mod capture;
pub mod config;
pub mod convert;
pub mod device;
pub mod error;
pub mod events;
pub mod mixer;
mod monitor;
pub mod protocol;
pub mod session;
pub mod state;
pub mod vad;
pub mod video;
pub mod volume;

pub use audio::{AudioEngine, HardwareCapabilities};
pub use config::{AudioProcessingSettings, VoiceConfig};
pub use convert::RgbaFrame;
pub use device::{AudioDevice, DeviceKind, DeviceSelection};
pub use error::{VoiceError, VoiceResult};
pub use events::VoiceEvent;
pub use mixer::Mixer;
pub use monitor::MicMonitor;
pub use protocol::{
    CallIdentity, CallPhase, CallRecord, INCOMING_CALL_TIMEOUT, ProtocolAction, REBIND_GRACE,
    VoiceCoordinator,
};
pub use session::VoiceSession;
pub use state::{ParticipantInfo, RoomSnapshot, RoomState, TrackKind};
pub use vad::VoiceDetector;
pub use video::{
    ReceivedVideoFrame, RemoteVideoReceiver, ScreenShareSession, VideoCodecChoice,
    VideoPublishConfig, VideoPublisher,
};
pub use volume::{Volume, VolumeMap};

/// Версия голосового слоя (для диагностики).
pub const STAGE: &str = "4.3";
