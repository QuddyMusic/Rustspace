//! Захват экрана через xcap.
//!
//! Объекты xcap (`Monitor`, `Window`) не `Send`: на X11 они держат
//! соединение с сервером, на Wayland — дескриптор PipeWire. Поэтому захват
//! живёт в собственном потоке: он сам создаёт захватчик, снимает кадры и
//! отправляет в канал обычные байты. Наружу торчит только `Send + Sync`
//! ручка [`ScreenCapture`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::error::{VoiceError, VoiceResult};

/// Что именно показываем.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureTarget {
    /// Весь монитор.
    Monitor,
    /// Одно окно.
    Window,
}

/// Описание источника захвата.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenSource {
    /// Идентификатор монитора или окна (xcap id).
    pub id: u32,
    /// Человекочитаемое имя для UI.
    pub name: String,
    /// Монитор или окно.
    pub target: CaptureTarget,
    /// Ширина источника в пикселях.
    pub width: u32,
    /// Высота источника в пикселях.
    pub height: u32,
}

/// Готовый кадр в RGBA8.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Параметры захвата.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureConfig {
    /// Кадров в секунду.
    pub fps: u32,
    /// Максимальная ширина; больше — уменьшаем пропорционально.
    pub max_width: u32,
    /// Уменьшать ли кадр под `max_width`.
    pub downscale: bool,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            fps: 15,
            max_width: 1920,
            downscale: true,
        }
    }
}

impl CaptureConfig {
    /// Проверяет разумность настроек.
    ///
    /// # Errors
    ///
    /// Если fps или ширина равны нулю.
    pub fn validate(&self) -> VoiceResult<()> {
        if self.fps == 0 || self.fps > 60 {
            return Err(VoiceError::Config(format!(
                "fps должен быть в 1..=60, получено {}",
                self.fps
            )));
        }
        if self.max_width < 2 {
            return Err(VoiceError::Config(
                "max_width должен быть не меньше 2 для I420".into(),
            ));
        }
        Ok(())
    }

    /// Итоговый размер кадра после возможного уменьшения.
    #[must_use]
    pub fn output_size(&self, width: u32, height: u32) -> (u32, u32) {
        if width == 0 || height == 0 {
            return (0, 0);
        }
        if !self.downscale || width <= self.max_width {
            return (width.max(2) & !1, height.max(2) & !1);
        }
        let scale = f64::from(self.max_width) / f64::from(width);
        let out_h = (f64::from(height) * scale).round() as u32;
        (self.max_width.max(2) & !1, out_h.max(2) & !1)
    }
}

/// Перечисляет доступные мониторы.
///
/// # Errors
///
/// Ошибка xcap, если сервер не отвечает или нет прав на захват.
pub fn list_monitors() -> VoiceResult<Vec<ScreenSource>> {
    let monitors = xcap::Monitor::all().map_err(|e| VoiceError::Capture(e.to_string()))?;
    Ok(monitors
        .into_iter()
        .filter_map(|m| {
            let id = m.id().ok()?;
            let width = m.width().ok()?;
            let height = m.height().ok()?;
            let name = m
                .friendly_name()
                .or_else(|_| m.name())
                .unwrap_or_else(|_| format!("Монитор {id}"));
            Some(ScreenSource {
                id,
                name,
                target: CaptureTarget::Monitor,
                width,
                height,
            })
        })
        .collect())
}

/// Перечисляет окна, которые имеет смысл показывать.
///
/// # Errors
///
/// Ошибка xcap при получении списка окон.
pub fn list_windows() -> VoiceResult<Vec<ScreenSource>> {
    let windows = xcap::Window::all().map_err(|e| VoiceError::Capture(e.to_string()))?;
    Ok(windows
        .into_iter()
        .filter(|w| !w.is_minimized().unwrap_or(true))
        .filter_map(|w| {
            let id = w.id().ok()?;
            let width = w.width().ok()?;
            let height = w.height().ok()?;
            if width == 0 || height == 0 {
                return None;
            }
            let title = w.title().unwrap_or_default();
            let app = w.app_name().unwrap_or_default();
            let name = if title.is_empty() {
                app
            } else {
                format!("{app} — {title}")
            };
            Some(ScreenSource {
                id,
                name,
                target: CaptureTarget::Window,
                width,
                height,
            })
        })
        .collect())
}

enum CaptureHandle {
    Monitor(xcap::Monitor),
    Window(xcap::Window),
}

impl CaptureHandle {
    fn capture_frame(&self) -> VoiceResult<CapturedFrame> {
        let image = match self {
            Self::Monitor(monitor) => monitor.capture_image(),
            Self::Window(window) => window.capture_image(),
        }
        .map_err(|e| VoiceError::Capture(e.to_string()))?;
        Ok(CapturedFrame {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
        })
    }
}

/// Ищет источник по id в текущем списке мониторов или окон.
fn find_source(source: &ScreenSource) -> VoiceResult<CaptureHandle> {
    let missing = || VoiceError::Capture(format!("источник {} не найден", source.id));
    match source.target {
        CaptureTarget::Monitor => {
            let monitors = xcap::Monitor::all().map_err(|e| VoiceError::Capture(e.to_string()))?;
            monitors
                .into_iter()
                .find(|m| m.id().is_ok_and(|id| id == source.id))
                .map(CaptureHandle::Monitor)
                .ok_or_else(missing)
        }
        CaptureTarget::Window => {
            let windows = xcap::Window::all().map_err(|e| VoiceError::Capture(e.to_string()))?;
            windows
                .into_iter()
                .find(|w| w.id().is_ok_and(|id| id == source.id))
                .map(CaptureHandle::Window)
                .ok_or_else(missing)
        }
    }
}

/// Захватывает один кадр монитора или окна по описанию источника.
///
/// # Errors
///
/// Ошибка xcap или неверные параметры конфигурации.
pub fn capture_once(source: &ScreenSource) -> VoiceResult<CapturedFrame> {
    find_source(source)?.capture_frame()
}

/// Ручка активного захвата экрана.
///
/// Поток захвата останавливается при вызове [`ScreenCapture::stop`] или
/// при удалении ручки.
#[derive(Debug)]
pub struct ScreenCapture {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    source: ScreenSource,
    config: CaptureConfig,
}

impl ScreenCapture {
    /// Запускает захват и возвращает ручку и приёмник кадров.
    ///
    /// Канал ограничен: если потребитель не успевает, новые кадры
    /// пропускаются без блокировки потока захвата.
    ///
    /// # Errors
    ///
    /// Неверная конфигурация или недоступный источник.
    pub fn start(
        source: ScreenSource,
        config: CaptureConfig,
    ) -> VoiceResult<(Self, mpsc::Receiver<CapturedFrame>)> {
        config.validate()?;
        find_source(&source)?;

        let (tx, rx) = mpsc::channel(2);
        let running = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&running);
        let interval = Duration::from_millis(u64::from(1000 / config.fps.max(1)));
        let thread_source = source.clone();

        let handle = std::thread::Builder::new()
            .name("bs-voice-capture".into())
            .spawn(move || {
                let capture = match find_source(&thread_source) {
                    Ok(handle) => handle,
                    Err(e) => {
                        warn!(error = %e, "захват не запущен");
                        return;
                    }
                };
                while flag.load(Ordering::Relaxed) {
                    match capture.capture_frame() {
                        Ok(frame) => match tx.try_send(frame) {
                            Ok(()) => {}
                            Err(mpsc::error::TrySendError::Full(_frame)) => {
                                debug!("кадр пропущен: потребитель не успевает");
                            }
                            Err(mpsc::error::TrySendError::Closed(_frame)) => {
                                debug!("приёмник кадров закрыт");
                                break;
                            }
                        },
                        Err(e) => warn!(error = %e, "кадр не захвачен"),
                    }
                    std::thread::sleep(interval);
                }
            })
            .map_err(|e| VoiceError::Capture(e.to_string()))?;

        Ok((
            Self {
                running,
                handle: Some(handle),
                source,
                config,
            },
            rx,
        ))
    }

    /// Источник, с которого идёт захват.
    #[must_use]
    pub fn source(&self) -> &ScreenSource {
        &self.source
    }

    /// Конфигурация захвата.
    #[must_use]
    pub const fn config(&self) -> CaptureConfig {
        self.config
    }

    /// Останавливает поток захвата и дожидается его завершения.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        assert!(CaptureConfig::default().validate().is_ok());
    }

    #[test]
    fn zero_fps_is_rejected() {
        let cfg = CaptureConfig {
            fps: 0,
            ..CaptureConfig::default()
        };
        assert!(matches!(cfg.validate(), Err(VoiceError::Config(_))));
    }

    #[test]
    fn too_high_fps_is_rejected() {
        let cfg = CaptureConfig {
            fps: 120,
            ..CaptureConfig::default()
        };
        assert!(matches!(cfg.validate(), Err(VoiceError::Config(_))));
    }

    #[test]
    fn too_small_width_is_rejected() {
        let cfg = CaptureConfig {
            max_width: 1,
            ..CaptureConfig::default()
        };
        assert!(matches!(cfg.validate(), Err(VoiceError::Config(_))));
    }

    #[test]
    fn small_frames_are_not_scaled() {
        let cfg = CaptureConfig {
            max_width: 1920,
            downscale: true,
            fps: 15,
        };
        assert_eq!(cfg.output_size(1280, 720), (1280, 720));
    }

    #[test]
    fn large_frames_keep_aspect_ratio() {
        let cfg = CaptureConfig {
            max_width: 1920,
            downscale: true,
            fps: 15,
        };
        assert_eq!(cfg.output_size(3840, 2160), (1920, 1080));
    }

    #[test]
    fn scaled_height_is_even() {
        let cfg = CaptureConfig {
            max_width: 1000,
            downscale: true,
            fps: 15,
        };
        let (w, h) = cfg.output_size(1920, 1080);
        assert_eq!(w, 1000);
        assert_eq!(h % 2, 0, "высота {h} должна быть чётной для I420");
    }

    #[test]
    fn downscale_disabled_keeps_size() {
        let cfg = CaptureConfig {
            max_width: 640,
            downscale: false,
            fps: 15,
        };
        assert_eq!(cfg.output_size(3840, 2160), (3840, 2160));
    }

    #[test]
    fn output_size_never_becomes_zero_for_nonzero_tiny_frames() {
        let cfg = CaptureConfig {
            max_width: 2,
            downscale: false,
            fps: 15,
        };
        assert_eq!(cfg.output_size(1, 1), (2, 2));
        let cfg = CaptureConfig {
            max_width: 2,
            downscale: true,
            fps: 15,
        };
        assert_eq!(cfg.output_size(2, 1), (2, 2));
    }

    #[test]
    fn source_roundtrip_through_json() {
        let src = ScreenSource {
            id: 7,
            name: "Монитор 1".into(),
            target: CaptureTarget::Monitor,
            width: 1920,
            height: 1080,
        };
        let json = serde_json::to_string(&src).unwrap();
        assert!(json.contains("\"target\":\"monitor\""), "{json}");
        assert_eq!(serde_json::from_str::<ScreenSource>(&json).unwrap(), src);
    }
}
