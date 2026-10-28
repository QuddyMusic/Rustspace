//! Видео: камера и демонстрация экрана.
//!
//! Публикация идёт через `NativeVideoSource`: мы сами скармливаем ему
//! кадры I420 (`capture_frame`), а libwebrtc кодирует и отправляет их.
//! Для приёма удалённого видео используем `NativeVideoStream` и отдаём
//! наружу RGBA — его напрямую принимает egui как текстуру.

use std::sync::Arc;

use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding};
use livekit::prelude::*;
use livekit::webrtc::prelude::*;
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, info};

use crate::capture::{CaptureConfig, ScreenCapture, ScreenSource};
use crate::convert::{RgbaFrame, buffer_to_rgba, frame_from_i420, rgba_to_i420};
use crate::error::{VoiceError, VoiceResult};
use crate::state::TrackKind;

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
}

impl Default for VideoPublishConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodecChoice::H264,
            max_bitrate: 3_000_000,
            max_framerate: 30.0,
            simulcast: false,
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
        if !(1.0..=60.0).contains(&self.max_framerate) {
            return Err(VoiceError::Config(format!(
                "fps должен быть в 1..=60, получено {}",
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
    size: (u32, u32),
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
    /// Создаёт источник и локальный трек нужного размера.
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
                size: (width, height),
            }),
        })
    }

    /// Размер кадра, под который создан источник.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        self.inner.size
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

    /// Отправляет кадр RGBA в источник.
    ///
    /// Если размер кадра не совпадает с источником, он масштабируется
    /// силами libwebrtc (быстрый билинейный ресайз в I420).
    ///
    /// # Errors
    ///
    /// Неверный размер буфера или сбой масштабирования.
    pub fn push_rgba(&self, rgba: &[u8], width: u32, height: u32) -> VoiceResult<()> {
        let mut buffer = rgba_to_i420(rgba, width, height)?;
        if (width, height) != self.inner.size {
            let (tw, th) = self.inner.size;
            buffer = buffer.scale(tw as i32, th as i32);
        }
        let frame = frame_from_i420(buffer);
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

/// Активная демонстрация экрана: захват в своём потоке + публикация.
#[derive(Debug)]
pub struct ScreenShareSession {
    capture: ScreenCapture,
    publisher: VideoPublisher,
    task: Option<tokio::task::JoinHandle<()>>,
    source: ScreenSource,
}

impl ScreenShareSession {
    /// Запускает захват, публикует дорожку и начинает пересылку кадров.
    ///
    /// # Errors
    ///
    /// Источник недоступен, неверные настройки захвата или ошибка публикации.
    pub async fn start(
        room: &Room,
        source: ScreenSource,
        capture_config: CaptureConfig,
        publish_config: VideoPublishConfig,
    ) -> VoiceResult<Self> {
        capture_config.validate()?;
        publish_config.validate()?;

        let (width, height) = capture_config.output_size(source.width, source.height);
        let (width, height) = (width & !1, height & !1);

        let (capture, mut frames) = ScreenCapture::start(source.clone(), capture_config)?;
        let publisher = VideoPublisher::new("screen", width, height, true)?;
        publisher
            .publish(room, &publish_config, TrackSource::Screenshare)
            .await?;

        let push_target = publisher.clone();
        let task = tokio::spawn(async move {
            while let Some(frame) = frames.recv().await {
                if let Err(e) = push_target.push_rgba(&frame.rgba, frame.width, frame.height) {
                    debug!(error = %e, "кадр пропущен");
                }
            }
            debug!("поток кадров демонстрации завершён");
        });

        Ok(Self {
            capture,
            publisher,
            task: Some(task),
            source,
        })
    }

    /// Источник, который показываем.
    #[must_use]
    pub const fn source(&self) -> &ScreenSource {
        &self.source
    }

    /// Останавливает захват и снимает публикацию.
    ///
    /// # Errors
    ///
    /// Ошибка LiveKit при отмене публикации.
    pub async fn stop(&mut self, room: &Room) -> VoiceResult<()> {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.capture.stop();
        self.publisher.unpublish(room).await
    }
}

// ── приём удалённого видео ───────────────────────────────────────────────

/// Один кадр удалённого видео, готовый к показу.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedVideoFrame {
    /// Кто прислал (identity участника).
    pub identity: String,
    /// SID дорожки.
    pub sid: String,
    /// Что это: камера или демонстрация экрана.
    pub kind: TrackKind,
    /// Кадр RGBA8.
    pub frame: RgbaFrame,
}

/// Приёмник одного удалённого видеотрека.
///
/// Читает [`NativeVideoStream`] в отдельной задаче и складывает
/// последний кадр в общий слот: рендер забирает его без блокировки.
#[derive(Debug)]
struct ReceiverTask(JoinHandle<()>);

impl Drop for ReceiverTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Debug, Clone)]
pub struct RemoteVideoReceiver {
    latest: Arc<RwLock<Option<RgbaFrame>>>,
    // Задача живёт, пока существует хотя бы одна копия приёмника.
    _task: Arc<ReceiverTask>,
    identity: String,
    sid: String,
    kind: TrackKind,
}

impl RemoteVideoReceiver {
    /// Запускает чтение трека в фоне.
    ///
    /// Кадры декодируются в RGBA прямо в задаче чтения, поэтому
    /// вызывающему достаётся уже готовая картинка.
    #[must_use]
    pub fn spawn(
        identity: impl Into<String>,
        sid: impl Into<String>,
        kind: TrackKind,
        track: &RemoteVideoTrack,
    ) -> Self {
        let latest = Arc::new(RwLock::new(None));
        let sink = Arc::clone(&latest);
        let mut stream = NativeVideoStream::new(track.rtc_track());

        let task = tokio::spawn(async move {
            use tokio_stream::StreamExt as _;

            while let Some(frame) = stream.next().await {
                match buffer_to_rgba(frame.buffer.as_ref()) {
                    Ok(rgba) => *sink.write().await = Some(rgba),
                    Err(e) => debug!(error = %e, "кадр видео пропущен"),
                }
            }
            debug!("поток удалённого видео завершён");
        });

        Self {
            latest,
            _task: Arc::new(ReceiverTask(task)),
            identity: identity.into(),
            sid: sid.into(),
            kind,
        }
    }

    /// Кто прислал.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// SID дорожки.
    #[must_use]
    pub fn sid(&self) -> &str {
        &self.sid
    }

    /// Камера или демонстрация экрана.
    #[must_use]
    pub const fn kind(&self) -> TrackKind {
        self.kind
    }

    /// Забирает последний кадр, если с прошлого раза появился новый.
    pub async fn take_latest(&self) -> Option<ReceivedVideoFrame> {
        let frame = self.latest.write().await.take()?;
        Some(ReceivedVideoFrame {
            identity: self.identity.clone(),
            sid: self.sid.clone(),
            kind: self.kind,
            frame,
        })
    }

    /// Смотрит последний кадр, не забирая его.
    pub async fn peek(&self) -> Option<RgbaFrame> {
        self.latest.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::pending;

    #[tokio::test]
    async fn receiver_task_is_aborted_when_last_guard_drops() {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _tx = tx;
            pending::<()>().await;
        });
        let guard = Arc::new(ReceiverTask(task));
        let clone = Arc::clone(&guard);
        drop(guard);
        assert!(
            rx.is_empty(),
            "задача не должна завершиться при наличии копии"
        );
        drop(clone);
        assert!(rx.await.is_err(), "последняя копия должна отменить задачу");
    }

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
        for fps in [0.0, 0.5, 61.0, 120.0] {
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
