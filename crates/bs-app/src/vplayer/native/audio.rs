//! Звук видео: дорожка AAC из MP4/MOV через symphonia.
//!
//! Декодер rodio бросает чтение на первом же пакете видео, поэтому пакеты
//! чужих дорожек пропускаем сами.

use std::io::Cursor;
use std::num::NonZero;
use std::sync::Arc;
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, DeviceSinkBuilder, MixerDeviceSink, Player, SampleRate, Source};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions};
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

/// Звуковая дорожка как источник rodio.
struct Track {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    id: u32,
    channels: u16,
    rate: u32,
    buf: Vec<f32>,
    at: usize,
}

impl Track {
    fn open(bytes: Arc<[u8]>) -> Option<Self> {
        let source = MediaSourceStream::new(
            Box::new(Cursor::new(bytes)),
            symphonia::core::io::MediaSourceStreamOptions::default(),
        );
        let mut hint = Hint::new();
        hint.with_extension("mp4");
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                source,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .ok()?;
        let format = probed.format;
        let track = format.tracks().iter().find(|track| {
            track.codec_params.codec != CODEC_TYPE_NULL && track.codec_params.sample_rate.is_some()
        })?;
        let params = track.codec_params.clone();
        let decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .ok()?;
        let channels = params
            .channels
            .map_or(2, symphonia::core::audio::Channels::count);
        let mut track = Self {
            id: track.id,
            format,
            decoder,
            channels: u16::try_from(channels).unwrap_or(2).max(1),
            rate: params.sample_rate.unwrap_or(44_100),
            buf: Vec::new(),
            at: 0,
        };
        // В MP4 число каналов часто не записано: берём его из первого пакета.
        track.fill()?;
        Some(track)
    }

    /// Декодирует следующий пакет своей дорожки в буфер.
    fn fill(&mut self) -> Option<()> {
        loop {
            let packet = self.format.next_packet().ok()?;
            if packet.track_id() != self.id {
                continue;
            }
            let Ok(decoded) = self.decoder.decode(&packet) else {
                continue;
            };
            let spec = *decoded.spec();
            self.rate = spec.rate;
            self.channels = u16::try_from(spec.channels.count()).unwrap_or(2).max(1);
            let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            samples.copy_interleaved_ref(decoded);
            self.buf = samples.samples().to_vec();
            self.at = 0;
            return Some(());
        }
    }
}

impl Iterator for Track {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        loop {
            if let Some(sample) = self.buf.get(self.at) {
                self.at += 1;
                return Some(*sample);
            }
            self.fill()?;
        }
    }
}

impl Source for Track {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        NonZero::new(self.channels).unwrap_or(NonZero::<u16>::MIN)
    }

    fn sample_rate(&self) -> SampleRate {
        NonZero::new(self.rate).unwrap_or(NonZero::<u32>::MIN)
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let time = Time::from(pos.as_secs_f64());
        let _ = self.format.seek(
            SeekMode::Coarse,
            SeekTo::Time {
                time,
                track_id: Some(self.id),
            },
        );
        self.decoder.reset();
        self.buf.clear();
        self.at = 0;
        Ok(())
    }
}

/// Устройство и плеер звука видео.
pub(super) struct Output {
    _sink: MixerDeviceSink,
    player: Player,
}

impl Output {
    /// `None` — в файле нет звука (или устройство недоступно).
    pub(super) fn open(bytes: Arc<[u8]>) -> Option<Self> {
        let track = Track::open(bytes)?;
        let mut sink = DeviceSinkBuilder::open_default_sink().ok()?;
        sink.log_on_drop(false);
        let player = Player::connect_new(sink.mixer());
        player.append(track);
        Some(Self {
            _sink: sink,
            player,
        })
    }

    pub(super) fn pause(&self) {
        self.player.pause();
    }

    pub(super) fn play(&self) {
        self.player.play();
    }

    pub(super) fn seek(&self, secs: f32) {
        let _ = self.player.try_seek(Duration::from_secs_f32(secs.max(0.0)));
    }

    pub(super) fn position(&self) -> f32 {
        self.player.get_pos().as_secs_f32()
    }

    pub(super) fn finished(&self) -> bool {
        self.player.empty()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    #[test]
    fn reads_the_whole_audio_track_of_a_video() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests-data/video/with-audio.mp4"
        );
        let bytes: Arc<[u8]> = std::fs::read(path).expect("файл").into();
        let track = Track::open(bytes).expect("дорожка");
        let (rate, channels) = (track.rate, usize::from(track.channels));
        #[allow(clippy::cast_precision_loss, reason = "короткий файл")]
        let secs = track.count() as f32 / (rate as usize * channels) as f32;
        assert!((secs - 1.0).abs() < 0.1, "{secs}");
    }
}
