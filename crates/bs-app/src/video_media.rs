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

/// Вид источника демонстрации.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(feature = "voice"),
    allow(dead_code, reason = "источники есть только с LiveKit")
)]
pub(crate) enum ShareKind {
    /// Монитор.
    Screen,
    /// Окно.
    Window,
    /// Выбор в системном диалоге (Wayland, macOS).
    System,
}

/// Что можно показать.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShareSource {
    /// Идентификатор источника у захватчика.
    pub(crate) id: u64,
    /// Имя для списка.
    pub(crate) name: String,
    /// Монитор, окно или системный выбор.
    pub(crate) kind: ShareKind,
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

pub(crate) use crate::share_quality::ShareSettings;
#[cfg(feature = "voice")]
use crate::share_quality::{ShareCodec, ShareMode};

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
    use bs_voice::capture::{CaptureConfig, CaptureTarget, ScreenSource, list_sources};

    use super::{
        CameraDevice, Feed, FeedImage, MediaEngine, ShareCodec, ShareKind, ShareMode,
        ShareSettings, ShareSource, VideoControl, VoicePrefs, lock,
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

        /// Мониторы и окна, которые можно показать (под Wayland — системный выбор).
        pub(crate) fn screen_sources() -> Result<Vec<ShareSource>, String> {
            let sources = list_sources().map_err(|err| err.to_string())?;
            Ok(sources
                .into_iter()
                .map(|source| ShareSource {
                    id: source.id,
                    kind: match source.target {
                        CaptureTarget::Monitor => ShareKind::Screen,
                        CaptureTarget::Window => ShareKind::Window,
                        CaptureTarget::System => ShareKind::System,
                    },
                    name: source.name,
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
            Fut: Future<Output = Result<(bool, Option<String>), bs_voice::VoiceError>> + Send,
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
                    Ok((on, warning)) => {
                        apply(&mut status, on);
                        status.error = warning;
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
            settings: ShareSettings,
        ) {
            let source = ScreenSource {
                id: source.id,
                name: source.name.clone(),
                target: match source.kind {
                    ShareKind::Screen => CaptureTarget::Monitor,
                    ShareKind::Window => CaptureTarget::Window,
                    ShareKind::System => CaptureTarget::System,
                },
            };
            let (max_width, max_height) = settings.max_size;
            let audio = settings.audio;
            let capture = CaptureConfig {
                fps: settings.fps,
                max_width,
                max_height,
                cursor: true,
            };
            let publish = VideoPublishConfig {
                codec: match settings.codec {
                    ShareCodec::Vp9 => bs_voice::VideoCodecChoice::Vp9,
                    ShareCodec::H264 => bs_voice::VideoCodecChoice::H264,
                },
                max_bitrate: settings.bitrate,
                max_framerate: f64::from(settings.fps),
                simulcast: false,
                content: Some(match settings.mode {
                    ShareMode::Gaming => bs_voice::ContentHint::Motion,
                    ShareMode::Detail => bs_voice::ContentHint::Detail,
                }),
            };
            self.run(
                media,
                |s, on| s.screen_on = on,
                move |session| async move {
                    session.start_screen_share(source, capture, publish).await?;
                    // Без звука трансляция всё равно идёт: ошибку только показываем.
                    let warning = if audio {
                        session.start_screen_audio().await.err().map(|err| {
                            crate::tf!(
                                "Экран показывается, но системный звук недоступен: {err}",
                                err = err
                            )
                        })
                    } else {
                        None
                    };
                    Ok((true, warning))
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
                    Ok((false, None))
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
                    Ok((true, None))
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
                    Ok((false, None))
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
        CameraDevice, Feed, MediaEngine, ShareSettings, ShareSource, VideoControl, VoicePrefs,
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
            _settings: ShareSettings,
        ) {
        }

        pub(crate) fn stop_screen(&self, _media: &MediaEngine) {}

        pub(crate) fn start_camera(&self, _media: &MediaEngine, _prefs: &VoicePrefs) {}

        pub(crate) fn stop_camera(&self, _media: &MediaEngine) {}

        pub(crate) fn set_watching(_media: &MediaEngine, _identity: &str, _watching: bool) {}
    }
}
