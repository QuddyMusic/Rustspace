//! Видео MP4/MOV с H.264 на чистом Rust: контейнер разбирает `mp4`, кадры
//! декодирует `rusty_h264` (без C и без `unsafe` в декодере).
//!
//! Этого хватает для видео из Telegram (включая «кружочки»), с телефонов и из
//! большинства мессенджеров. Другие кодеки (HEVC, VP9, AV1) дают
//! [`VideoError::Codec`]: такой файл остаётся только скачать.

use std::io::Cursor;
use std::sync::Arc;

use rusty_h264_decoder::Decoder;

/// Кадр больше этого (по каждой стороне) не декодируем.
const MAX_SIDE: u16 = 4096;
/// Глубина перестановки кадров (B-кадры приходят не по порядку показа).
const REORDER: usize = 4;

/// Что не так с видео.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VideoError {
    /// Не MP4/MOV или файл повреждён.
    #[error("контейнер не разобран: {0}")]
    Container(String),
    /// В файле нет видеодорожки.
    #[error("в файле нет видео")]
    NoVideo,
    /// Видео не в H.264.
    #[error("кодек не поддерживается")]
    Codec,
    /// Кадр слишком большой.
    #[error("слишком большой кадр")]
    TooLarge,
}

/// Сведения о видео.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoInfo {
    /// Ширина кадра на экране (с учётом поворота).
    pub width: u32,
    /// Высота кадра на экране.
    pub height: u32,
    /// Длительность, секунды.
    pub duration: f64,
    /// Поворот при показе: 0, 90, 180 или 270 градусов по часовой.
    pub rotation: u16,
}

/// Готовый кадр в RGBA (уже повёрнутый).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// Время показа, миллисекунды от начала.
    pub pts_ms: u64,
    /// Ширина.
    pub width: u32,
    /// Высота.
    pub height: u32,
    /// Пиксели RGBA построчно.
    pub rgba: Vec<u8>,
}

/// Поток кадров видео.
pub struct VideoStream {
    reader: mp4::Mp4Reader<Cursor<Arc<[u8]>>>,
    track: u32,
    timescale: u64,
    count: u32,
    /// Номер следующего сэмпла (с 1).
    next: u32,
    /// Время декодирования каждого сэмпла (единицы `timescale`).
    starts: Vec<u64>,
    /// Ключевые сэмплы; пусто — ключевые все.
    sync: Vec<u32>,
    /// SPS и PPS в виде Annex-B: перед первым сэмплом после старта/перемотки.
    head: Vec<u8>,
    nal_len: usize,
    decoder: Decoder,
    pending: Vec<VideoFrame>,
    /// Кадры раньше этого (мс) пропускаются: перемотка.
    skip_before: u64,
    send_head: bool,
    bt709: bool,
    info: VideoInfo,
}

impl std::fmt::Debug for VideoStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoStream")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

/// Поворот по матрице дорожки.
const fn rotation(a: i32, b: i32, c: i32, d: i32) -> u16 {
    match (a.signum(), b.signum(), c.signum(), d.signum()) {
        (0, 1, -1, 0) => 90,
        (-1, 0, 0, -1) => 180,
        (0, -1, 1, 0) => 270,
        _ => 0,
    }
}

impl VideoStream {
    /// Открывает видео.
    ///
    /// # Errors
    /// Не MP4/MOV, нет видео, не H.264 или кадр слишком велик.
    pub fn open(bytes: Arc<[u8]>) -> Result<Self, VideoError> {
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let reader = mp4::Mp4Reader::read_header(Cursor::new(bytes), size)
            .map_err(|err| VideoError::Container(err.to_string()))?;
        let (track, video) = reader
            .tracks()
            .iter()
            .find(|(_, track)| track.track_type().ok() == Some(mp4::TrackType::Video))
            .map(|(id, track)| (*id, track.trak.clone()))
            .ok_or(VideoError::NoVideo)?;
        let avc1 = video
            .mdia
            .minf
            .stbl
            .stsd
            .avc1
            .as_ref()
            .ok_or(VideoError::Codec)?;
        if avc1.width > MAX_SIDE || avc1.height > MAX_SIDE || avc1.width == 0 || avc1.height == 0 {
            return Err(VideoError::TooLarge);
        }
        let mut head = Vec::new();
        for set in avc1
            .avcc
            .sequence_parameter_sets
            .iter()
            .chain(&avc1.avcc.picture_parameter_sets)
        {
            head.extend_from_slice(&[0, 0, 0, 1]);
            head.extend_from_slice(&set.bytes);
        }
        let stbl = &video.mdia.minf.stbl;
        let mut starts = Vec::new();
        let mut at = 0_u64;
        for entry in &stbl.stts.entries {
            for _ in 0..entry.sample_count {
                starts.push(at);
                at += u64::from(entry.sample_delta);
            }
        }
        let timescale = u64::from(video.mdia.mdhd.timescale.max(1));
        let matrix = &video.tkhd.matrix;
        let rotation = rotation(matrix.a, matrix.b, matrix.c, matrix.d);
        let (w, h) = (u32::from(avc1.width), u32::from(avc1.height));
        let (width, height) = if rotation.is_multiple_of(180) {
            (w, h)
        } else {
            (h, w)
        };
        #[allow(clippy::cast_precision_loss, reason = "длительность для полосы")]
        let duration = at as f64 / timescale as f64;
        let count = reader
            .sample_count(track)
            .map_err(|err| VideoError::Container(err.to_string()))?;
        Ok(Self {
            reader,
            track,
            timescale,
            count,
            next: 1,
            starts,
            sync: stbl
                .stss
                .as_ref()
                .map(|stss| stss.entries.clone())
                .unwrap_or_default(),
            head,
            nal_len: usize::from(avc1.avcc.length_size_minus_one & 3) + 1,
            decoder: Decoder::new(),
            pending: Vec::new(),
            skip_before: 0,
            send_head: true,
            bt709: h >= 720 || w >= 1280,
            info: VideoInfo {
                width,
                height,
                duration,
                rotation,
            },
        })
    }

    /// Сведения о видео.
    #[must_use]
    pub const fn info(&self) -> &VideoInfo {
        &self.info
    }

    fn ms(&self, units: i64) -> u64 {
        let units = u64::try_from(units).unwrap_or(0);
        units.saturating_mul(1000) / self.timescale
    }

    /// Следующий кадр в порядке показа; `None` — видео кончилось.
    pub fn next_frame(&mut self) -> Option<VideoFrame> {
        loop {
            if self.pending.len() > REORDER || (self.next > self.count && !self.pending.is_empty())
            {
                let first = self
                    .pending
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, frame)| frame.pts_ms)
                    .map(|(i, _)| i)?;
                let frame = self.pending.swap_remove(first);
                if frame.pts_ms + 1 < self.skip_before {
                    continue;
                }
                return Some(frame);
            }
            if self.next > self.count {
                return None;
            }
            let sample = self
                .reader
                .read_sample(self.track, self.next)
                .ok()
                .flatten();
            self.next += 1;
            let Some(sample) = sample else {
                continue;
            };
            let mut unit = if self.send_head {
                self.send_head = false;
                self.head.clone()
            } else {
                Vec::with_capacity(sample.bytes.len() + 16)
            };
            let data = sample.bytes.as_ref();
            let mut at = 0;
            while at + self.nal_len <= data.len() {
                let len = data[at..at + self.nal_len]
                    .iter()
                    .fold(0_usize, |len, byte| len << 8 | usize::from(*byte));
                at += self.nal_len;
                let Some(nal) = data.get(at..at + len) else {
                    break;
                };
                unit.extend_from_slice(&[0, 0, 0, 1]);
                unit.extend_from_slice(nal);
                at += len;
            }
            let Ok(Some(yuv)) = self.decoder.decode(&unit) else {
                continue;
            };
            let pts = i64::try_from(sample.start_time).unwrap_or(i64::MAX)
                + i64::from(sample.rendering_offset);
            let pts_ms = self.ms(pts);
            self.pending
                .push(to_rgba(&yuv, self.info.rotation, self.bt709, pts_ms));
        }
    }

    /// Перемотка: следующие кадры — с `ms` (от ближайшего ключевого кадра).
    pub fn seek(&mut self, ms: u64) {
        let target = ms.saturating_mul(self.timescale) / 1000;
        let index = self.starts.partition_point(|start| *start <= target).max(1);
        let index = u32::try_from(index).unwrap_or(1);
        let key = if self.sync.is_empty() {
            index
        } else {
            self.sync
                .iter()
                .copied()
                .filter(|key| *key <= index)
                .max()
                .unwrap_or(1)
        };
        self.next = key;
        self.decoder = Decoder::new();
        self.pending.clear();
        self.send_head = true;
        self.skip_before = ms;
    }
}

/// Первый кадр видео (миниатюра) и сведения о нём.
///
/// # Errors
/// См. [`VideoStream::open`]; ни одного кадра — [`VideoError::Codec`].
pub fn first_frame(bytes: &[u8]) -> Result<(VideoInfo, VideoFrame), VideoError> {
    let mut stream = VideoStream::open(Arc::from(bytes))?;
    let frame = stream.next_frame().ok_or(VideoError::Codec)?;
    Ok((stream.info, frame))
}

/// YUV 4:2:0 → RGBA с поворотом.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names,
    reason = "значения ограничены 0..=255; обозначения как в формулах YUV"
)]
fn to_rgba(
    yuv: &rusty_h264_common::types::YuvFrame,
    rotation: u16,
    bt709: bool,
    pts_ms: u64,
) -> VideoFrame {
    let (w, h) = (yuv.width, yuv.height);
    let (out_w, out_h) = if rotation.is_multiple_of(180) {
        (w, h)
    } else {
        (h, w)
    };
    let mut rgba = vec![255_u8; out_w * out_h * 4];
    // Коэффициенты (×256) для ограниченного диапазона.
    let (rv, gu, gv, bu) = if bt709 {
        (459, 55, 136, 541)
    } else {
        (409, 100, 208, 516)
    };
    let cw = w.div_ceil(2);
    for y in 0..h {
        for x in 0..w {
            let luma = i32::from(yuv.y.get(y * w + x).copied().unwrap_or(16)) - 16;
            let c = (y / 2) * cw + x / 2;
            let u = i32::from(yuv.u.get(c).copied().unwrap_or(128)) - 128;
            let v = i32::from(yuv.v.get(c).copied().unwrap_or(128)) - 128;
            let l = 298 * luma + 128;
            let r = ((l + rv * v) >> 8).clamp(0, 255) as u8;
            let g = ((l - gu * u - gv * v) >> 8).clamp(0, 255) as u8;
            let b = ((l + bu * u) >> 8).clamp(0, 255) as u8;
            let (ox, oy) = match rotation {
                90 => (h - 1 - y, x),
                180 => (w - 1 - x, h - 1 - y),
                270 => (y, w - 1 - x),
                _ => (x, y),
            };
            let at = (oy * out_w + ox) * 4;
            if let Some(px) = rgba.get_mut(at..at + 3) {
                px.copy_from_slice(&[r, g, b]);
            }
        }
    }
    VideoFrame {
        pts_ms,
        width: u32::try_from(out_w).unwrap_or(0),
        height: u32::try_from(out_h).unwrap_or(0),
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests-data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn plays_h264_with_b_frames_in_order() {
        let mut stream = VideoStream::open(Arc::from(sample("high.mp4"))).unwrap();
        let info = *stream.info();
        assert_eq!((info.width, info.height, info.rotation), (160, 96, 0));
        assert!((info.duration - 1.0).abs() < 0.05, "{info:?}");
        let mut last = None;
        let mut count = 0;
        while let Some(frame) = stream.next_frame() {
            assert_eq!(frame.rgba.len(), 160 * 96 * 4);
            assert!(
                last.is_none_or(|last| frame.pts_ms > last),
                "порядок показа"
            );
            last = Some(frame.pts_ms);
            count += 1;
        }
        assert_eq!(count, 25);
    }

    #[test]
    fn seeks_to_the_middle() {
        let mut stream = VideoStream::open(Arc::from(sample("high.mp4"))).unwrap();
        stream.seek(600);
        let frame = stream.next_frame().unwrap();
        assert!((600..680).contains(&frame.pts_ms), "{}", frame.pts_ms);
    }

    #[test]
    fn rotated_video_is_turned() {
        let (info, frame) = first_frame(&sample("rotated.mp4")).unwrap();
        assert_eq!((info.width, info.height, info.rotation), (96, 160, 90));
        assert_eq!((frame.width, frame.height), (96, 160));
    }

    #[test]
    fn red_stays_red() {
        let (_, frame) = first_frame(&sample("red.mp4")).unwrap();
        let px = &frame.rgba[..3];
        assert!(px[0] > 200 && px[1] < 60 && px[2] < 60, "{px:?}");
    }

    #[test]
    fn rejects_other_codecs_and_garbage() {
        assert!(matches!(
            first_frame(b"not a video"),
            Err(VideoError::Container(_))
        ));
        assert_eq!(
            first_frame(&sample("vp9.mp4")).err(),
            Some(VideoError::Codec)
        );
    }
}
