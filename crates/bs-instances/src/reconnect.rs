//! Политика переподключения: экспоненциальный backoff с ограничением.

use std::time::Duration;

/// Экспоненциальный backoff: `initial * factor^n`, но не больше `max`.
///
/// Между попытками добавляется детерминированный «джиттер» по номеру
/// попытки — чтобы несколько инстансов не били по серверу синхронно.
/// Джиттер считается без ГСЧ, чтобы политика была полностью воспроизводима
/// (и тестируема без seed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectPolicy {
    /// Задержка перед первой повторной попыткой.
    pub initial: Duration,
    /// Верхняя граница задержки.
    pub max: Duration,
    /// Множитель на каждой следующей попытке (обычно 2).
    pub factor: u32,
    /// Доля задержки, добавляемая как джиттер (0..=100 процентов).
    pub jitter_percent: u32,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(30),
            factor: 2,
            jitter_percent: 20,
        }
    }
}

impl ReconnectPolicy {
    /// Политика с явными параметрами; `factor == 0` трактуется как 1.
    pub fn new(initial: Duration, max: Duration, factor: u32, jitter_percent: u32) -> Self {
        Self {
            initial,
            max,
            factor: factor.max(1),
            jitter_percent: jitter_percent.min(100),
        }
    }

    /// Задержка перед попыткой номер `attempt` (0 — первая повторная).
    ///
    /// Не паникует и не переполняется: при больших `attempt` упирается в `max`.
    pub fn delay(&self, attempt: u32) -> Duration {
        let base = self.base_delay(attempt);
        let jitter = self.jitter_for(base, attempt);
        base.saturating_add(jitter).min(self.max)
    }

    /// Базовая экспонента без джиттера, зажатая `max`.
    fn base_delay(&self, attempt: u32) -> Duration {
        let mut ms = self.initial.as_millis().min(u128::from(u64::MAX)) as u64;
        let max_ms = self.max.as_millis().min(u128::from(u64::MAX)) as u64;
        for _ in 0..attempt {
            if ms >= max_ms {
                return self.max;
            }
            ms = ms.saturating_mul(u64::from(self.factor));
        }
        Duration::from_millis(ms.min(max_ms))
    }

    /// Джиттер как доля от `base`, детерминированно зависящая от `attempt`.
    fn jitter_for(&self, base: Duration, attempt: u32) -> Duration {
        if self.jitter_percent == 0 {
            return Duration::ZERO;
        }
        let base_ms = base.as_millis().min(u128::from(u64::MAX)) as u64;
        let span = base_ms.saturating_mul(u64::from(self.jitter_percent)) / 100;
        if span == 0 {
            return Duration::ZERO;
        }
        // Простая LCG-подобная примесь — воспроизводимо и без внешних крейтов.
        let mixed =
            (u64::from(attempt) ^ 0x9E37_79B9_7F4A_7C15).wrapping_mul(0x2545_F491_4F6C_DD1D);
        let offset = mixed % span;
        Duration::from_millis(offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_attempt_uses_initial() {
        let policy = ReconnectPolicy::new(Duration::from_secs(1), Duration::from_secs(30), 2, 0);
        assert_eq!(policy.delay(0), Duration::from_secs(1));
    }

    #[test]
    fn grows_exponentially_and_caps() {
        let policy = ReconnectPolicy::new(Duration::from_secs(1), Duration::from_secs(30), 2, 0);
        assert_eq!(policy.delay(1), Duration::from_secs(2));
        assert_eq!(policy.delay(2), Duration::from_secs(4));
        assert_eq!(policy.delay(3), Duration::from_secs(8));
        assert_eq!(policy.delay(4), Duration::from_secs(16));
        assert_eq!(policy.delay(5), Duration::from_secs(30));
        assert_eq!(policy.delay(50), Duration::from_secs(30));
    }

    #[test]
    fn never_exceeds_max_with_jitter() {
        let policy = ReconnectPolicy::default();
        for attempt in 0..64 {
            assert!(policy.delay(attempt) <= policy.max);
        }
    }

    #[test]
    fn jitter_is_deterministic() {
        let policy = ReconnectPolicy::default();
        for attempt in 0..16 {
            assert_eq!(policy.delay(attempt), policy.delay(attempt));
        }
    }

    #[test]
    fn zero_factor_is_treated_as_one() {
        let policy = ReconnectPolicy::new(Duration::from_millis(100), Duration::from_secs(5), 0, 0);
        assert_eq!(policy.factor, 1);
        assert_eq!(policy.delay(3), Duration::from_millis(100));
    }

    #[test]
    fn jitter_percent_is_clamped() {
        let policy = ReconnectPolicy::new(Duration::from_secs(1), Duration::from_secs(5), 2, 250);
        assert_eq!(policy.jitter_percent, 100);
    }
}
