//! Проверка микрофона: уровень входного сигнала без подключения к комнате.
//!
//! Создаёт локальный трек от того же источника, что и звонок, и читает его
//! PCM через `NativeAudioStream`. Уровень — пик за последний кадр (10 мс),
//! 0.0–1.0, с плавным спадом, чтобы индикатор не дёргался. Монитор
//! останавливается при `Drop`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use livekit::webrtc::audio_stream::native::NativeAudioStream;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt as _;

use crate::audio::AudioEngine;
use crate::error::VoiceResult;

/// Частота, в которой читаем кадры.
const SAMPLE_RATE: i32 = 48_000;
/// Доля, на которую уровень падает за кадр без сигнала.
const DECAY: f32 = 0.9;

/// Идущая проверка микрофона.
#[derive(Debug)]
pub struct MicMonitor {
    level: Arc<AtomicU32>,
    task: JoinHandle<()>,
    /// Захват запустил сам монитор (и должен его остановить).
    owns_recording: bool,
    engine: AudioEngine,
}

impl MicMonitor {
    /// Запускает захват (если он ещё не идёт) и чтение уровня.
    ///
    /// # Errors
    /// Микрофон не открылся.
    pub fn start(engine: &AudioEngine, handle: &Handle) -> VoiceResult<Self> {
        let owns_recording = !engine.is_recording();
        engine.start()?;
        let track = engine.create_mic_track("mic-test");
        let mut stream = NativeAudioStream::new(track.rtc_track(), SAMPLE_RATE, 1);
        let level = Arc::new(AtomicU32::new(0));
        let sink = Arc::clone(&level);
        let task = handle.spawn(async move {
            // Трек держим живым, пока читаем.
            let _track = track;
            while let Some(frame) = stream.next().await {
                let peak = frame
                    .data
                    .iter()
                    .map(|sample| sample.unsigned_abs())
                    .max()
                    .unwrap_or(0);
                let now = f32::from(peak) / f32::from(i16::MAX);
                let previous = f32::from_bits(sink.load(Ordering::Relaxed));
                sink.store(now.max(previous * DECAY).to_bits(), Ordering::Relaxed);
            }
        });
        Ok(Self {
            level,
            task,
            owns_recording,
            engine: engine.clone(),
        })
    }

    /// Текущий уровень, 0.0–1.0.
    #[must_use]
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed)).clamp(0.0, 1.0)
    }
}

impl Drop for MicMonitor {
    fn drop(&mut self) {
        self.task.abort();
        if self.owns_recording {
            let _ = self.engine.stop();
        }
    }
}
