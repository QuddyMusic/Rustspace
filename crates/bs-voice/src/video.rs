//! Видео: камера и демонстрация экрана.
//!
//! Публикация идёт через `NativeVideoSource`: мы сами скармливаем ему
//! кадры I420 (`capture_frame`), а libwebrtc кодирует и отправляет их.
//! Приём удалённого видео — в [`crate::stage`].

use std::sync::Arc;

use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding};
use livekit::prelude::*;
use livekit::webrtc::prelude::*;
use livekit::webrtc::video_frame::I420Buffer;
use livekit::webrtc::video_source::native::NativeVideoSource;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

use crate::convert::{fit_i420, frame_from_i420};
use crate::error::{VoiceError, VoiceResult};

/// Кодек для публикации видео.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VideoCodecChoice {
    /// VP8 — дефолт LiveKit, аппаратного ускорения нет, но совместим везде.
    Vp8,
    /// H264 — есть аппаратное кодирование на всех трёх платформах.
    H264,
    /// VP9 — лучше качество при том же битрейте, дороже по CPU.
    Vp9,
    /// AV1 — лучший битрейт, но тяжёлый и не везде поддержан.
    Av1,
}

impl VideoCodecChoice {
    /// Преобразует в кодек LiveKit.
    #[must_use]
    pub const fn to_livekit(self) -> VideoCodec {
        match self {
            Self::Vp8 => VideoCodec::VP8,
            Self::H264 => VideoCodec::H264,
            Self::Vp9 => VideoCodec::VP9,
            Self::Av1 => VideoCodec::AV1,
        }
    }

    /// Поддерживает ли кодек simulcast-слои в libwebrtc.
    #[must_use]
    pub const fn supports_simulcast(self) -> bool {
        matches!(self, Self::Vp8 | Self::H264)
    }
}

/// Настройки публикации видео.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VideoPublishConfig {
    /// Кодек.
    pub codec: VideoCodecChoice,
    /// Целевой битрейт, бит/с.
    pub max_bitrate: u64,
    /// Максимальный fps.
    pub max_framerate: f64,
    /// Включать simulcast (только для VP8/H264).
    pub simulcast: bool,
    /// Что беречь, когда канала не хватает; `None` — выбор LiveKit по источнику.
    pub content: Option<ContentHint>,
}

/// Чем жертвовать при нехватке канала (`contentHint` оригинала).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentHint {
    /// Игры и видео: держим частоту кадров, разрешение может падать.
    Motion,
    /// Текст и код: держим разрешение, кадры могут пропускаться.
    Detail,
}

impl ContentHint {
    const fn degradation(self) -> livekit::options::DegradationPreference {
        match self {
            Self::Motion => livekit::options::DegradationPreference::MaintainFramerate,
            Self::Detail => livekit::options::DegradationPreference::MaintainResolution,
        }
    }
}

impl Default for VideoPublishConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodecChoice::H264,
            max_bitrate: 3_000_000,
            max_framerate: 30.0,
            simulcast: false,
            content: None,
        }
    }
}

impl VideoPublishConfig {
    /// Настройки для демонстрации экрана: текст важнее плавности.
    #[must_use]
    pub fn screen_share() -> Self {
        Self {
            codec: VideoCodecChoice::H264,
            max_bitrate: 4_000_000,
            max_framerate: 15.0,
            simulcast: false,
            content: None,
        }
    }

    /// Настройки для камеры.
    #[must_use]
    pub fn camera() -> Self {
        Self {
            codec: VideoCodecChoice::Vp8,
            max_bitrate: 1_500_000,
            max_framerate: 30.0,
            simulcast: true,
            content: None,
        }
    }

    /// Проверяет параметры.
    ///
    /// # Errors
    ///
    /// Нулевой битрейт или недопустимый fps.
    pub fn validate(&self) -> VoiceResult<()> {
        if self.max_bitrate == 0 {
            return Err(VoiceError::Config("битрейт не может быть 0".into()));
        }
        if !(1.0..=f64::from(crate::capture::MAX_FPS)).contains(&self.max_framerate) {
            return Err(VoiceError::Config(format!(
                "fps должен быть в 1..={}, получено {}",
                crate::capture::MAX_FPS,
                self.max_framerate
            )));
        }
        Ok(())
    }

    /// Собирает опции публикации для LiveKit.
    #[must_use]
    pub fn publish_options(&self, source: TrackSource) -> TrackPublishOptions {
        let simulcast = self.simulcast && self.codec.supports_simulcast();
        TrackPublishOptions {
            video_codec: self.codec.to_livekit(),
            video_encoding: Some(VideoEncoding {
                max_bitrate: self.max_bitrate,
                max_framerate: self.max_framerate,
            }),
            simulcast,
            source,
            degradation_preference: self.content.map(ContentHint::degradation),
            ..Default::default()
        }
    }
}

/// Внутреннее состояние публикатора.
#[derive(Debug)]
struct PublisherInner {
    source: NativeVideoSource,
    track: LocalVideoTrack,
    publication: RwLock<Option<LocalTrackPublication>>,
    /// Предел размера кадра; меньшие кадры уходят как есть.
    max: (u32, u32),
}

/// Публикуемая дорожка видео.
///
/// Клонируется дёшево: внутри `Arc`, поэтому ручку можно отдать
/// в задачу пересылки кадров.
#[derive(Debug, Clone)]
pub struct VideoPublisher {
    inner: Arc<PublisherInner>,
}

impl VideoPublisher {
    /// Создаёт источник и локальный трек. `width`×`height` — предел
    /// размера: кадры крупнее уменьшаются с сохранением пропорций.
    ///
    /// # Errors
    ///
    /// Нулевой размер кадра.
    pub fn new(name: &str, width: u32, height: u32, is_screencast: bool) -> VoiceResult<Self> {
        if width == 0 || height == 0 {
            return Err(VoiceError::InvalidFrameSize { width, height });
        }
        let resolution = VideoResolution { width, height };
        let source = NativeVideoSource::new(resolution, is_screencast);
        let track =
            LocalVideoTrack::create_video_track(name, RtcVideoSource::Native(source.clone()));
        Ok(Self {
            inner: Arc::new(PublisherInner {
                source,
                track,
                publication: RwLock::new(None),
                max: (width, height),
            }),
        })
    }

    /// Предел размера кадра.
    #[must_use]
    pub fn max_size(&self) -> (u32, u32) {
        self.inner.max
    }

    /// Публикует трек в комнате.
    ///
    /// # Errors
    ///
    /// Ошибка LiveKit при публикации.
    pub async fn publish(
        &self,
        room: &Room,
        config: &VideoPublishConfig,
        source: TrackSource,
    ) -> VoiceResult<()> {
        config.validate()?;
        let options = config.publish_options(source);
        let publication = room
            .local_participant()
            .publish_track(LocalTrack::Video(self.inner.track.clone()), options)
            .await?;
        info!(sid = %publication.sid(), "видео опубликовано");
        *self.inner.publication.write().await = Some(publication);
        Ok(())
    }

    /// Отправляет кадр I420 (крупный — уменьшив под предел).
    ///
    /// # Errors
    ///
    /// Источник отклонил кадр.
    pub fn push_i420(&self, buffer: I420Buffer) -> VoiceResult<()> {
        let (max_width, max_height) = self.inner.max;
        let frame = frame_from_i420(fit_i420(buffer, max_width, max_height));
        if !self.inner.source.capture_frame(&frame) {
            return Err(VoiceError::Video("источник отклонил кадр".into()));
        }
        Ok(())
    }

    /// Снимает публикацию с комнаты.
    ///
    /// # Errors
    ///
    /// Ошибка LiveKit при отмене публикации.
    pub async fn unpublish(&self, room: &Room) -> VoiceResult<()> {
        let publication = self.inner.publication.write().await.take();
        if let Some(publication) = publication {
            room.local_participant()
                .unpublish_track(&publication.sid())
                .await?;
            info!("видео снято с публикации");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        assert!(VideoPublishConfig::default().validate().is_ok());
    }

    #[test]
    fn zero_bitrate_is_rejected() {
        let cfg = VideoPublishConfig {
            max_bitrate: 0,
            ..VideoPublishConfig::default()
        };
        assert!(matches!(cfg.validate(), Err(VoiceError::Config(_))));
    }

    #[test]
    fn fps_out_of_range_is_rejected() {
        for fps in [0.0, 0.5, 121.0, 240.0] {
            let cfg = VideoPublishConfig {
                max_framerate: fps,
                ..VideoPublishConfig::default()
            };
            assert!(cfg.validate().is_err(), "fps {fps} должен быть отвергнут");
        }
    }

    #[test]
    fn screen_share_prefers_low_fps_high_bitrate() {
        let screen = VideoPublishConfig::screen_share();
        let camera = VideoPublishConfig::camera();
        assert!(screen.max_framerate < camera.max_framerate);
        assert!(screen.max_bitrate > camera.max_bitrate);
        assert!(!screen.simulcast);
    }

    #[test]
    fn simulcast_only_for_supported_codecs() {
        assert!(VideoCodecChoice::Vp8.supports_simulcast());
        assert!(VideoCodecChoice::H264.supports_simulcast());
        assert!(!VideoCodecChoice::Vp9.supports_simulcast());
        assert!(!VideoCodecChoice::Av1.supports_simulcast());
    }

    #[test]
    fn publish_options_drop_simulcast_for_vp9() {
        let cfg = VideoPublishConfig {
            codec: VideoCodecChoice::Vp9,
            simulcast: true,
            ..VideoPublishConfig::default()
        };
        let options = cfg.publish_options(TrackSource::Camera);
        assert!(!options.simulcast);
        assert_eq!(options.source, TrackSource::Camera);
    }

    #[test]
    fn publish_options_keep_simulcast_for_vp8() {
        let cfg = VideoPublishConfig {
            codec: VideoCodecChoice::Vp8,
            simulcast: true,
            ..VideoPublishConfig::default()
        };
        assert!(cfg.publish_options(TrackSource::Camera).simulcast);
    }

    #[test]
    fn config_roundtrips_through_json() {
        let cfg = VideoPublishConfig::screen_share();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: VideoPublishConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn codec_choice_serializes_camel_case() {
        let json = serde_json::to_string(&VideoCodecChoice::Vp8).unwrap();
        assert_eq!(json, "\"vp8\"");
    }

    #[test]
    fn zero_frame_size_is_rejected() {
        assert!(matches!(
            VideoPublisher::new("cam", 0, 720, false),
            Err(VoiceError::InvalidFrameSize { .. })
        ));
    }
}
