//! Идентификаторы-снежинки в формате оригинала: 42 бита времени от
//! 2024-01-01, 10 бит номера воркера, 12 бит счётчика.

use std::sync::{Mutex, PoisonError};

/// Начало отсчёта: 1 января 2024 года, UTC, в миллисекундах.
const EPOCH_MS: u64 = 1_704_067_200_000;

/// Генератор идентификаторов.
#[derive(Debug)]
pub struct Snowflake {
    worker: u64,
    /// Последняя метка времени и счётчик в её пределах.
    state: Mutex<(u64, u64)>,
}

fn now_ms() -> u64 {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

impl Snowflake {
    /// Генератор для воркера `worker` (0–1023, хранится в настройках инстанса).
    #[must_use]
    pub fn new(worker: u16) -> Self {
        Self {
            worker: u64::from(worker & 0x3FF),
            state: Mutex::new((0, 0)),
        }
    }

    /// Следующий идентификатор (десятичной строкой, как у оригинала).
    ///
    /// В отличие от оригинала метка времени не идёт назад, если системные
    /// часы перевели: идентификаторы остаются возрастающими и уникальными.
    #[must_use]
    pub fn next(&self) -> String {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (last, sequence) = *state;
        let mut timestamp = now_ms().max(last);
        let sequence = if timestamp == last {
            let next = (sequence + 1) & 0xFFF;
            if next == 0 {
                // Счётчик миллисекунды исчерпан: берём следующую.
                timestamp += 1;
            }
            next
        } else {
            0
        };
        *state = (timestamp, sequence);
        drop(state);
        let id = (timestamp.saturating_sub(EPOCH_MS) << 22) | (self.worker << 12) | sequence;
        id.to_string()
    }
}

/// Время создания идентификатора, мс от начала эпохи Unix.
#[must_use]
pub fn timestamp_of(id: &str) -> Option<u64> {
    id.parse::<u64>().ok().map(|id| (id >> 22) + EPOCH_MS)
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_increasing_and_carry_time() {
        let generator = Snowflake::new(513);
        let before = now_ms();
        let ids: Vec<u64> = (0..10_000)
            .map(|_| generator.next().parse().expect("число"))
            .collect();
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!((ids[0] >> 12) & 0x3FF, 513);
        let stamp = timestamp_of(&ids[0].to_string()).expect("время");
        assert!(stamp >= before && stamp <= now_ms() + 10);
    }
}
