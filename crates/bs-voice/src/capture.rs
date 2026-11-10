//! Захват экрана захватчиком самой libwebrtc — тем же, что в Chrome.
//!
//! - **Wayland**: системный портал ScreenCast (xdg-desktop-portal + PipeWire).
//!   Экран или окно выбирает пользователь в диалоге системы, кадры идут
//!   потоком PipeWire с частотой композитора. Прежний захват через
//!   wlr-screencopy делал полный скриншот на каждый кадр, путал мониторы
//!   и давал единицы кадров в секунду.
//! - **X11**: XShm/XDamage; **Windows**: DXGI/WGC; **macOS**: ScreenCaptureKit
//!   (по умолчанию тоже системный выбор).
//!
//! Захватчик живёт в своём потоке: тот раз в 1/fps просит кадр, переводит
//! его из BGRA в I420 (libyuv) с уменьшением под предел и отдаёт в канал.
//! Если получатель не успевает, лишний кадр просто пропускается.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use livekit::webrtc::desktop_capturer::{
    CaptureError, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions, DesktopFrame,
};
use livekit::webrtc::prelude::VideoBuffer as _;
use livekit::webrtc::video_frame::I420Buffer;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::convert::{PackedFormat, fit_i420, packed_to_i420};
use crate::error::{VoiceError, VoiceResult};

/// Что именно показываем.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureTarget {
    /// Весь монитор.
    Monitor,
    /// Одно окно.
    Window,
    /// Выбор в системном диалоге (портал Wayland, ScreenCaptureKit).
    System,
}

/// Описание источника захвата.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenSource {
    /// Идентификатор источника у захватчика (для `System` — 0).
    pub id: u64,
    /// Человекочитаемое имя для UI.
    pub name: String,
    /// Монитор, окно или системный выбор.
    pub target: CaptureTarget,
}

/// Готовый кадр для отправки.
#[derive(Debug)]
pub struct CapturedFrame {
    /// Кадр в I420, уже уменьшенный под предел.
    pub i420: I420Buffer,
}

impl CapturedFrame {
    /// Ширина кадра.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.i420.width()
    }

    /// Высота кадра.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.i420.height()
    }
}

/// Параметры захвата.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureConfig {
    /// Кадров в секунду.
    pub fps: u32,
    /// Предел ширины кадра; больше — уменьшаем пропорционально.
    pub max_width: u32,
    /// Предел высоты кадра.
    pub max_height: u32,
    /// Показывать курсор.
    pub cursor: bool,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            max_width: 1920,
            max_height: 1080,
            cursor: true,
        }
    }
}

impl CaptureConfig {
    /// Проверяет разумность настроек.
    ///
    /// # Errors
    ///
    /// fps вне 1..=60 или предел размера меньше 2.
    pub fn validate(&self) -> VoiceResult<()> {
        if self.fps == 0 || self.fps > 60 {
            return Err(VoiceError::Config(format!(
                "fps должен быть в 1..=60, получено {}",
                self.fps
            )));
        }
        if self.max_width < 2 || self.max_height < 2 {
            return Err(VoiceError::Config(
                "предел размера должен быть не меньше 2 для I420".into(),
            ));
        }
        Ok(())
    }
}

/// Работаем ли под Wayland (тогда выбор только через портал).
#[must_use]
pub fn is_wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn system_source() -> ScreenSource {
    ScreenSource {
        id: 0,
        name: "system".into(),
        target: CaptureTarget::System,
    }
}

fn list_of(kind: DesktopCaptureSourceType, target: CaptureTarget) -> Vec<ScreenSource> {
    let mut options = DesktopCapturerOptions::new(kind);
    #[cfg(target_os = "macos")]
    options.set_sck_system_picker(false);
    options.set_include_cursor(false);
    let Some(capturer) = DesktopCapturer::new(options) else {
        return Vec::new();
    };
    capturer
        .get_source_list()
        .into_iter()
        .map(|source| ScreenSource {
            id: source.id(),
            name: source.title(),
            target,
        })
        .collect()
}

/// Источники, которые можно показать.
///
/// Под Wayland приложение не видит чужие окна и мониторы — это делает
/// системный диалог, поэтому источник один: [`CaptureTarget::System`].
/// Если система не отдала ни одного источника (macOS без разрешения),
/// тоже предлагается системный выбор.
///
/// # Errors
///
/// Сейчас не возвращает ошибок; сигнатура оставлена на будущее.
pub fn list_sources() -> VoiceResult<Vec<ScreenSource>> {
    if is_wayland() {
        return Ok(vec![system_source()]);
    }
    let mut sources = list_of(DesktopCaptureSourceType::Screen, CaptureTarget::Monitor);
    // У мониторов WebRTC отдаёт служебные имена («screen», «\\.\\DISPLAY1»):
    // нумеруем их по порядку, как в системных настройках.
    if sources.len() > 1 || sources.iter().any(|s| s.name.trim().is_empty()) {
        for (index, source) in sources.iter_mut().enumerate() {
            source.name = format!("{}", index + 1);
        }
    }
    sources.extend(
        list_of(DesktopCaptureSourceType::Window, CaptureTarget::Window)
            .into_iter()
            .filter(|window| !window.name.trim().is_empty()),
    );
    if sources.is_empty() {
        sources.push(system_source());
    }
    Ok(sources)
}

/// Кадр захвата → I420 под предел размера.
fn convert(frame: &DesktopFrame, config: &CaptureConfig) -> Option<I420Buffer> {
    let (width, height) = (
        u32::try_from(frame.width()).ok()?,
        u32::try_from(frame.height()).ok()?,
    );
    if width < 2 || height < 2 {
        return None;
    }
    let i420 = packed_to_i420(
        frame.data(),
        frame.stride(),
        width,
        height,
        PackedFormat::Bgra,
    )
    .map_err(|err| debug!(%err, "кадр захвата пропущен"))
    .ok()?;
    Some(fit_i420(i420, config.max_width, config.max_height))
}

/// Ручка идущего захвата. Останавливает поток при `Drop`.
#[derive(Debug)]
pub struct ScreenCapture {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ScreenCapture {
    /// Запускает захват источника; кадры I420 приходят в канал.
    ///
    /// # Errors
    ///
    /// Неверные настройки или захватчик не создался.
    pub fn start(
        source: &ScreenSource,
        config: CaptureConfig,
    ) -> VoiceResult<(Self, mpsc::Receiver<CapturedFrame>)> {
        config.validate()?;
        let (tx, rx) = mpsc::channel(2);
        let running = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&running);
        let source = source.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<VoiceResult<()>>();
        let handle = std::thread::Builder::new()
            .name("bs-voice-capture".into())
            .spawn(move || capture_loop(&source, config, &flag, &tx, &ready_tx))
            .map_err(|err| VoiceError::Capture(err.to_string()))?;
        ready_rx
            .recv()
            .unwrap_or_else(|_| Err(VoiceError::Capture("поток захвата завершился".into())))?;
        Ok((
            Self {
                running,
                handle: Some(handle),
            },
            rx,
        ))
    }

    /// Останавливает захват и ждёт поток.
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

fn capture_loop(
    source: &ScreenSource,
    config: CaptureConfig,
    running: &AtomicBool,
    tx: &mpsc::Sender<CapturedFrame>,
    ready: &std::sync::mpsc::Sender<VoiceResult<()>>,
) {
    let kind = match source.target {
        CaptureTarget::Monitor => DesktopCaptureSourceType::Screen,
        CaptureTarget::Window => DesktopCaptureSourceType::Window,
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        CaptureTarget::System => DesktopCaptureSourceType::Generic,
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        CaptureTarget::System => DesktopCaptureSourceType::Screen,
    };
    let mut options = DesktopCapturerOptions::new(kind);
    options.set_include_cursor(config.cursor);
    #[cfg(target_os = "macos")]
    options.set_sck_system_picker(source.target == CaptureTarget::System);
    let Some(mut capturer) = DesktopCapturer::new(options) else {
        let _ = ready.send(Err(VoiceError::Capture(
            "система не дала захватить экран".into(),
        )));
        return;
    };
    let selected = (source.target != CaptureTarget::System)
        .then(|| {
            capturer
                .get_source_list()
                .into_iter()
                .find(|candidate| candidate.id() == source.id)
        })
        .flatten();
    if source.target != CaptureTarget::System && selected.is_none() {
        let _ = ready.send(Err(VoiceError::Capture(format!(
            "источник «{}» больше недоступен",
            source.name
        ))));
        return;
    }
    let sender = tx.clone();
    let stopped = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stopped);
    capturer.start_capture(selected, move |result| match result {
        Ok(frame) => {
            if let Some(i420) = convert(&frame, &config) {
                // Полный канал — кадр устарел ещё до отправки, пропускаем.
                if let Err(mpsc::error::TrySendError::Closed(_)) =
                    sender.try_send(CapturedFrame { i420 })
                {
                    stop_flag.store(true, Ordering::Relaxed);
                }
            }
        }
        Err(CaptureError::Temporary) => {}
        Err(CaptureError::Permanent) => {
            warn!("захват экрана прерван системой");
            stop_flag.store(true, Ordering::Relaxed);
        }
    });
    let _ = ready.send(Ok(()));
    info!(target = ?source.target, fps = config.fps, "захват экрана начат");
    let interval = Duration::from_secs(1) / config.fps;
    let mut next = Instant::now();
    while running.load(Ordering::Relaxed) && !stopped.load(Ordering::Relaxed) {
        capturer.capture_frame();
        next += interval;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    debug!("захват экрана остановлен");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        assert!(CaptureConfig::default().validate().is_ok());
    }

    #[test]
    fn bad_config_is_rejected() {
        for config in [
            CaptureConfig {
                fps: 0,
                ..CaptureConfig::default()
            },
            CaptureConfig {
                fps: 61,
                ..CaptureConfig::default()
            },
            CaptureConfig {
                max_height: 1,
                ..CaptureConfig::default()
            },
        ] {
            assert!(config.validate().is_err(), "{config:?}");
        }
    }
}
