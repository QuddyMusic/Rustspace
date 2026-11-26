//! Десктоп: rodio (symphonia) и Ogg Opus.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};

use crate::i18n::tr;

mod opus;

type Boxed = Box<dyn Source + Send>;

/// Устройство, плеер и сам файл (чтобы начать заново после конца).
#[derive(Default)]
pub(super) struct Backend {
    sink: Option<MixerDeviceSink>,
    player: Option<Player>,
    bytes: Option<(Arc<[u8]>, String)>,
    len: Option<f32>,
}

impl Backend {
    fn sink(&mut self) -> Result<&MixerDeviceSink, String> {
        if self.sink.is_none() {
            let mut sink = DeviceSinkBuilder::open_default_sink().map_err(|err| {
                crate::tf!(
                    "Звуковое устройство недоступно: {err}",
                    err = err.to_string()
                )
            })?;
            sink.log_on_drop(false);
            self.sink = Some(sink);
        }
        self.sink
            .as_ref()
            .ok_or_else(|| tr("Звуковое устройство недоступно").to_owned())
    }

    fn source(bytes: &Arc<[u8]>, ext: &str) -> Result<Boxed, String> {
        if opus::is_ogg_opus(bytes) {
            return Ok(Box::new(opus::OggOpus::new(bytes)?));
        }
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let mut builder = Decoder::builder()
            .with_data(Cursor::new(Arc::clone(bytes)))
            .with_byte_len(len)
            .with_seekable(true);
        if !ext.is_empty() {
            builder = builder.with_hint(ext);
        }
        builder
            .build()
            .map(|decoder| Box::new(decoder) as Boxed)
            .map_err(|err| crate::tf!("Формат не поддерживается: {err}", err = err.to_string()))
    }

    fn play_from(&mut self, at: f32) -> Result<(), String> {
        let (bytes, ext) = self
            .bytes
            .clone()
            .ok_or_else(|| tr("Нет файла").to_owned())?;
        let source = Self::source(&bytes, &ext)?;
        self.len = source.total_duration().map(|d| d.as_secs_f32());
        let player = Player::connect_new(self.sink()?.mixer());
        player.append(source);
        if at > 0.0 {
            let _ = player.try_seek(Duration::from_secs_f32(at));
        }
        self.player = Some(player);
        Ok(())
    }

    pub(super) fn start(&mut self, bytes: Vec<u8>, ext: &str) -> Result<(), String> {
        self.stop();
        self.bytes = Some((bytes.into(), ext.to_owned()));
        self.play_from(0.0)
    }

    pub(super) fn pause(&self) {
        if let Some(player) = &self.player {
            player.pause();
        }
    }

    /// Продолжает; после конца дорожки начинает с `at` заново.
    pub(super) fn resume(&mut self, at: f32) -> Result<(), String> {
        match &self.player {
            Some(player) if !player.empty() => {
                player.play();
                Ok(())
            }
            _ => self.play_from(at),
        }
    }

    pub(super) fn seek(&mut self, secs: f32) -> bool {
        let pos = Duration::from_secs_f32(secs.max(0.0));
        match &self.player {
            Some(player) if !player.empty() => player.try_seek(pos).is_ok(),
            // Доиграла: заново с нужного места, но на паузе.
            Some(_) => {
                let ok = self.play_from(secs).is_ok();
                self.pause();
                ok
            }
            None => false,
        }
    }

    pub(super) fn position(&self) -> f32 {
        self.player
            .as_ref()
            .map_or(0.0, |player| player.get_pos().as_secs_f32())
    }

    pub(super) const fn len(&self) -> Option<f32> {
        self.len
    }

    pub(super) fn finished(&self) -> bool {
        self.player.as_ref().is_some_and(Player::empty)
    }

    pub(super) fn stop(&mut self) {
        // `Player` без `detach` замолкает, когда его бросают.
        self.player = None;
        self.bytes = None;
        self.len = None;
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    fn decode(name: &str) -> (Boxed, usize) {
        let path = format!("{}/tests-data/audio/{name}", env!("CARGO_MANIFEST_DIR"));
        let bytes: Arc<[u8]> = std::fs::read(path).expect("файл").into();
        let ext = name.rsplit_once('.').map_or("", |(_, ext)| ext);
        let source = Backend::source(&bytes, ext).expect(name);
        let count = Backend::source(&bytes, ext).expect(name).count();
        (source, count)
    }

    #[test]
    #[allow(clippy::cast_precision_loss, reason = "короткие файлы")]
    fn decodes_popular_formats() {
        for name in ["t.wav", "t.mp3", "t.flac", "t.ogg", "t.m4a", "t.opus"] {
            let (source, count) = decode(name);
            let rate = source.sample_rate().get() as usize;
            let channels = usize::from(source.channels().get());
            let secs = count as f32 / (rate * channels) as f32;
            let want = if name == "t.opus" { 1.5 } else { 0.5 };
            assert!((secs - want).abs() < 0.1, "{name}: {secs} с");
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss, reason = "короткие файлы")]
    fn opus_knows_length_and_seeks() {
        let (mut source, total) = decode("t.opus");
        let len = source.total_duration().expect("длина").as_secs_f32();
        assert!((len - 1.5).abs() < 0.05, "{len}");
        source.try_seek(Duration::from_secs(1)).expect("перемотка");
        let rest = source.count() as f32 / 48_000.0;
        assert!((rest - 0.5).abs() < 0.05, "{rest}, всего {total}");
    }
}
