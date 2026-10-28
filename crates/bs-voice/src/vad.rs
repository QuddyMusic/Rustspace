//! Детектор речи по уровню звука (energy-based VAD).
//!
//! LiveKit сам сообщает `ActiveSpeakersChanged`, но для локального микрофона
//! нужен независимый детектор: он даёт мгновенный индикатор в UI и позволяет
//! не полагаться на серверную задержку.

use std::time::{Duration, Instant};

/// Порог срабатывания по умолчанию (0.0..=1.0 в шкале LiveKit).
const DEFAULT_THRESHOLD: f32 = 0.02;
/// Сколько тишины нужно, чтобы считать речь законченной.
const DEFAULT_HOLD: Duration = Duration::from_millis(300);

/// Простой энергетический детектор речи с гистерезисом.
#[derive(Debug, Clone)]
pub struct VoiceDetector {
    threshold: f32,
    hold: Duration,
    speaking: bool,
    last_voice: Option<Instant>,
}

impl Default for VoiceDetector {
    fn default() -> Self {
        Self::new(DEFAULT_THRESHOLD, DEFAULT_HOLD)
    }
}

impl VoiceDetector {
    /// Создаёт детектор с порогом и временем удержания.
    #[must_use]
    pub const fn new(threshold: f32, hold: Duration) -> Self {
        Self {
            threshold,
            hold,
            speaking: false,
            last_voice: None,
        }
    }

    /// Текущее состояние «говорит».
    #[must_use]
    pub const fn is_speaking(&self) -> bool {
        self.speaking
    }

    /// Порог срабатывания.
    #[must_use]
    pub const fn threshold(&self) -> f32 {
        self.threshold
    }

    /// Меняет порог на лету.
    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = if threshold.is_finite() {
            threshold.clamp(0.0, 1.0)
        } else {
            DEFAULT_THRESHOLD
        };
    }

    /// Обрабатывает новый уровень звука, возвращает `true`, если состояние изменилось.
    pub fn update(&mut self, level: f32, now: Instant) -> bool {
        let level = if level.is_finite() {
            level.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if level >= self.threshold {
            self.last_voice = Some(now);
            if !self.speaking {
                self.speaking = true;
                return true;
            }
            return false;
        }

        if self.speaking {
            let silent_for = self
                .last_voice
                .map_or(self.hold, |t| now.saturating_duration_since(t));
            if silent_for >= self.hold {
                self.speaking = false;
                return true;
            }
        }
        false
    }

    /// Сбрасывает состояние (например, при выходе из канала).
    pub fn reset(&mut self) {
        self.speaking = false;
        self.last_voice = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_speech_and_holds() {
        let mut d = VoiceDetector::new(0.1, Duration::from_millis(200));
        let t0 = Instant::now();
        assert!(d.update(0.5, t0));
        assert!(d.is_speaking());
        assert!(!d.update(0.5, t0 + Duration::from_millis(50)));
        // тишина, но удержание ещё не истекло
        assert!(!d.update(0.0, t0 + Duration::from_millis(150)));
        assert!(d.is_speaking());
        // удержание истекло
        assert!(d.update(0.0, t0 + Duration::from_millis(400)));
        assert!(!d.is_speaking());
    }

    #[test]
    fn nan_and_inf_are_silence() {
        let mut d = VoiceDetector::default();
        assert!(!d.update(f32::NAN, Instant::now()));
        assert!(!d.update(f32::INFINITY, Instant::now()));
        assert!(!d.is_speaking());
    }

    #[test]
    fn threshold_is_clamped() {
        let mut d = VoiceDetector::default();
        d.set_threshold(5.0);
        assert!((d.threshold() - 1.0).abs() < f32::EPSILON);
        d.set_threshold(f32::NAN);
        assert!((d.threshold() - DEFAULT_THRESHOLD).abs() < f32::EPSILON);
    }

    #[test]
    fn reset_clears_state() {
        let mut d = VoiceDetector::default();
        d.update(1.0, Instant::now());
        assert!(d.is_speaking());
        d.reset();
        assert!(!d.is_speaking());
    }
}
