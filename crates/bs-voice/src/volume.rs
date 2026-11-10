//! Громкость участников и локальный миксдаун.
//!
//! В libwebrtc 0.3.54 у `RtcAudioTrack` нет метода громкости: звук удалённых
//! треков воспроизводит системный playout (Platform ADM). Поэтому:
//!
//! - `0 %` реализуется как отписка/`set_enabled(false)` на публикации;
//! - промежуточные значения требуют собственного миксдауна (модуль [`crate::mixer`]),
//!   который забирает PCM из `NativeAudioStream` и отдаёт его в приложение;
//! - UI оперирует [`Volume`] — доменным значением, не зависящим от бэкенда.

use serde::{Deserialize, Serialize};

use crate::error::{VoiceError, VoiceResult};

/// Максимальная допустимая громкость, %.
pub const MAX_VOLUME_PERCENT: u16 = 200;

/// Громкость участника в процентах (0..=200).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Volume(u16);

impl Default for Volume {
    fn default() -> Self {
        Self(100)
    }
}

impl Volume {
    /// Нормальная громкость.
    pub const NORMAL: Self = Self(100);
    /// Полная тишина.
    pub const MUTED: Self = Self(0);

    /// Создаёт громкость с обрезкой в допустимый диапазон.
    #[must_use]
    pub const fn new(percent: u16) -> Self {
        if percent > MAX_VOLUME_PERCENT {
            Self(MAX_VOLUME_PERCENT)
        } else {
            Self(percent)
        }
    }

    /// Значение в процентах.
    #[must_use]
    pub const fn percent(self) -> u16 {
        self.0
    }

    /// Линейный коэффициент усиления 0.0..=2.0.
    #[must_use]
    pub fn gain(self) -> f32 {
        f32::from(self.0) / 100.0
    }

    /// Нужно ли вообще воспроизводить трек.
    #[must_use]
    pub const fn is_audible(self) -> bool {
        self.0 > 0
    }

    /// Разбирает громкость из строки (например, из конфига).
    pub fn parse(value: &str) -> VoiceResult<Self> {
        let trimmed = value.trim().trim_end_matches('%');
        let parsed: u16 = trimmed
            .parse()
            .map_err(|e| VoiceError::Config(format!("громкость '{value}': {e}")))?;
        if parsed > MAX_VOLUME_PERCENT {
            return Err(VoiceError::Config(format!(
                "громкость {parsed}% выше максимума {MAX_VOLUME_PERCENT}%"
            )));
        }
        Ok(Self(parsed))
    }
}

/// Громкости всех участников, кроме локального.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VolumeMap {
    inner: std::collections::BTreeMap<String, Volume>,
}

impl VolumeMap {
    /// Громкость участника (по умолчанию 100 %).
    #[must_use]
    pub fn get(&self, identity: &str) -> Volume {
        self.inner.get(identity).copied().unwrap_or_default()
    }

    /// Устанавливает громкость; возвращает предыдущее значение.
    pub fn set(&mut self, identity: impl Into<String>, volume: Volume) -> Volume {
        let key = identity.into();
        if volume == Volume::default() {
            self.inner.remove(&key).unwrap_or_default()
        } else {
            self.inner.insert(key, volume).unwrap_or_default()
        }
    }

    /// Убирает запись участника (при выходе из комнаты).
    pub fn forget(&mut self, identity: &str) -> Option<Volume> {
        self.inner.remove(identity)
    }

    /// Перебирает всех, кому задана нестандартная громкость.
    pub fn iter(&self) -> impl Iterator<Item = (&str, Volume)> {
        self.inner.iter().map(|(k, v)| (k.as_str(), *v))
    }

    /// Сколько записей.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Пусто ли.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_clamps() {
        assert_eq!(Volume::new(500).percent(), MAX_VOLUME_PERCENT);
        assert_eq!(Volume::new(0), Volume::MUTED);
        assert!(!Volume::MUTED.is_audible());
        assert!(Volume::NORMAL.is_audible());
        assert!((Volume::new(150).gain() - 1.5).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_accepts_percent_sign_and_rejects_overflow() {
        assert_eq!(Volume::parse(" 75% ").expect("valid"), Volume::new(75));
        assert!(Volume::parse("201").is_err());
        assert!(Volume::parse("abc").is_err());
    }

    #[test]
    fn map_defaults_to_normal() {
        let mut map = VolumeMap::default();
        assert_eq!(map.get("u1"), Volume::NORMAL);
        map.set("u1", Volume::new(50));
        assert_eq!(map.get("u1"), Volume::new(50));
        assert_eq!(map.len(), 1);
        // возврат к норме удаляет запись
        map.set("u1", Volume::NORMAL);
        assert!(map.is_empty());
    }

    #[test]
    fn map_forgets_participant() {
        let mut map = VolumeMap::default();
        map.set("u1", Volume::MUTED);
        assert_eq!(map.forget("u1"), Some(Volume::MUTED));
        assert_eq!(map.forget("u1"), None);
    }
}
