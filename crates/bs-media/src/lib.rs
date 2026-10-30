//! Безопасное декодирование картинок и анимаций.
//!
//! Файлы приходят с чужого сервера, поэтому декодер никому не доверяет:
//! формат определяется по содержимому (только PNG, JPEG, GIF и WebP — без
//! SVG и экзотики), размеры проверяются **до** распаковки пикселей, а
//! число кадров и суммарный объём анимации ограничены. Большие картинки
//! уменьшаются до [`Limits::max_output_side`], чтобы не тратить видеопамять.
//!
//! Крейт ничего не знает про сеть и GUI: байты на входе, RGBA-кадры на выходе.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::io::Cursor;

use image::{
    AnimationDecoder as _, DynamicImage, ImageDecoder as _, ImageFormat, ImageReader,
    codecs::{gif::GifDecoder, webp::WebPDecoder},
    imageops::FilterType,
};

/// Нижняя граница задержки кадра: меньшие значения браузеры тоже считают 100 мс.
const MIN_FRAME_DELAY_MS: u32 = 20;
/// Задержка кадра, если файл её не указал.
const DEFAULT_FRAME_DELAY_MS: u32 = 100;

/// Ограничения на входной файл и результат.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Максимальный размер входного файла в байтах.
    pub max_input_bytes: usize,
    /// Максимальная длина стороны исходной картинки в пикселях.
    pub max_side: u32,
    /// Максимальное число пикселей исходной картинки.
    pub max_pixels: u64,
    /// Максимальное число кадров анимации (остальные отбрасываются).
    pub max_frames: usize,
    /// Максимальный объём всех RGBA-кадров результата в байтах.
    pub max_total_bytes: usize,
    /// Сторона, до которой уменьшаются большие картинки.
    pub max_output_side: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: 16 * 1024 * 1024,
            max_side: 16_384,
            max_pixels: 40_000_000,
            max_frames: 150,
            max_total_bytes: 64 * 1024 * 1024,
            max_output_side: 2048,
        }
    }
}

/// Один кадр: RGBA8 и время показа.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    /// Пиксели RGBA, `width * height * 4` байт.
    pub rgba: Vec<u8>,
    /// Сколько миллисекунд показывать кадр.
    pub delay_ms: u32,
}

impl std::fmt::Debug for Frame {
    /// Пиксели в логи не попадают: только размер.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("bytes", &self.rgba.len())
            .field("delay_ms", &self.delay_ms)
            .finish()
    }
}

/// Результат декодирования.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// Ширина всех кадров.
    pub width: u32,
    /// Высота всех кадров.
    pub height: u32,
    /// Кадры; у статичной картинки ровно один.
    pub frames: Vec<Frame>,
}

impl Decoded {
    /// Есть ли у картинки анимация.
    #[must_use]
    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }

    /// Объём пикселей всех кадров в байтах.
    #[must_use]
    pub fn byte_size(&self) -> usize {
        self.frames.iter().map(|f| f.rgba.len()).sum()
    }
}

/// Почему файл не показан.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MediaError {
    /// Файл больше допустимого.
    #[error("файл слишком большой ({size} байт, максимум {limit})")]
    InputTooLarge {
        /// Фактический размер.
        size: usize,
        /// Лимит.
        limit: usize,
    },
    /// Формат не входит в список поддерживаемых.
    #[error("неподдерживаемый формат изображения")]
    UnsupportedFormat,
    /// У картинки нулевой или слишком большой размер.
    #[error("недопустимый размер картинки: {width}×{height}")]
    BadDimensions {
        /// Ширина.
        width: u32,
        /// Высота.
        height: u32,
    },
    /// Файл повреждён.
    #[error("не удалось декодировать изображение: {0}")]
    Decode(String),
}

impl From<image::ImageError> for MediaError {
    fn from(err: image::ImageError) -> Self {
        Self::Decode(err.to_string())
    }
}

/// Форматы, которые мы согласны разбирать.
fn supported(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
    )
}

/// Проверяет размеры до выделения памяти под пиксели.
fn check_dimensions(width: u32, height: u32, limits: &Limits) -> Result<(), MediaError> {
    let pixels = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || width > limits.max_side
        || height > limits.max_side
        || pixels > limits.max_pixels
    {
        return Err(MediaError::BadDimensions { width, height });
    }
    Ok(())
}

/// Размер после уменьшения так, чтобы большая сторона не превышала `max_side`.
fn fit(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= max_side {
        return (width, height);
    }
    let scale = f64::from(max_side) / f64::from(longest);
    let scaled = |side: u32| ((f64::from(side) * scale).round() as u32).max(1);
    (scaled(width), scaled(height))
}

/// Декодирует картинку или анимацию с учётом ограничений.
///
/// # Errors
/// Возвращает [`MediaError`], если файл слишком большой, не поддерживается,
/// имеет недопустимые размеры или повреждён.
pub fn decode(bytes: &[u8], limits: &Limits) -> Result<Decoded, MediaError> {
    if bytes.len() > limits.max_input_bytes {
        return Err(MediaError::InputTooLarge {
            size: bytes.len(),
            limit: limits.max_input_bytes,
        });
    }
    let format = image::guess_format(bytes).map_err(|_| MediaError::UnsupportedFormat)?;
    if !supported(format) {
        return Err(MediaError::UnsupportedFormat);
    }
    match format {
        ImageFormat::Gif => {
            let decoder = GifDecoder::new(Cursor::new(bytes))?;
            let (w, h) = decoder.dimensions();
            check_dimensions(w, h, limits)?;
            animated(decoder.into_frames(), w, h, limits)
        }
        ImageFormat::WebP => {
            let decoder = WebPDecoder::new(Cursor::new(bytes))?;
            let (w, h) = decoder.dimensions();
            check_dimensions(w, h, limits)?;
            if decoder.has_animation() {
                animated(decoder.into_frames(), w, h, limits)
            } else {
                still_from(decoder, limits)
            }
        }
        _ => {
            let reader = ImageReader::with_format(Cursor::new(bytes), format);
            let decoder = reader.into_decoder()?;
            let (w, h) = decoder.dimensions();
            check_dimensions(w, h, limits)?;
            still_from(decoder, limits)
        }
    }
}

/// Статичная картинка из декодера: ориентация EXIF и уменьшение.
fn still_from(
    mut decoder: impl image::ImageDecoder,
    limits: &Limits,
) -> Result<Decoded, MediaError> {
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    let (w, h) = (image.width(), image.height());
    let (nw, nh) = fit(w, h, limits.max_output_side);
    let rgba = if (nw, nh) == (w, h) {
        image.into_rgba8()
    } else {
        image::imageops::resize(&image.into_rgba8(), nw, nh, FilterType::Triangle)
    };
    Ok(Decoded {
        width: nw,
        height: nh,
        frames: vec![Frame {
            rgba: rgba.into_raw(),
            delay_ms: 0,
        }],
    })
}

/// Анимация: кадры собираются, пока не исчерпан лимит кадров и памяти.
fn animated(
    frames: image::Frames<'_>,
    width: u32,
    height: u32,
    limits: &Limits,
) -> Result<Decoded, MediaError> {
    let (nw, nh) = fit(width, height, limits.max_output_side);
    let frame_bytes = (nw as usize)
        .saturating_mul(nh as usize)
        .saturating_mul(4)
        .max(1);
    let by_memory = (limits.max_total_bytes / frame_bytes).max(1);
    let cap = limits.max_frames.min(by_memory).max(1);

    let mut out = Vec::new();
    for frame in frames {
        if out.len() >= cap {
            break;
        }
        let frame = frame?;
        let (numer, denom) = frame.delay().numer_denom_ms();
        let delay_ms = if denom == 0 || numer == 0 {
            DEFAULT_FRAME_DELAY_MS
        } else {
            (numer / denom).max(MIN_FRAME_DELAY_MS)
        };
        let buffer = frame.into_buffer();
        let rgba = if (nw, nh) == (width, height) {
            buffer.into_raw()
        } else {
            image::imageops::resize(&buffer, nw, nh, FilterType::Triangle).into_raw()
        };
        out.push(Frame { rgba, delay_ms });
    }
    if out.is_empty() {
        return Err(MediaError::Decode("в анимации нет кадров".to_owned()));
    }
    if out.len() == 1 {
        // Одиночный кадр — обычная картинка: задержка ни к чему.
        out[0].delay_ms = 0;
    }
    Ok(Decoded {
        width: nw,
        height: nh,
        frames: out,
    })
}

#[cfg(test)]
mod tests {
    use super::{Limits, MediaError, decode, fit};
    use image::{
        Delay, Frame as ImgFrame, ImageEncoder as _, Rgba, RgbaImage,
        codecs::{gif::GifEncoder, png::PngEncoder},
    };

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = RgbaImage::from_pixel(w, h, Rgba([200, 30, 60, 255]));
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8)
            .unwrap();
        out
    }

    fn gif(frames: u32, w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            for i in 0..frames {
                let shade = u8::try_from(i * 40 % 255).unwrap();
                let img = RgbaImage::from_pixel(w, h, Rgba([shade, 10, 10, 255]));
                enc.encode_frame(ImgFrame::from_parts(
                    img,
                    0,
                    0,
                    Delay::from_numer_denom_ms(80, 1),
                ))
                .unwrap();
            }
        }
        out
    }

    #[test]
    fn decodes_still_png() {
        let d = decode(&png(8, 6), &Limits::default()).unwrap();
        assert_eq!((d.width, d.height), (8, 6));
        assert_eq!(d.frames.len(), 1);
        assert!(!d.is_animated());
        assert_eq!(d.frames[0].rgba.len(), 8 * 6 * 4);
    }

    #[test]
    fn decodes_animated_gif_with_delays() {
        let d = decode(&gif(3, 4, 4), &Limits::default()).unwrap();
        assert!(d.is_animated());
        assert_eq!(d.frames.len(), 3);
        assert!(d.frames.iter().all(|f| f.delay_ms == 80));
    }

    #[test]
    fn frame_count_is_capped() {
        let limits = Limits {
            max_frames: 2,
            ..Limits::default()
        };
        assert_eq!(decode(&gif(5, 4, 4), &limits).unwrap().frames.len(), 2);
    }

    #[test]
    fn memory_budget_caps_frames() {
        // Кадр 4×4×4 = 64 байта; бюджета хватает на три.
        let limits = Limits {
            max_total_bytes: 64 * 3,
            ..Limits::default()
        };
        assert_eq!(decode(&gif(10, 4, 4), &limits).unwrap().frames.len(), 3);
    }

    #[test]
    fn rejects_unsupported_and_garbage() {
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>";
        assert_eq!(
            decode(svg, &Limits::default()),
            Err(MediaError::UnsupportedFormat)
        );
        assert_eq!(
            decode(&[0u8; 32], &Limits::default()),
            Err(MediaError::UnsupportedFormat)
        );
    }

    #[test]
    fn rejects_oversized_input_before_decoding() {
        let limits = Limits {
            max_input_bytes: 10,
            ..Limits::default()
        };
        assert!(matches!(
            decode(&png(8, 8), &limits),
            Err(MediaError::InputTooLarge { .. })
        ));
    }

    #[test]
    fn rejects_huge_dimensions_from_header_alone() {
        let limits = Limits {
            max_side: 100,
            ..Limits::default()
        };
        assert_eq!(
            decode(&png(200, 10), &limits),
            Err(MediaError::BadDimensions {
                width: 200,
                height: 10
            })
        );
        let limits = Limits {
            max_pixels: 50,
            ..Limits::default()
        };
        assert!(matches!(
            decode(&png(10, 10), &limits),
            Err(MediaError::BadDimensions { .. })
        ));
    }

    #[test]
    fn truncated_png_is_a_decode_error_not_a_panic() {
        let mut bytes = png(64, 64);
        bytes.truncate(bytes.len() / 2);
        assert!(matches!(
            decode(&bytes, &Limits::default()),
            Err(MediaError::Decode(_))
        ));
    }

    #[test]
    fn large_images_are_downscaled_keeping_aspect() {
        let limits = Limits {
            max_output_side: 50,
            ..Limits::default()
        };
        let d = decode(&png(200, 100), &limits).unwrap();
        assert_eq!((d.width, d.height), (50, 25));
        assert_eq!(d.frames[0].rgba.len(), 50 * 25 * 4);
    }

    #[test]
    fn fit_never_returns_zero() {
        assert_eq!(fit(1, 4000, 100), (1, 100));
        assert_eq!(fit(10, 10, 100), (10, 10));
    }
}
