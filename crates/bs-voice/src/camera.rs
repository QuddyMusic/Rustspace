//! Захват камеры.
//!
//! С флагом `camera` работает через nokhwa (V4L2, Media Foundation,
//! AVFoundation). Объекты камеры не `Send`, поэтому, как и захват экрана,
//! живут в своём потоке и отдают наружу готовые кадры RGBA. Без флага все
//! функции честно отвечают, что камеры в сборке нет.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::capture::CapturedFrame;
#[cfg(not(feature = "camera"))]
use crate::error::VoiceError;
use crate::error::VoiceResult;

/// Камера в системе.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraInfo {
    /// Порядковый номер (его и передаём при запуске).
    pub index: u32,
    /// Человекочитаемое имя.
    pub name: String,
}

/// Желаемые параметры камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CameraConfig {
    /// Ширина кадра.
    pub width: u32,
    /// Высота кадра.
    pub height: u32,
    /// Кадров в секунду.
    pub fps: u32,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fps: 30,
        }
    }
}

/// Ручка идущего захвата камеры; останавливается при `Drop`.
#[derive(Debug)]
pub struct CameraCapture {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl CameraCapture {
    /// Останавливает поток захвата.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CameraCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// RGB8 → RGBA8.
#[must_use]
pub fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
    for pixel in rgb.chunks_exact(3) {
        rgba.extend_from_slice(pixel);
        rgba.push(u8::MAX);
    }
    rgba
}

#[cfg(feature = "camera")]
mod native {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use nokhwa::Camera;
    use nokhwa::pixel_format::RgbFormat;
    use nokhwa::utils::{
        ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
        Resolution,
    };
    use tokio::sync::mpsc;
    use tracing::{debug, info, warn};

    use super::{CameraCapture, CameraConfig, CameraInfo, rgb_to_rgba};
    use crate::capture::CapturedFrame;
    use crate::error::{VoiceError, VoiceResult};

    fn error(err: impl std::fmt::Display) -> VoiceError {
        VoiceError::Video(format!("камера: {err}"))
    }

    fn init() {
        #[cfg(target_os = "macos")]
        if !nokhwa::nokhwa_check() {
            nokhwa::nokhwa_initialize(|granted| {
                if !granted {
                    warn!("доступ к камере не выдан");
                }
            });
        }
    }

    pub(super) fn list() -> VoiceResult<Vec<CameraInfo>> {
        init();
        let cameras = nokhwa::query(ApiBackend::Auto).map_err(error)?;
        Ok(cameras
            .iter()
            .enumerate()
            .map(|(position, camera)| CameraInfo {
                index: match camera.index() {
                    CameraIndex::Index(index) => *index,
                    CameraIndex::String(_) => u32::try_from(position).unwrap_or(u32::MAX),
                },
                name: camera.human_name(),
            })
            .collect())
    }

    pub(super) fn start(
        index: u32,
        config: CameraConfig,
    ) -> VoiceResult<(CameraCapture, mpsc::Receiver<CapturedFrame>)> {
        init();
        let (tx, rx) = mpsc::channel(2);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<VoiceResult<()>>();
        let running = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&running);
        let handle = std::thread::Builder::new()
            .name("bs-voice-camera".into())
            .spawn(move || {
                let format = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(
                    CameraFormat::new(
                        Resolution::new(config.width, config.height),
                        FrameFormat::MJPEG,
                        config.fps,
                    ),
                ));
                let opened = Camera::new(CameraIndex::Index(index), format)
                    .and_then(|mut camera| camera.open_stream().map(|()| camera));
                let mut camera = match opened {
                    Ok(camera) => {
                        let _ = ready_tx.send(Ok(()));
                        camera
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(error(err)));
                        return;
                    }
                };
                info!(format = %camera.camera_format(), "камера открыта");
                while flag.load(Ordering::Relaxed) {
                    let frame = camera
                        .frame()
                        .and_then(|buffer| buffer.decode_image::<RgbFormat>());
                    match frame {
                        Ok(image) => {
                            let (width, height) = image.dimensions();
                            let frame = CapturedFrame {
                                width,
                                height,
                                rgba: rgb_to_rgba(image.as_raw()),
                            };
                            if let Err(mpsc::error::TrySendError::Closed(_)) = tx.try_send(frame) {
                                break;
                            }
                        }
                        Err(err) => {
                            warn!(error = %err, "кадр камеры не получен");
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                    }
                }
                let _ = camera.stop_stream();
                debug!("камера закрыта");
            })
            .map_err(error)?;
        ready_rx
            .recv()
            .unwrap_or_else(|_| Err(error("поток камеры завершился")))?;
        Ok((
            CameraCapture {
                running,
                handle: Some(handle),
            },
            rx,
        ))
    }
}

/// Перечисляет камеры.
///
/// # Errors
/// Система не отдала список или сборка без камеры.
pub fn list_cameras() -> VoiceResult<Vec<CameraInfo>> {
    #[cfg(feature = "camera")]
    {
        native::list()
    }
    #[cfg(not(feature = "camera"))]
    {
        Err(VoiceError::Video("сборка без поддержки камеры".into()))
    }
}

/// Запускает камеру; кадры приходят в канал (лишние пропускаются).
///
/// # Errors
/// Камера не открылась или сборка без камеры.
pub fn start_camera(
    index: u32,
    config: CameraConfig,
) -> VoiceResult<(CameraCapture, mpsc::Receiver<CapturedFrame>)> {
    #[cfg(feature = "camera")]
    {
        native::start(index, config)
    }
    #[cfg(not(feature = "camera"))]
    {
        let _ = (index, config);
        Err(VoiceError::Video("сборка без поддержки камеры".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_gets_opaque_alpha() {
        assert_eq!(
            rgb_to_rgba(&[1, 2, 3, 4, 5, 6]),
            vec![1, 2, 3, 255, 4, 5, 6, 255]
        );
    }
}
