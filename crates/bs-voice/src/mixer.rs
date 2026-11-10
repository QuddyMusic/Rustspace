//! Собственный звуковой тракт: микшер удалённых источников и ресемплер.
//!
//! libwebrtc не даёт громкости на трек, поэтому весь звук идёт мимо
//! системного playout: PCM каждого удалённого трека попадает в свой буфер,
//! микшер складывает буферы с учётом громкостей (как цепочка `GainNode`
//! оригинала: общая громкость × громкость источника × приглушение
//! трансляции, пока кто-то говорит) и отдаёт поток 48 кГц моно.
//! Всё здесь — чистые вычисления без ввода-вывода.

use std::collections::{HashMap, VecDeque};

use crate::volume::Volume;

/// Частота внутреннего тракта.
pub const SAMPLE_RATE: u32 = 48_000;
/// Сэмплов в одном кадре 10 мс.
pub const FRAME_SAMPLES: usize = (SAMPLE_RATE / 100) as usize;
/// Сколько звука копим перед стартом источника (40 мс), чтобы не трещать.
const PRIME_SAMPLES: usize = FRAME_SAMPLES * 4;
/// Предел буфера источника (250 мс): дальше выбрасываем старое.
const MAX_BUFFER: usize = FRAME_SAMPLES * 25;

/// Откуда звук участника.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    /// Микрофон.
    Voice,
    /// Звук демонстрации экрана.
    Screen,
}

#[derive(Debug, Default)]
struct Channel {
    buffer: VecDeque<i16>,
    playing: bool,
}

/// Микшер удалённых источников.
#[derive(Debug)]
pub struct Mixer {
    channels: HashMap<(String, SourceKind), Channel>,
    volumes: HashMap<(String, SourceKind), f32>,
    master: f32,
    deafened: bool,
    /// Множитель для трансляций, пока кто-то говорит (1.0 — без приглушения).
    duck: f32,
    someone_speaking: bool,
}

impl Default for Mixer {
    fn default() -> Self {
        Self {
            channels: HashMap::new(),
            volumes: HashMap::new(),
            master: 1.0,
            deafened: false,
            duck: 1.0,
            someone_speaking: false,
        }
    }
}

impl Mixer {
    /// Добавляет свежий PCM источника.
    pub fn push(&mut self, identity: &str, kind: SourceKind, samples: &[i16]) {
        let channel = self
            .channels
            .entry((identity.to_owned(), kind))
            .or_default();
        channel.buffer.extend(samples);
        let excess = channel.buffer.len().saturating_sub(MAX_BUFFER);
        channel.buffer.drain(..excess);
        if channel.buffer.len() >= PRIME_SAMPLES {
            channel.playing = true;
        }
    }

    /// Убирает источник (трек отписан).
    pub fn remove(&mut self, identity: &str, kind: SourceKind) {
        self.channels.remove(&(identity.to_owned(), kind));
    }

    /// Сколько источников сейчас в миксе.
    #[must_use]
    pub fn len(&self) -> usize {
        self.channels.len()
    }

    /// Пуст ли микс.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// Громкость источника (0 — локально заглушён).
    pub fn set_volume(&mut self, identity: &str, kind: SourceKind, volume: Volume) {
        let key = (identity.to_owned(), kind);
        if volume == Volume::NORMAL {
            self.volumes.remove(&key);
        } else {
            self.volumes.insert(key, volume.gain());
        }
    }

    /// Общая громкость вывода.
    pub fn set_master(&mut self, volume: Volume) {
        self.master = volume.gain();
    }

    /// Оглушение: на выходе тишина, буферы не копятся.
    pub fn set_deafened(&mut self, deafened: bool) {
        self.deafened = deafened;
        if deafened {
            for channel in self.channels.values_mut() {
                channel.buffer.clear();
                channel.playing = false;
            }
        }
    }

    /// Приглушение трансляций при разговоре: `strength` 0–100 %, `None` — выключено.
    pub fn set_attenuation(&mut self, strength: Option<u8>) {
        self.duck = strength.map_or(1.0, |s| 1.0 - f32::from(s.min(100)) / 100.0);
    }

    /// Говорит ли сейчас кто-то из удалённых участников.
    pub const fn set_someone_speaking(&mut self, speaking: bool) {
        self.someone_speaking = speaking;
    }

    fn gain(&self, key: &(String, SourceKind)) -> f32 {
        let mut gain = self.master * self.volumes.get(key).copied().unwrap_or(1.0);
        if key.1 == SourceKind::Screen && self.someone_speaking {
            gain *= self.duck;
        }
        gain
    }

    /// Следующий сэмпл микса, −1.0..=1.0.
    pub fn next_sample(&mut self) -> f32 {
        if self.deafened {
            return 0.0;
        }
        let mut sum = 0.0;
        let keys: Vec<_> = self
            .channels
            .iter()
            .filter(|(_, c)| c.playing)
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            let gain = self.gain(&key);
            if let Some(channel) = self.channels.get_mut(&key) {
                match channel.buffer.pop_front() {
                    Some(sample) => sum = (f32::from(sample) / 32_768.0).mul_add(gain, sum),
                    // Буфер опустел: ждём, пока снова наберётся запас.
                    None => channel.playing = false,
                }
            }
        }
        soft_clip(sum)
    }

    /// Заполняет буфер сэмплами микса.
    pub fn fill(&mut self, out: &mut [f32]) {
        for sample in out {
            *sample = self.next_sample();
        }
    }
}

/// Мягкое ограничение: до 0.8 сигнал не трогаем, выше — плавно прижимаем к 1.0.
#[must_use]
pub fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let magnitude = x.abs();
    if magnitude <= KNEE {
        return x;
    }
    let over = (magnitude - KNEE) / (1.0 - KNEE);
    (1.0 - KNEE).mul_add(over.tanh(), KNEE).copysign(x)
}

/// Применяет усиление к кадру i16 с мягким ограничением; возвращает пик 0.0–1.0.
pub fn apply_gain(frame: &mut [i16], gain: f32) -> f32 {
    let mut peak = 0.0_f32;
    for sample in frame {
        let value = soft_clip(f32::from(*sample) / 32_768.0 * gain);
        peak = peak.max(value.abs());
        *sample = to_i16(value);
    }
    peak
}

/// −1.0..=1.0 → i16.
#[must_use]
pub fn to_i16(value: f32) -> i16 {
    // Значение заранее ограничено, приведение не теряет ничего, кроме дроби.
    #[allow(clippy::cast_possible_truncation)]
    let sample = (value.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
    sample
}

/// Линейный ресемплер с состоянием между вызовами.
///
/// Позиция — дробная доля между двумя соседними входными сэмплами; сравнение
/// её с 1.0 и есть шаг алгоритма, поэтому `while_float` здесь уместен.
#[derive(Debug, Clone)]
pub struct Resampler {
    /// Входных сэмплов на один выходной.
    step: f64,
    pos: f64,
    prev: f32,
    cur: f32,
}

#[allow(clippy::while_float)]
impl Resampler {
    /// Ресемплер из `from` Гц в `to` Гц.
    #[must_use]
    pub fn new(from: u32, to: u32) -> Self {
        Self {
            step: f64::from(from.max(1)) / f64::from(to.max(1)),
            pos: 0.0,
            prev: 0.0,
            cur: 0.0,
        }
    }

    /// Частоты совпадают — можно не ресемплировать.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        (self.step - 1.0).abs() < f64::EPSILON
    }

    #[allow(clippy::cast_possible_truncation)]
    fn interpolate(&self) -> f32 {
        (self.cur - self.prev).mul_add(self.pos as f32, self.prev)
    }

    /// Подаёт входной сэмпл, выходные отдаёт в `emit`.
    pub fn push(&mut self, sample: f32, mut emit: impl FnMut(f32)) {
        if self.is_identity() {
            emit(sample);
            return;
        }
        self.prev = self.cur;
        self.cur = sample;
        while self.pos < 1.0 {
            emit(self.interpolate());
            self.pos += self.step;
        }
        self.pos -= 1.0;
    }

    /// Выдаёт выходной сэмпл, по необходимости забирая входные из `next`.
    pub fn pull(&mut self, mut next: impl FnMut() -> f32) -> f32 {
        if self.is_identity() {
            return next();
        }
        while self.pos >= 1.0 {
            self.prev = self.cur;
            self.cur = next();
            self.pos -= 1.0;
        }
        let value = self.interpolate();
        self.pos += self.step;
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(value: i16, len: usize) -> Vec<i16> {
        vec![value; len]
    }

    #[test]
    fn source_waits_for_prime_then_plays() {
        let mut mixer = Mixer::default();
        mixer.push("a", SourceKind::Voice, &frame(16_384, FRAME_SAMPLES));
        assert!(mixer.next_sample().abs() < f32::EPSILON, "ещё копим запас");
        mixer.push("a", SourceKind::Voice, &frame(16_384, PRIME_SAMPLES));
        assert!((mixer.next_sample() - 0.5).abs() < 1e-3);
    }

    #[test]
    fn volumes_multiply_master_and_source() {
        let mut mixer = Mixer::default();
        mixer.push("a", SourceKind::Voice, &frame(8_192, PRIME_SAMPLES));
        mixer.set_master(Volume::new(50));
        mixer.set_volume("a", SourceKind::Voice, Volume::new(200));
        assert!((mixer.next_sample() - 0.25).abs() < 1e-3);
        mixer.set_volume("a", SourceKind::Voice, Volume::MUTED);
        assert!(mixer.next_sample().abs() < f32::EPSILON);
    }

    #[test]
    fn screen_audio_ducks_only_while_someone_speaks() {
        let mut mixer = Mixer::default();
        mixer.push("a", SourceKind::Screen, &frame(8_192, PRIME_SAMPLES * 2));
        mixer.set_attenuation(Some(50));
        assert!((mixer.next_sample() - 0.25).abs() < 1e-3);
        mixer.set_someone_speaking(true);
        assert!((mixer.next_sample() - 0.125).abs() < 1e-3);
        mixer.set_attenuation(None);
        assert!((mixer.next_sample() - 0.25).abs() < 1e-3);
    }

    #[test]
    fn deafen_silences_and_drops_buffers() {
        let mut mixer = Mixer::default();
        mixer.push("a", SourceKind::Voice, &frame(8_192, PRIME_SAMPLES));
        mixer.set_deafened(true);
        assert!(mixer.next_sample().abs() < f32::EPSILON);
        mixer.set_deafened(false);
        assert!(mixer.next_sample().abs() < f32::EPSILON, "буфер сброшен");
    }

    #[test]
    fn buffer_is_bounded() {
        let mut mixer = Mixer::default();
        for _ in 0..100 {
            mixer.push("a", SourceKind::Voice, &frame(1, FRAME_SAMPLES));
        }
        let channel = &mixer.channels[&("a".to_owned(), SourceKind::Voice)];
        assert_eq!(channel.buffer.len(), MAX_BUFFER);
    }

    #[test]
    fn sum_is_soft_clipped() {
        let mut mixer = Mixer::default();
        for id in ["a", "b", "c"] {
            mixer.push(id, SourceKind::Voice, &frame(30_000, PRIME_SAMPLES));
        }
        let value = mixer.next_sample();
        assert!(value <= 1.0 && value > 0.95);
    }

    #[test]
    fn gain_boosts_and_reports_peak() {
        let mut data = frame(8_192, 4);
        let peak = apply_gain(&mut data, 2.0);
        assert_eq!(data[0], 16_384);
        assert!((peak - 0.5).abs() < 1e-3);
        let mut loud = frame(30_000, 1);
        apply_gain(&mut loud, 2.0);
        assert!(
            loud[0] > 31_000,
            "громкое прижато к пределу, а не обрезано в ноль"
        );
    }

    #[test]
    fn resampler_keeps_rate_ratio() {
        let mut up = Resampler::new(44_100, SAMPLE_RATE);
        let mut out = 0usize;
        for _ in 0..44_100 {
            up.push(0.1, |_| out += 1);
        }
        assert!(out.abs_diff(48_000) <= 1, "получено {out}");

        let mut down = Resampler::new(SAMPLE_RATE, 44_100);
        let mut taken = 0usize;
        for _ in 0..44_100 {
            down.pull(|| {
                taken += 1;
                0.1
            });
        }
        assert!(taken.abs_diff(48_000) <= 2, "забрано {taken}");
    }

    #[test]
    fn resampler_interpolates_a_ramp() {
        let mut half = Resampler::new(1, 2);
        let mut out = Vec::new();
        for value in [0.0, 1.0, 2.0] {
            half.push(value, |v| out.push(v));
        }
        assert_eq!(out, vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.5]);
    }

    #[test]
    fn identity_resampler_passes_through() {
        let mut same = Resampler::new(SAMPLE_RATE, SAMPLE_RATE);
        let mut out = Vec::new();
        same.push(0.3, |v| out.push(v));
        assert_eq!(out, vec![0.3]);
        assert!((same.pull(|| 0.7) - 0.7).abs() < f32::EPSILON);
    }
}
