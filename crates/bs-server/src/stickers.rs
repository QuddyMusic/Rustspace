//! Картинки стикеров: приём, приведение к WebP не больше 512 точек по
//! стороне, анимация (`encodeSticker` из `routes/stickers.ts`).
//!
//! Оригинал кодирует библиотекой sharp. Здесь кадры сжимаются без потерь
//! кодировщиком крейта `image`, а анимированный файл собирается из них
//! вручную по описанию контейнера WebP (VP8X + ANIM + ANMF).

use std::io::Cursor;

use image::codecs::gif::GifDecoder;
use image::codecs::webp::{WebPDecoder, WebPEncoder};
use image::imageops::FilterType;
use image::{
    AnimationDecoder, DynamicImage, ExtendedColorType, ImageDecoder, ImageFormat, ImageReader,
    RgbaImage,
};

/// Предел размера исходного и готового стикера.
pub const MAX_STICKER_BYTES: usize = 5 * 1024 * 1024;
/// Наибольшая сторона стикера.
const MAX_DIMENSION: u32 = 512;
/// Сколько точек (по всем кадрам) разрешено разобрать.
const MAX_DECODED_PIXELS: u64 = 16_777_216;

/// Картинка не подходит для стикера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StickerImageError;

/// Размер, вписанный в квадрат 512 без увеличения.
fn fitted(width: u32, height: u32) -> (u32, u32) {
    if width <= MAX_DIMENSION && height <= MAX_DIMENSION {
        return (width, height);
    }
    let scale = f64::from(MAX_DIMENSION) / f64::from(width.max(height));
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "результат не больше 512"
    )]
    let side = |value: u32| ((f64::from(value) * scale).round() as u32).clamp(1, MAX_DIMENSION);
    (side(width), side(height))
}

fn resized(frame: RgbaImage) -> RgbaImage {
    let (width, height) = fitted(frame.width(), frame.height());
    if (width, height) == (frame.width(), frame.height()) {
        frame
    } else {
        image::imageops::resize(&frame, width, height, FilterType::Triangle)
    }
}

/// Кадр как порция `VP8L` (сжатие без потерь).
fn lossless_chunk(frame: &RgbaImage) -> Result<Vec<u8>, StickerImageError> {
    let mut file = Vec::new();
    WebPEncoder::new_lossless(&mut file)
        .encode(
            frame.as_raw(),
            frame.width(),
            frame.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|_| StickerImageError)?;
    // Кодировщик пишет целый файл RIFF; из него нужна только порция VP8L.
    let mut at = 12;
    while at + 8 <= file.len() {
        let size =
            u32::from_le_bytes([file[at + 4], file[at + 5], file[at + 6], file[at + 7]]) as usize;
        let end = at + 8 + size;
        if end > file.len() {
            break;
        }
        if &file[at..at + 4] == b"VP8L" {
            return Ok(file[at + 8..end].to_vec());
        }
        at = end + (size & 1);
    }
    Err(StickerImageError)
}

fn push_u24(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes()[..3]);
}

fn push_chunk(out: &mut Vec<u8>, name: [u8; 4], payload: &[u8]) {
    out.extend_from_slice(&name);
    out.extend_from_slice(
        &u32::try_from(payload.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
}

/// Собирает анимированный WebP из кадров одного размера.
fn animated_webp(frames: &[(RgbaImage, u32)]) -> Result<Vec<u8>, StickerImageError> {
    let (width, height) = frames
        .first()
        .map(|(frame, _)| frame.dimensions())
        .ok_or(StickerImageError)?;
    let mut body = Vec::new();
    let mut header = vec![0x02 | 0x10, 0, 0, 0];
    push_u24(&mut header, width - 1);
    push_u24(&mut header, height - 1);
    push_chunk(&mut body, *b"VP8X", &header);
    // Фон прозрачный, повторов без конца.
    push_chunk(&mut body, *b"ANIM", &[0, 0, 0, 0, 0, 0]);
    for (frame, delay_ms) in frames {
        if frame.dimensions() != (width, height) {
            return Err(StickerImageError);
        }
        let mut payload = Vec::new();
        push_u24(&mut payload, 0);
        push_u24(&mut payload, 0);
        push_u24(&mut payload, width - 1);
        push_u24(&mut payload, height - 1);
        push_u24(&mut payload, (*delay_ms).min(0x00ff_ffff));
        // Кадр полный: без наложения на предыдущий.
        payload.push(0x02);
        push_chunk(&mut payload, *b"VP8L", &lossless_chunk(frame)?);
        push_chunk(&mut body, *b"ANMF", &payload);
        if body.len() > MAX_STICKER_BYTES {
            return Err(StickerImageError);
        }
    }
    let mut file = Vec::with_capacity(body.len() + 12);
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(
        &u32::try_from(body.len() + 4)
            .map_err(|_| StickerImageError)?
            .to_le_bytes(),
    );
    file.extend_from_slice(b"WEBP");
    file.extend_from_slice(&body);
    Ok(file)
}

fn frames_of<'a>(
    decoder: impl AnimationDecoder<'a>,
    width: u32,
    height: u32,
) -> Result<Vec<(RgbaImage, u32)>, StickerImageError> {
    let per_frame = u64::from(width) * u64::from(height);
    let limit = usize::try_from(MAX_DECODED_PIXELS / per_frame.max(1)).unwrap_or(0);
    let mut frames = Vec::new();
    for frame in decoder.into_frames() {
        let frame = frame.map_err(|_| StickerImageError)?;
        if frames.len() >= limit {
            return Err(StickerImageError);
        }
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay = numerator / denominator.max(1);
        frames.push((resized(frame.into_buffer()), delay));
    }
    Ok(frames)
}

/// Приводит картинку к виду стикера (`encodeSticker`).
///
/// # Errors
/// Файл пуст, слишком велик, не PNG/JPEG/WebP/GIF или не разбирается.
pub fn encode_sticker(bytes: &[u8]) -> Result<Vec<u8>, StickerImageError> {
    if bytes.is_empty() || bytes.len() > MAX_STICKER_BYTES {
        return Err(StickerImageError);
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| StickerImageError)?;
    let format = reader.format().ok_or(StickerImageError)?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif
    ) {
        return Err(StickerImageError);
    }
    let (width, height) = reader.into_dimensions().map_err(|_| StickerImageError)?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_DECODED_PIXELS {
        return Err(StickerImageError);
    }
    let frames = match format {
        ImageFormat::Gif => frames_of(
            GifDecoder::new(Cursor::new(bytes)).map_err(|_| StickerImageError)?,
            width,
            height,
        )?,
        ImageFormat::WebP => {
            let decoder = WebPDecoder::new(Cursor::new(bytes)).map_err(|_| StickerImageError)?;
            if decoder.has_animation() {
                frames_of(decoder, width, height)?
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    };
    let encoded = if frames.len() > 1 {
        animated_webp(&frames)?
    } else {
        let mut decoder = ImageReader::with_format(Cursor::new(bytes), format)
            .into_decoder()
            .map_err(|_| StickerImageError)?;
        let orientation = decoder.orientation().map_err(|_| StickerImageError)?;
        let mut image = DynamicImage::from_decoder(decoder).map_err(|_| StickerImageError)?;
        image.apply_orientation(orientation);
        let frame = resized(image.to_rgba8());
        let mut file = Vec::new();
        WebPEncoder::new_lossless(&mut file)
            .encode(
                frame.as_raw(),
                frame.width(),
                frame.height(),
                ExtendedColorType::Rgba8,
            )
            .map_err(|_| StickerImageError)?;
        file
    };
    if encoded.len() > MAX_STICKER_BYTES {
        return Err(StickerImageError);
    }
    Ok(encoded)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "в тестах сбой подготовки — это провал теста"
)]
mod tests {
    use super::*;
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame, Rgba};

    fn encoded(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, format).expect("кодирование");
        out.into_inner()
    }

    fn solid(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(width, height, Rgba(color))
    }

    #[test]
    fn large_pictures_are_fitted_into_the_square() {
        let png = encoded(
            &DynamicImage::ImageRgba8(solid(1024, 256, [10, 20, 30, 255])),
            ImageFormat::Png,
        );
        let sticker = encode_sticker(&png).expect("стикер");
        let reader = ImageReader::new(Cursor::new(&sticker))
            .with_guessed_format()
            .expect("формат");
        assert_eq!(reader.format(), Some(ImageFormat::WebP));
        assert_eq!(reader.into_dimensions().expect("размер"), (512, 128));
        let small = encoded(
            &DynamicImage::ImageRgb8(image::RgbImage::new(40, 30)),
            ImageFormat::Jpeg,
        );
        let sticker = encode_sticker(&small).expect("стикер");
        assert_eq!(
            ImageReader::new(Cursor::new(&sticker))
                .with_guessed_format()
                .expect("формат")
                .into_dimensions()
                .expect("размер"),
            (40, 30)
        );
    }

    #[test]
    fn animation_survives() {
        let mut gif = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut gif);
            for color in [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]] {
                let frame = Frame::from_parts(
                    solid(600, 300, color),
                    0,
                    0,
                    Delay::from_numer_denom_ms(80, 1),
                );
                encoder.encode_frame(frame).expect("кадр");
            }
        }
        let sticker = encode_sticker(&gif).expect("стикер");
        let decoder = WebPDecoder::new(Cursor::new(&sticker)).expect("webp");
        assert!(decoder.has_animation());
        assert_eq!(decoder.dimensions(), (512, 256));
        let frames: Vec<_> = decoder
            .into_frames()
            .collect::<Result<_, _>>()
            .expect("кадры");
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[1].delay().numer_denom_ms(), (80, 1));
        assert_eq!(frames[2].buffer().get_pixel(5, 5).0, [0, 0, 255, 255]);
    }

    #[test]
    fn foreign_and_broken_files_are_refused() {
        assert_eq!(encode_sticker(b""), Err(StickerImageError));
        assert_eq!(
            encode_sticker(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
            Err(StickerImageError)
        );
        assert_eq!(
            encode_sticker(&[0x89, b'P', b'N', b'G', 1, 2, 3]),
            Err(StickerImageError)
        );
        let bmp = encoded(
            &DynamicImage::ImageRgb8(image::RgbImage::new(4, 4)),
            ImageFormat::Png,
        );
        assert!(encode_sticker(&bmp[..bmp.len() / 2]).is_err());
        assert_eq!(
            encode_sticker(&vec![0u8; MAX_STICKER_BYTES + 1]),
            Err(StickerImageError)
        );
    }
}
