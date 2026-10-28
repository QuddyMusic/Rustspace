//! Локальный миксдаун удалённых треков с учётом персональной громкости.
//!
//! Слой получает PCM-фреймы (`AudioFrame`) из `NativeAudioStream` каждого
//! удалённого трека и складывает их в один буфер. Это единственный способ
//! реализовать громкость 0..200 %, поскольку libwebrtc не даёт API громкости
//! на трек. Приложение (bs-app/bs-tui) само решает, отдавать ли результат в
//! системный playout или рисовать уровни.

use std::collections::BTreeMap;

use crate::volume::Volume;

/// Один источник звука: PCM-буфер и его громкость.
#[derive(Debug, Clone)]
pub struct MixSource {
    /// Identity участника.
    pub identity: String,
    /// Текущая громкость.
    pub volume: Volume,
    /// Моно-PCM последнего фрейма.
    pub samples: Vec<i16>,
}

impl MixSource {
    /// Создаёт источник с пустым буфером.
    #[must_use]
    pub fn new(identity: impl Into<String>, volume: Volume) -> Self {
        Self {
            identity: identity.into(),
            volume,
            samples: Vec::new(),
        }
    }

    /// Заменяет буфер свежим фреймом.
    pub fn push_frame(&mut self, samples: &[i16]) {
        self.samples.clear();
        self.samples.extend_from_slice(samples);
    }
}

/// Микшер удалённых источников.
#[derive(Debug, Clone, Default)]
pub struct Mixer {
    sources: BTreeMap<String, MixSource>,
}

impl Mixer {
    /// Добавляет участника в микс.
    pub fn add(&mut self, identity: impl Into<String>, volume: Volume) {
        let identity = identity.into();
        self.sources
            .insert(identity.clone(), MixSource::new(identity, volume));
    }

    /// Убирает участника из микса.
    pub fn remove(&mut self, identity: &str) -> Option<MixSource> {
        self.sources.remove(identity)
    }

    /// Меняет громкость участника; возвращает `false`, если его нет в миксе.
    pub fn set_volume(&mut self, identity: &str, volume: Volume) -> bool {
        self.sources
            .get_mut(identity)
            .map(|s| s.volume = volume)
            .is_some()
    }

    /// Записывает фрейм участника.
    pub fn push(&mut self, identity: &str, samples: &[i16]) -> bool {
        self.sources
            .get_mut(identity)
            .map(|s| s.push_frame(samples))
            .is_some()
    }

    /// Сколько источников.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// Пусто ли.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Смешивает все источники в `out` (моно, i16 с насыщением).
    ///
    /// `out` обрезается/дополняется до длины самого длинного источника.
    pub fn mix_into(&self, out: &mut Vec<i16>) {
        let Some(max_len) = self.sources.values().map(|s| s.samples.len()).max() else {
            out.clear();
            return;
        };
        out.clear();
        out.resize(max_len, 0);

        for source in self.sources.values() {
            if !source.volume.is_audible() {
                continue;
            }
            let gain = source.volume.gain();
            for (acc, sample) in out.iter_mut().zip(source.samples.iter()) {
                let scaled = (f32::from(*sample) * gain).round();
                let mixed = f32::from(*acc) + scaled;
                *acc = mixed.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_two_sources_with_gain() {
        let mut mixer = Mixer::default();
        mixer.add("a", Volume::new(100));
        mixer.add("b", Volume::new(50));
        assert!(mixer.push("a", &[100, -100]));
        assert!(mixer.push("b", &[100, 100]));

        let mut out = Vec::new();
        mixer.mix_into(&mut out);
        assert_eq!(out, vec![150, -50]);
    }

    #[test]
    fn muted_source_is_skipped() {
        let mut mixer = Mixer::default();
        mixer.add("a", Volume::new(100));
        mixer.add("b", Volume::MUTED);
        mixer.push("a", &[10]);
        mixer.push("b", &[1000]);

        let mut out = Vec::new();
        mixer.mix_into(&mut out);
        assert_eq!(out, vec![10]);
    }

    #[test]
    fn saturates_instead_of_wrapping() {
        let mut mixer = Mixer::default();
        mixer.add("a", Volume::new(200));
        mixer.add("b", Volume::new(200));
        mixer.push("a", &[i16::MAX]);
        mixer.push("b", &[i16::MAX]);

        let mut out = Vec::new();
        mixer.mix_into(&mut out);
        assert_eq!(out, vec![i16::MAX]);
    }

    #[test]
    fn empty_mixer_clears_output() {
        let mut out = vec![1, 2, 3];
        Mixer::default().mix_into(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn set_volume_requires_present_source() {
        let mut mixer = Mixer::default();
        mixer.add("a", Volume::NORMAL);
        assert!(mixer.set_volume("a", Volume::new(30)));
        assert!(!mixer.set_volume("zzz", Volume::new(30)));
    }
}
