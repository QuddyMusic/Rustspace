//! Ogg Opus (RFC 7845) как источник rodio: голосовые сообщения Telegram,
//! WhatsApp и т. п. Symphonia Opus не декодирует, поэтому — `opus-decoder`.

use std::io::Cursor;
use std::num::NonZero;
use std::time::Duration;

use opus_decoder::OpusDecoder;
use rodio::source::SeekError;
use rodio::{ChannelCount, SampleRate, Source};

use crate::i18n::tr;

/// Opus всегда декодируется в 48 кГц.
const RATE: u32 = 48_000;
/// Самый длинный пакет — 120 мс.
const MAX_FRAME: usize = 5760;

/// Начинается ли файл с заголовка Ogg Opus.
pub(super) fn is_ogg_opus(bytes: &[u8]) -> bool {
    let head = bytes.get(..bytes.len().min(512)).unwrap_or_default();
    head.starts_with(b"OggS") && head.windows(8).any(|w| w == b"OpusHead")
}

/// Пакет и позиция (в отсчётах) начала его страницы.
struct Packet {
    data: Vec<u8>,
    start: u64,
}

pub(super) struct OggOpus {
    packets: Vec<Packet>,
    decoder: OpusDecoder,
    channels: u16,
    pre_skip: u64,
    end: u64,
    next: usize,
    buf: Vec<f32>,
    filled: usize,
    at: usize,
    /// Сколько отсчётов (на канал) ещё выбросить.
    skip: u64,
}

impl OggOpus {
    pub(super) fn new(bytes: &[u8]) -> Result<Self, String> {
        let bad = || tr("Файл Opus повреждён").to_owned();
        let mut reader = ogg::PacketReader::new(Cursor::new(bytes));
        let head = reader
            .read_packet()
            .map_err(|_| bad())?
            .filter(|p| p.data.starts_with(b"OpusHead"))
            .ok_or_else(bad)?;
        let channels = *head.data.get(9).ok_or_else(bad)?;
        let pre_skip = head
            .data
            .get(10..12)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(bad)?;
        let family = head.data.get(18).copied().unwrap_or(0);
        if !(1..=2).contains(&channels) || family > 1 {
            return Err(tr("Многоканальный Opus не поддерживается").to_owned());
        }
        let serial = head.stream_serial();
        let mut packets = Vec::new();
        let (mut page_start, mut last_gp) = (0_u64, 0_u64);
        let mut tags = true;
        while let Ok(Some(packet)) = reader.read_packet() {
            if packet.stream_serial() != serial {
                continue;
            }
            if tags {
                // Второй пакет — OpusTags.
                tags = false;
                last_gp = 0;
                continue;
            }
            if packet.first_in_page() {
                page_start = last_gp;
            }
            let gp = packet.absgp_page();
            if gp != u64::MAX {
                last_gp = gp;
            }
            packets.push(Packet {
                data: packet.data,
                start: page_start,
            });
        }
        let decoder =
            OpusDecoder::new(RATE, usize::from(channels)).map_err(|err| err.to_string())?;
        Ok(Self {
            packets,
            decoder,
            channels: u16::from(channels),
            pre_skip: u64::from(pre_skip),
            end: last_gp,
            next: 0,
            buf: vec![0.0; MAX_FRAME * usize::from(channels)],
            filled: 0,
            at: 0,
            skip: u64::from(pre_skip),
        })
    }
}

impl Iterator for OggOpus {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        loop {
            if self.at < self.filled {
                let sample = self.buf.get(self.at).copied();
                self.at += 1;
                return sample;
            }
            let packet = self.packets.get(self.next)?;
            self.next += 1;
            let frames = self
                .decoder
                .decode_float(&packet.data, &mut self.buf, false)
                .unwrap_or(0);
            let channels = usize::from(self.channels);
            self.filled = frames * channels;
            let dropped = usize::try_from(self.skip).unwrap_or(usize::MAX).min(frames);
            self.skip -= dropped as u64;
            self.at = dropped * channels;
        }
    }
}

impl Source for OggOpus {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        NonZero::new(self.channels).unwrap_or(NonZero::<u16>::MIN)
    }

    fn sample_rate(&self) -> SampleRate {
        NonZero::new(RATE).unwrap_or(NonZero::<u32>::MIN)
    }

    #[allow(clippy::cast_precision_loss, reason = "длительность для полосы")]
    fn total_duration(&self) -> Option<Duration> {
        let samples = self.end.checked_sub(self.pre_skip)?;
        Some(Duration::from_secs_f64(samples as f64 / f64::from(RATE)))
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "позиция в отсчётах неотрицательна"
    )]
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let target = self.pre_skip + (pos.as_secs_f64() * f64::from(RATE)) as u64;
        // Первый пакет страницы, на которую приходится нужное место.
        let page = self
            .packets
            .iter()
            .rposition(|p| p.start <= target)
            .and_then(|last| self.packets.get(last))
            .map_or(0, |p| p.start);
        self.next = self
            .packets
            .iter()
            .position(|p| p.start == page)
            .unwrap_or(0);
        self.skip = target.saturating_sub(page);
        self.decoder.reset();
        self.filled = 0;
        self.at = 0;
        Ok(())
    }
}
