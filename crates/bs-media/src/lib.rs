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

#[cfg(feature = "video")]
pub mod video;

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

/// Уменьшенная копия статичной картинки для маленьких мест.
///
/// Нужна для аватарок и значков: видеокарта берёт образцы без усреднения,
/// поэтому большая картинка в 40 пикселях выглядит рваной, а заранее
/// уменьшенная — чёткой.
#[derive(Clone, PartialEq, Eq)]
pub struct Thumb {
    /// Ширина.
    pub width: u32,
    /// Высота.
    pub height: u32,
    /// Пиксели RGBA.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Thumb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Thumb")
            .field("size", &(self.width, self.height))
            .finish_non_exhaustive()
    }
}

/// Длинная сторона миниатюры.
pub const THUMB_SIDE: u32 = 256;

/// Результат декодирования.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// Уменьшенная копия: есть у статичных картинок заметно больше миниатюры.
    pub thumbs: Vec<Thumb>,
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
        self.frames.iter().map(|f| f.rgba.len()).sum::<usize>()
            + self.thumbs.iter().map(|t| t.rgba.len()).sum::<usize>()
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

/// Кодирует RGBA8 в PNG (для картинок из буфера обмена).
///
/// # Errors
/// Размер не совпадает с числом байт или кодировщик отказал.
pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, MediaError> {
    use image::ImageEncoder as _;
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if width == 0 || height == 0 || rgba.len() != expected {
        return Err(MediaError::Decode("неверный размер пикселей".to_owned()));
    }
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|err| MediaError::Decode(err.to_string()))?;
    Ok(out)
}

/// Квадратная иконка: первый кадр, обрезка по центру до квадрата,
/// уменьшение до `side` и PNG. Для иконок групп и пространств.
///
/// # Errors
/// Файл не картинка, слишком большой или повреждён.
pub fn square_icon(bytes: &[u8], side: u32, limits: &Limits) -> Result<Vec<u8>, MediaError> {
    let decoded = decode(bytes, limits)?;
    let frame = decoded
        .frames
        .first()
        .ok_or_else(|| MediaError::Decode("нет кадров".to_owned()))?;
    let image = image::RgbaImage::from_raw(decoded.width, decoded.height, frame.rgba.clone())
        .ok_or_else(|| MediaError::Decode("неверный размер пикселей".to_owned()))?;
    let edge = decoded.width.min(decoded.height);
    let x = (decoded.width - edge) / 2;
    let y = (decoded.height - edge) / 2;
    let cropped = image::imageops::crop_imm(&image, x, y, edge, edge).to_image();
    let target = edge.min(side.max(1));
    let resized = if target == edge {
        cropped
    } else {
        image::imageops::resize(
            &cropped,
            target,
            target,
            image::imageops::FilterType::Lanczos3,
        )
    };
    encode_png(resized.as_raw(), target, target)
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
    let thumbs = make_thumbs(&rgba);
    Ok(Decoded {
        thumbs,
        width: nw,
        height: nh,
        frames: vec![Frame {
            rgba: rgba.into_raw(),
            delay_ms: 0,
        }],
    })
}

/// Стороны уменьшенных копий, по возрастанию.
const THUMB_STEPS: [u32; 3] = [64, 128, THUMB_SIDE];

/// Цепочка уменьшенных копий: у каждой своя сторона, пока картинка заметно
/// больше неё. Мелкие места (аватары) берут ближайшую копию — так нет «рваности».
fn make_thumbs(image: &image::RgbaImage) -> Vec<Thumb> {
    let (w, h) = image.dimensions();
    THUMB_STEPS
        .iter()
        .filter(|side| w.max(h) > **side * 3 / 2)
        .map(|side| {
            let (tw, th) = fit(w, h, *side);
            let small = image::imageops::resize(image, tw, th, FilterType::Lanczos3);
            Thumb {
                width: tw,
                height: th,
                rgba: small.into_raw(),
            }
        })
        .collect()
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
        thumbs: Vec::new(),
        width: nw,
        height: nh,
        frames: out,
    })
}

#[cfg(test)]
mod tests {
    use super::{Limits, MediaError, THUMB_SIDE, decode, encode_png, fit, square_icon};
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

    #[test]
    fn encode_png_roundtrips_and_rejects_bad_sizes() {
        let rgba = [10u8, 20, 30, 255].repeat(6);
        let bytes = super::encode_png(&rgba, 3, 2).expect("png");
        let back = decode(&bytes, &Limits::default()).expect("decode");
        assert_eq!((back.width, back.height), (3, 2));
        assert!(super::encode_png(&rgba, 4, 2).is_err());
        assert!(super::encode_png(&[], 0, 0).is_err());
    }

    #[test]
    fn large_still_gets_a_sharp_thumbnail_and_small_one_does_not() {
        let big = decode(&png(1000, 500), &Limits::default()).unwrap();
        let thumb = big.thumbs.last().expect("thumb");
        assert_eq!((thumb.width, thumb.height), (THUMB_SIDE, THUMB_SIDE / 2));
        assert_eq!(big.thumbs.len(), 3);
        assert_eq!(thumb.rgba.len(), (thumb.width * thumb.height * 4) as usize);
        let small = decode(&png(100, 100), &Limits::default()).unwrap();
        assert!(
            small
                .thumbs
                .iter()
                .all(|t| t.width.max(t.height) < THUMB_SIDE)
        );
    }

    #[test]
    fn square_icon_crops_center_and_shrinks() {
        // 40×20: левая половина красная, правая — синяя; центр 20×20 — граница.
        let mut rgba = Vec::new();
        for _y in 0..20 {
            for x in 0..40 {
                rgba.extend_from_slice(if x < 20 {
                    &[255, 0, 0, 255]
                } else {
                    &[0, 0, 255, 255]
                });
            }
        }
        let png = encode_png(&rgba, 40, 20).unwrap();
        let icon = square_icon(&png, 8, &Limits::default()).unwrap();
        let decoded = decode(&icon, &Limits::default()).unwrap();
        assert_eq!((decoded.width, decoded.height), (8, 8));
        let px = |x: usize| &decoded.frames[0].rgba[x * 4..x * 4 + 3];
        assert!(px(0)[0] > 200, "слева красный");
        assert!(px(7)[2] > 200, "справа синий");
        assert!(square_icon(b"not an image", 8, &Limits::default()).is_err());
    }
}
