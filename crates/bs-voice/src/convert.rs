//! Конвертация кадров через libyuv (SIMD, из состава libwebrtc).
//!
//! - отправка: RGBA (камера) или BGRA (захват экрана) → I420 для
//!   `NativeVideoSource`, с уменьшением под предел размера;
//! - приём: любой буфер декодера → RGBA для текстуры egui, тоже с
//!   уменьшением, чтобы не гонять в видеопамять 4K там, где плитка 640×360.
//!
//! Скалярный код на f32 тратил на 1080p десятки миллисекунд на кадр —
//! отсюда и «убогая» частота кадров демонстрации.

use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::prelude::*;
use livekit::webrtc::video_frame::I420Buffer;
use livekit::webrtc::video_frame::native::VideoFrameBufferExt as _;

use crate::error::{VoiceError, VoiceResult};

/// Проверяет, что в буфере хватает байт под кадр `width`×`height` со строкой `stride`.
fn validate_packed(len: usize, stride: u32, width: u32, height: u32) -> VoiceResult<()> {
    if width == 0 || height == 0 {
        return Err(VoiceError::InvalidFrameSize { width, height });
    }
    if (stride as usize) < (width as usize) * 4 {
        return Err(VoiceError::Capture(format!(
            "строка {stride} байт короче ширины {width}"
        )));
    }
    let expected = (stride as usize)
        .checked_mul(height as usize)
        .ok_or(VoiceError::InvalidFrameSize { width, height })?;
    if len < expected {
        return Err(VoiceError::Capture(format!(
            "ожидалось {expected} байт, получено {len}"
        )));
    }
    Ok(())
}

fn signed(value: u32) -> VoiceResult<i32> {
    i32::try_from(value).map_err(|_| VoiceError::Capture("слишком большой кадр".into()))
}

/// Пиксельный порядок упакованного кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackedFormat {
    /// R, G, B, A в памяти (egui, камера).
    Rgba,
    /// B, G, R, A в памяти (кадры захвата экрана WebRTC).
    Bgra,
}

/// Упакованный кадр → I420.
///
/// # Errors
///
/// Нулевой размер или буфер меньше `stride * height`.
pub fn packed_to_i420(
    data: &[u8],
    stride: u32,
    width: u32,
    height: u32,
    format: PackedFormat,
) -> VoiceResult<I420Buffer> {
    validate_packed(data.len(), stride, width, height)?;
    let (signed_width, signed_height) = (signed(width)?, signed(height)?);
    let mut buffer = I420Buffer::new(width, height);
    let (stride_y, stride_u, stride_v) = buffer.strides();
    let (plane_y, plane_u, plane_v) = buffer.data_mut();
    // В терминах libyuv «ABGR» — это R,G,B,A в памяти, «ARGB» — B,G,R,A.
    let convert = match format {
        PackedFormat::Rgba => yuv_helper::abgr_to_i420,
        PackedFormat::Bgra => yuv_helper::argb_to_i420,
    };
    convert(
        data,
        stride,
        plane_y,
        stride_y,
        plane_u,
        stride_u,
        plane_v,
        stride_v,
        signed_width,
        signed_height,
    );
    Ok(buffer)
}

/// RGBA8 без выравнивания строк → I420.
///
/// # Errors
///
/// Нулевой размер или длина буфера не равна `width*height*4`.
pub fn rgba_to_i420(rgba: &[u8], width: u32, height: u32) -> VoiceResult<I420Buffer> {
    let expected = (width as usize) * (height as usize) * 4;
    if width > 0 && height > 0 && rgba.len() != expected {
        return Err(VoiceError::Capture(format!(
            "ожидалось {expected} байт RGBA, получено {}",
            rgba.len()
        )));
    }
    packed_to_i420(
        rgba,
        width.saturating_mul(4),
        width,
        height,
        PackedFormat::Rgba,
    )
}

/// Оборачивает I420-буфер во фрейм с нулевой ориентацией.
#[must_use]
pub fn frame_from_i420(buffer: I420Buffer) -> VideoFrame<I420Buffer> {
    VideoFrame::new(VideoRotation::VideoRotation0, buffer)
}

/// Размер, вписанный в `max_width`×`max_height` с сохранением пропорций,
/// чётный (I420) и не больше исходного.
#[must_use]
pub fn fit_size(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (0, 0);
    }
    let scale = (f64::from(max_width.max(2)) / f64::from(width))
        .min(f64::from(max_height.max(2)) / f64::from(height))
        .min(1.0);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "размер кадра"
    )]
    let scaled = |value: u32| ((f64::from(value) * scale).round() as u32).max(2) & !1;
    (scaled(width), scaled(height))
}

/// Уменьшает I420 под предел, если нужно.
#[must_use]
pub fn fit_i420(mut buffer: I420Buffer, max_width: u32, max_height: u32) -> I420Buffer {
    let (width, height) = (buffer.width(), buffer.height());
    let (tw, th) = fit_size(width, height, max_width, max_height);
    if (tw, th) == (width, height) || tw == 0 {
        return buffer;
    }
    #[allow(
        clippy::cast_possible_wrap,
        reason = "размер кадра заведомо меньше i32::MAX"
    )]
    buffer.scale(tw as i32, th as i32)
}

// ── приём: YUV → RGBA ────────────────────────────────────────────────────

/// Кадр в RGBA8, готовый к загрузке в текстуру (egui принимает его как есть).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaFrame {
    /// Ширина в пикселях.
    pub width: u32,
    /// Высота в пикселях.
    pub height: u32,
    /// Пиксели RGBA8, построчно, без выравнивания строк.
    pub rgba: Vec<u8>,
}

impl RgbaFrame {
    /// Число байт в буфере.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rgba.len()
    }

    /// Пуст ли буфер.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rgba.is_empty()
    }
}

/// I420 → RGBA8.
#[must_use]
pub fn i420_to_rgba(buffer: &I420Buffer) -> RgbaFrame {
    let (width, height) = (buffer.width(), buffer.height());
    let mut rgba = vec![0_u8; (width as usize) * (height as usize) * 4];
    if width > 0 && height > 0 {
        #[allow(
            clippy::cast_possible_wrap,
            reason = "размер кадра заведомо меньше i32::MAX"
        )]
        buffer.to_argb(
            VideoFormatType::ABGR,
            &mut rgba,
            width * 4,
            width as i32,
            height as i32,
        );
    }
    RgbaFrame {
        width,
        height,
        rgba,
    }
}

/// Любой буфер декодера → RGBA8, уменьшенный до `max_width`×`max_height`.
///
/// # Errors
///
/// Пустой кадр.
pub fn buffer_to_rgba_fit(
    buffer: &dyn VideoBuffer,
    max_width: u32,
    max_height: u32,
) -> VoiceResult<RgbaFrame> {
    let (width, height) = (buffer.width(), buffer.height());
    if width == 0 || height == 0 {
        return Err(VoiceError::InvalidFrameSize { width, height });
    }
    let i420 = fit_i420(buffer.to_i420(), max_width, max_height);
    Ok(i420_to_rgba(&i420))
}

/// Любой буфер декодера → RGBA8 в исходном размере.
///
/// # Errors
///
/// Пустой кадр.
pub fn buffer_to_rgba(buffer: &dyn VideoBuffer) -> VoiceResult<RgbaFrame> {
    buffer_to_rgba_fit(buffer, u32::MAX / 2, u32::MAX / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_size() {
        assert!(matches!(
            rgba_to_i420(&[], 0, 0),
            Err(VoiceError::InvalidFrameSize { .. })
        ));
    }

    #[test]
    fn rejects_wrong_buffer_len() {
        assert!(matches!(
            rgba_to_i420(&[0; 10], 4, 4),
            Err(VoiceError::Capture(_))
        ));
        assert!(packed_to_i420(&[0; 10], 16, 4, 4, PackedFormat::Bgra).is_err());
    }

    #[test]
    fn white_and_black_map_to_studio_range() {
        let white = rgba_to_i420(&[255_u8; 16], 2, 2).unwrap();
        let (y, u, v) = white.data();
        assert!(y.iter().all(|&b| b == 235), "Y={y:?}");
        assert!(u.iter().all(|&b| b == 128) && v.iter().all(|&b| b == 128));
        let black = rgba_to_i420(&[0, 0, 0, 255].repeat(4), 2, 2).unwrap();
        assert!(black.data().0.iter().all(|&b| b == 16));
    }

    fn roundtrip(pixel: [u8; 4], format: PackedFormat) -> [u8; 4] {
        let data = pixel.repeat(16);
        let i420 = packed_to_i420(&data, 16, 4, 4, format).unwrap();
        let rgba = i420_to_rgba(&i420);
        [rgba.rgba[0], rgba.rgba[1], rgba.rgba[2], rgba.rgba[3]]
    }

    #[test]
    fn channel_order_survives_roundtrip() {
        let red = roundtrip([255, 0, 0, 255], PackedFormat::Rgba);
        assert!(red[0] > 200 && red[1] < 40 && red[2] < 40, "{red:?}");
        // Тот же красный в BGRA лежит как 0,0,255.
        let red = roundtrip([0, 0, 255, 255], PackedFormat::Bgra);
        assert!(red[0] > 200 && red[1] < 40 && red[2] < 40, "{red:?}");
        assert_eq!(red[3], 255);
    }

    #[test]
    fn padded_rows_are_respected() {
        // Строка 4 пикселя = 16 байт, но шаг 24: хвост строки — мусор.
        let mut data = Vec::new();
        for _ in 0..2 {
            data.extend([0, 0, 255, 255].repeat(4));
            data.extend([0, 255, 0, 255].repeat(2));
        }
        let i420 = packed_to_i420(&data, 24, 4, 2, PackedFormat::Bgra).unwrap();
        let rgba = i420_to_rgba(&i420);
        assert!(
            rgba.rgba.chunks_exact(4).all(|p| p[0] > 200 && p[1] < 40),
            "{rgba:?}"
        );
    }

    #[test]
    fn fit_keeps_aspect_and_never_upscales() {
        assert_eq!(fit_size(3840, 2160, 1920, 1080), (1920, 1080));
        assert_eq!(fit_size(2560, 1440, 1280, 720), (1280, 720));
        assert_eq!(fit_size(1280, 1024, 1920, 720), (900, 720));
        assert_eq!(fit_size(800, 600, 1920, 1080), (800, 600));
        assert_eq!(fit_size(801, 601, 1920, 1080), (800, 600));
        assert_eq!(fit_size(0, 600, 1920, 1080), (0, 0));
    }

    #[test]
    fn decoded_frames_are_downscaled_for_preview() {
        let mut buffer = I420Buffer::new(1280, 720);
        {
            let (y, u, v) = buffer.data_mut();
            y.fill(235);
            u.fill(128);
            v.fill(128);
        }
        let frame = buffer_to_rgba_fit(&buffer, 640, 640).unwrap();
        assert_eq!((frame.width, frame.height), (640, 360));
        assert!(
            frame
                .rgba
                .chunks_exact(4)
                .all(|p| p == [255, 255, 255, 255])
        );
    }
}
