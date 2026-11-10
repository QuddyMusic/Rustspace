//! Проверка микрофона: уровень входного сигнала без подключения к комнате.
//!
//! Уровень берётся из того же тракта, что и звонок (после обработки и
//! усиления), поэтому ползунок громкости микрофона сразу виден на индикаторе.
//! Монитор останавливает захват при `Drop`, если сам его запускал.

use tokio::runtime::Handle;

use crate::audio::AudioEngine;
use crate::error::VoiceResult;

/// Идущая проверка микрофона.
#[derive(Debug)]
pub struct MicMonitor {
    /// Захват запустил сам монитор (и должен его остановить).
    owns_recording: bool,
    engine: AudioEngine,
}

impl MicMonitor {
    /// Запускает захват, если он ещё не идёт.
    ///
    /// # Errors
    /// Микрофон не открылся.
    pub fn start(engine: &AudioEngine, handle: &Handle) -> VoiceResult<Self> {
        let _runtime = handle.enter();
        let owns_recording = !engine.is_recording();
        engine.start()?;
        Ok(Self {
            owns_recording,
            engine: engine.clone(),
        })
    }

    /// Текущий уровень, 0.0–1.0.
    #[must_use]
    pub fn level(&self) -> f32 {
        self.engine.level()
    }
}

impl Drop for MicMonitor {
    fn drop(&mut self) {
        if self.owns_recording {
            let _ = self.engine.stop();
        }
    }
}
