//! Видео звонка: демонстрация экрана, камера, чужие потоки.
//!
//! Поверх [`bs_voice::VoiceSession`]; без флага `voice` — заглушка с тем же
//! интерфейсом, которая честно говорит, что видео в сборке нет.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::voice_media::{MediaEngine, VoicePrefs};

/// Кадр потока, готовый к загрузке в текстуру.
#[derive(Clone)]
pub(crate) struct FeedImage {
    /// Ширина и высота.
    pub(crate) size: [usize; 2],
    #[cfg(feature = "voice")]
    frame: Arc<bs_voice::RgbaFrame>,
}

impl FeedImage {
    /// Пиксели RGBA8.
    #[cfg_attr(
        not(feature = "voice"),
        allow(clippy::unused_self, reason = "без голоса кадров нет")
    )]
    pub(crate) fn rgba(&self) -> &[u8] {
        #[cfg(feature = "voice")]
        {
            &self.frame.rgba
        }
        #[cfg(not(feature = "voice"))]
        {
            &[]
        }
    }
}

#[allow(clippy::missing_fields_in_debug, reason = "пиксели в отладке не нужны")]
impl std::fmt::Debug for FeedImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeedImage")
            .field("size", &self.size)
            .finish()
    }
}

/// Один видеопоток.
#[derive(Debug, Clone)]
pub(crate) struct Feed {
    /// Identity в LiveKit (`userId:имя`).
    pub(crate) identity: String,
    /// Id пользователя.
    pub(crate) user_id: String,
    /// Демонстрация экрана (иначе камера).
    pub(crate) screen: bool,
    /// Наш собственный поток.
    pub(crate) local: bool,
    /// Последний кадр.
    pub(crate) image: Option<FeedImage>,
    /// Номер кадра: изменился — пора обновить текстуру.
    pub(crate) generation: u64,
}

/// Что можно показать: монитор или окно.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShareSource {
    /// Идентификатор источника.
    pub(crate) id: u32,
    /// Имя для списка.
    pub(crate) name: String,
    /// Окно (иначе монитор).
    pub(crate) window: bool,
    /// Ширина.
    pub(crate) width: u32,
    /// Высота.
    pub(crate) height: u32,
}

/// Камера в системе.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CameraDevice {
    /// Номер.
    pub(crate) index: u32,
    /// Имя.
    pub(crate) name: String,
}

/// Состояние своего видео.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct VideoStatus {
    /// Идёт демонстрация экрана.
    pub(crate) screen_on: bool,
    /// Включена камера.
    pub(crate) camera_on: bool,
    /// Операция ещё выполняется.
    pub(crate) busy: bool,
    /// Последняя ошибка.
    pub(crate) error: Option<String>,
}

/// Высота и частота кадров демонстрации (как `screenShareConfig` оригинала).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ShareQuality {
    /// Высота кадра: 720, 1080 или 1440.
    pub(crate) height: u32,
    /// Кадров в секунду: 15, 30 или 60.
    pub(crate) fps: u32,
}

impl Default for ShareQuality {
    fn default() -> Self {
        Self {
            height: 720,
            fps: 30,
        }
    }
}

impl ShareQuality {
    /// Допустимые высоты.
    pub(crate) const HEIGHTS: [u32; 3] = [720, 1080, 1440];
    /// Допустимые частоты.
    pub(crate) const RATES: [u32; 3] = [15, 30, 60];

    /// Приводит к допустимым значениям.
    pub(crate) fn sanitize(&mut self) {
        if !Self::HEIGHTS.contains(&self.height) {
            self.height = 720;
        }
        if !Self::RATES.contains(&self.fps) {
            self.fps = 30;
        }
    }

    #[cfg_attr(
        not(feature = "voice"),
        allow(dead_code, reason = "нужно только с LiveKit")
    )]
    /// Ширина кадра для источника с заданными размерами (не шире источника).
    #[must_use]
    pub(crate) fn max_width(self, width: u32, height: u32) -> u32 {
        if height == 0 || height <= self.height {
            return width;
        }
        let scaled = u64::from(width) * u64::from(self.height) / u64::from(height);
        u32::try_from(scaled).unwrap_or(width)
    }

    #[cfg_attr(
        not(feature = "voice"),
        allow(dead_code, reason = "нужно только с LiveKit")
    )]
    /// Битрейт, бит/с: растёт с площадью кадра и частотой.
    #[must_use]
    pub(crate) fn bitrate(self) -> u64 {
        let base: u64 = match self.height {
            0..=720 => 2_500_000,
            721..=1080 => 4_500_000,
            _ => 7_000_000,
        };
        if self.fps >= 60 { base * 3 / 2 } else { base }
    }
}

fn lock(status: &Mutex<VideoStatus>) -> MutexGuard<'_, VideoStatus> {
    status.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Управление своим видео и доступ к чужому.
#[derive(Debug, Default)]
pub(crate) struct VideoControl {
    status: Arc<Mutex<VideoStatus>>,
}

impl VideoControl {
    /// Текущее состояние своего видео.
    pub(crate) fn status(&self) -> VideoStatus {
        lock(&self.status).clone()
    }

    /// Звонок закончился: всё выключено.
    pub(crate) fn reset(&self) {
        *lock(&self.status) = VideoStatus::default();
    }

    /// Забыть последнюю ошибку.
    pub(crate) fn clear_error(&self) {
        lock(&self.status).error = None;
    }
}

#[cfg(feature = "voice")]
mod real {
    use std::future::Future;

    use bs_voice::VideoPublishConfig;
    use bs_voice::camera::{CameraConfig, list_cameras};
    use bs_voice::capture::{
        CaptureConfig, CaptureTarget, ScreenSource, list_monitors, list_windows,
    };

    use super::{
        CameraDevice, Feed, FeedImage, MediaEngine, ShareQuality, ShareSource, VideoControl,
        VoicePrefs, lock,
    };

    impl VideoControl {
        /// Видеопотоки звонка.
        pub(crate) fn feeds(media: &MediaEngine) -> Vec<Feed> {
            let Some(session) = media.session() else {
                return Vec::new();
            };
            session
                .video_feeds()
                .into_iter()
                .map(|feed| Feed {
                    screen: feed.kind == bs_voice::TrackKind::ScreenShare,
                    image: feed.frame.map(|frame| FeedImage {
                        size: [frame.width as usize, frame.height as usize],
                        frame,
                    }),
                    identity: feed.identity,
                    user_id: feed.user_id,
                    local: feed.local,
                    generation: feed.generation,
                })
                .collect()
        }

        /// Кто смотрит нашу трансляцию (id пользователей).
        pub(crate) fn watchers(media: &MediaEngine) -> Vec<String> {
            media.session().map_or_else(Vec::new, |session| {
                session
                    .my_watchers()
                    .iter()
                    .map(|identity| bs_voice::user_id_of(identity).to_owned())
                    .collect()
            })
        }

        /// Мониторы и окна, которые можно показать.
        pub(crate) fn screen_sources() -> Result<Vec<ShareSource>, String> {
            let mut sources = list_monitors().map_err(|err| err.to_string())?;
            sources.extend(list_windows().unwrap_or_default());
            Ok(sources
                .into_iter()
                .map(|source| ShareSource {
                    id: source.id,
                    window: source.target == CaptureTarget::Window,
                    name: source.name,
                    width: source.width,
                    height: source.height,
                })
                .collect())
        }

        /// Камеры в системе.
        pub(crate) fn cameras() -> Result<Vec<CameraDevice>, String> {
            list_cameras()
                .map(|cameras| {
                    cameras
                        .into_iter()
                        .map(|camera| CameraDevice {
                            index: camera.index,
                            name: camera.name,
                        })
                        .collect()
                })
                .map_err(|err| err.to_string())
        }

        /// Выполняет операцию со звонком в фоне, отмечая занятость и ошибки.
        fn run<F, Fut>(&self, media: &MediaEngine, apply: fn(&mut super::VideoStatus, bool), op: F)
        where
            F: FnOnce(bs_voice::VoiceSession) -> Fut + Send + 'static,
            Fut: Future<Output = Result<bool, bs_voice::VoiceError>> + Send,
        {
            let Some(session) = media.session() else {
                lock(&self.status).error = Some(crate::i18n::tr("Нет звонка").to_owned());
                return;
            };
            lock(&self.status).busy = true;
            let status = std::sync::Arc::clone(&self.status);
            media.runtime().spawn(async move {
                let result = op(session).await;
                let mut status = lock(&status);
                status.busy = false;
                match result {
                    Ok(on) => {
                        apply(&mut status, on);
                        status.error = None;
                    }
                    Err(err) => status.error = Some(err.to_string()),
                }
            });
        }

        /// Начинает демонстрацию экрана.
        pub(crate) fn start_screen(
            &self,
            media: &MediaEngine,
            source: &ShareSource,
            quality: ShareQuality,
        ) {
            let source = ScreenSource {
                id: source.id,
                name: source.name.clone(),
                target: if source.window {
                    CaptureTarget::Window
                } else {
                    CaptureTarget::Monitor
                },
                width: source.width,
                height: source.height,
            };
            let capture = CaptureConfig {
                fps: quality.fps,
                max_width: quality.max_width(source.width, source.height).max(2),
                downscale: true,
            };
            let publish = VideoPublishConfig {
                max_bitrate: quality.bitrate(),
                max_framerate: f64::from(quality.fps),
                ..VideoPublishConfig::screen_share()
            };
            self.run(
                media,
                |s, on| s.screen_on = on,
                move |session| async move {
                    session.start_screen_share(source, capture, publish).await?;
                    Ok(true)
                },
            );
        }

        /// Останавливает демонстрацию экрана.
        pub(crate) fn stop_screen(&self, media: &MediaEngine) {
            self.run(
                media,
                |s, on| s.screen_on = on,
                |session| async move {
                    session.stop_screen_share().await?;
                    Ok(false)
                },
            );
        }

        /// Включает камеру.
        pub(crate) fn start_camera(&self, media: &MediaEngine, prefs: &VoicePrefs) {
            let index = prefs.camera.unwrap_or(0);
            self.run(
                media,
                |s, on| s.camera_on = on,
                move |session| async move {
                    session
                        .start_camera(index, CameraConfig::default(), VideoPublishConfig::camera())
                        .await?;
                    Ok(true)
                },
            );
        }

        /// Выключает камеру.
        pub(crate) fn stop_camera(&self, media: &MediaEngine) {
            self.run(
                media,
                |s, on| s.camera_on = on,
                |session| async move {
                    session.stop_camera().await?;
                    Ok(false)
                },
            );
        }

        /// Сообщает ведущему, что мы смотрим (или нет) его трансляцию.
        pub(crate) fn set_watching(media: &MediaEngine, identity: &str, watching: bool) {
            let Some(session) = media.session() else {
                return;
            };
            let identity = identity.to_owned();
            media.runtime().spawn(async move {
                session.set_watching(&identity, watching).await;
            });
        }
    }
}

#[cfg(not(feature = "voice"))]
mod stub {
    use super::{
        CameraDevice, Feed, MediaEngine, ShareQuality, ShareSource, VideoControl, VoicePrefs,
    };
    use crate::i18n::tr;

    #[allow(
        clippy::unused_self,
        reason = "тот же интерфейс, что у настоящего движка"
    )]
    impl VideoControl {
        pub(crate) fn feeds(_media: &MediaEngine) -> Vec<Feed> {
            Vec::new()
        }

        pub(crate) fn watchers(_media: &MediaEngine) -> Vec<String> {
            Vec::new()
        }

        pub(crate) fn screen_sources() -> Result<Vec<ShareSource>, String> {
            Err(tr("сборка без голоса").to_owned())
        }

        pub(crate) fn cameras() -> Result<Vec<CameraDevice>, String> {
            Err(tr("сборка без голоса").to_owned())
        }

        pub(crate) fn start_screen(
            &self,
            _media: &MediaEngine,
            _source: &ShareSource,
            _quality: ShareQuality,
        ) {
        }

        pub(crate) fn stop_screen(&self, _media: &MediaEngine) {}

        pub(crate) fn start_camera(&self, _media: &MediaEngine, _prefs: &VoicePrefs) {}

        pub(crate) fn stop_camera(&self, _media: &MediaEngine) {}

        pub(crate) fn set_watching(_media: &MediaEngine, _identity: &str, _watching: bool) {}
    }
}

#[cfg(test)]
mod tests {
    use super::ShareQuality;

    #[test]
    fn quality_never_upscales_and_keeps_aspect() {
        let quality = ShareQuality {
            height: 720,
            fps: 30,
        };
        assert_eq!(quality.max_width(2560, 1440), 1280);
        assert_eq!(quality.max_width(1280, 600), 1280);
    }

    #[test]
    fn quality_is_sanitized_to_known_steps() {
        let mut quality = ShareQuality {
            height: 999,
            fps: 7,
        };
        quality.sanitize();
        assert_eq!(quality, ShareQuality::default());
    }

    #[test]
    fn bitrate_grows_with_size_and_rate() {
        let base = ShareQuality::default().bitrate();
        let full = ShareQuality {
            height: 1080,
            fps: 60,
        }
        .bitrate();
        assert!(full > base);
    }
}
