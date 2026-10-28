//! Конвертация кадров.
//!
//! xcap отдаёт RGBA8, а `NativeVideoSource` ждёт I420 (YUV 4:2:0).
//! Конвертация на CPU: для 1080p@30 это ~2 мс на кадр.

use livekit::webrtc::prelude::*;
use livekit::webrtc::video_frame::I420Buffer;

use crate::error::{VoiceError, VoiceResult};

const Y_R: f32 = 0.257;
const Y_G: f32 = 0.504;
const Y_B: f32 = 0.098;
const Y_OFFSET: f32 = 16.0;

const U_R: f32 = -0.148;
const U_G: f32 = -0.291;
const U_B: f32 = 0.439;
const UV_OFFSET: f32 = 128.0;

const V_R: f32 = 0.439;
const V_G: f32 = -0.368;
const V_B: f32 = -0.071;

fn validate_rgba(len: usize, width: u32, height: u32) -> VoiceResult<()> {
    if width == 0 || height == 0 {
        return Err(VoiceError::InvalidFrameSize { width, height });
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or(VoiceError::InvalidFrameSize { width, height })?;
    if len != expected {
        return Err(VoiceError::Capture(format!(
            "ожидалось {expected} байт RGBA, получено {len}"
        )));
    }
    Ok(())
}

fn to_u8(value: f32) -> u8 {
    value.clamp(0.0, 255.0).round() as u8
}

/// Конвертирует RGBA8 в I420. Альфа игнорируется.
///
/// # Errors
///
/// Ошибка, если размеры нулевые или длина буфера не равна `width*height*4`.
pub fn rgba_to_i420(rgba: &[u8], width: u32, height: u32) -> VoiceResult<I420Buffer> {
    validate_rgba(rgba.len(), width, height)?;

    let mut buffer = I420Buffer::new(width, height);
    let chroma_width = buffer.chroma_width();
    let chroma_height = buffer.chroma_height();
    let (stride_y, stride_u, stride_v) = buffer.strides();
    let (data_y, data_u, data_v) = buffer.data_mut();
    let image_width = width as usize;
    let image_height = height as usize;

    for row_y in 0..image_height {
        let row_offset = row_y * image_width * 4;
        let luma_row_offset = row_y * stride_y as usize;
        for column_x in 0..image_width {
            let pixel_index = row_offset + column_x * 4;
            let red = f32::from(rgba[pixel_index]);
            let green = f32::from(rgba[pixel_index + 1]);
            let blue = f32::from(rgba[pixel_index + 2]);
            data_y[luma_row_offset + column_x] =
                to_u8(Y_R.mul_add(red, Y_G.mul_add(green, Y_B * blue)) + Y_OFFSET);
        }
    }

    let chroma_width_usize = chroma_width as usize;
    let chroma_height_usize = chroma_height as usize;
    for chroma_y in 0..chroma_height_usize {
        let u_row_offset = chroma_y * stride_u as usize;
        let v_row_offset = chroma_y * stride_v as usize;
        for chroma_x in 0..chroma_width_usize {
            let base_x = chroma_x * 2;
            let base_y = chroma_y * 2;
            let (mut sum_u, mut sum_v) = (0.0_f32, 0.0_f32);
            let mut samples = 0.0_f32;
            for offset_y in 0..2 {
                for offset_x in 0..2 {
                    let pixel_x = base_x + offset_x;
                    let pixel_y = base_y + offset_y;
                    if pixel_x >= image_width || pixel_y >= image_height {
                        continue;
                    }
                    let pixel_index = (pixel_y * image_width + pixel_x) * 4;
                    let red = f32::from(rgba[pixel_index]);
                    let green = f32::from(rgba[pixel_index + 1]);
                    let blue = f32::from(rgba[pixel_index + 2]);
                    sum_u += U_R.mul_add(red, U_G.mul_add(green, U_B * blue));
                    sum_v += V_R.mul_add(red, V_G.mul_add(green, V_B * blue));
                    samples += 1.0;
                }
            }
            data_u[u_row_offset + chroma_x] = to_u8(sum_u / samples + UV_OFFSET);
            data_v[v_row_offset + chroma_x] = to_u8(sum_v / samples + UV_OFFSET);
        }
    }

    Ok(buffer)
}

/// Оборачивает I420-буфер во фрейм с нулевой ориентацией.
#[must_use]
pub fn frame_from_i420(buffer: I420Buffer) -> VideoFrame<I420Buffer> {
    VideoFrame::new(VideoRotation::VideoRotation0, buffer)
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
    }

    #[test]
    fn white_maps_to_luma_235() {
        let rgba = vec![255_u8; 2 * 2 * 4];
        let buffer = rgba_to_i420(&rgba, 2, 2).unwrap();
        let (y, u, v) = buffer.data();
        assert!(y.iter().all(|&b| b == 235), "Y={y:?}");
        assert!(u.iter().all(|&b| b == 128), "U={u:?}");
        assert!(v.iter().all(|&b| b == 128), "V={v:?}");
    }

    #[test]
    fn odd_width_chroma_does_not_sample_next_row() {
        // Красная первая строка и синяя вторая. Крайний блок 2x2
        // включает красный пиксель над синим, но не должен захватить
        // дополнительный синий пиксель из начала следующей строки.
        let mut rgba = Vec::new();
        for _ in 0..3 {
            rgba.extend_from_slice(&[255, 0, 0, 255]);
        }
        for _ in 0..3 {
            rgba.extend_from_slice(&[0, 0, 255, 255]);
        }
        let buffer = rgba_to_i420(&rgba, 3, 2).unwrap();
        let (_, u, v) = buffer.data();
        // Один красный + один синий дают U около 165; лишний синий
        // из-за перехода через строку поднял бы U примерно до 202.
        assert!((150..180).contains(&u[1]), "U крайнего блока: {u:?}");
        assert!((160..190).contains(&v[1]), "V крайнего блока: {v:?}");
    }

    #[test]
    fn i420_white_becomes_white_rgba() {
        let mut buffer = I420Buffer::new(4, 4);
        {
            let (y, u, v) = buffer.data_mut();
            y.fill(235);
            u.fill(128);
            v.fill(128);
        }
        let frame = buffer_to_rgba(&buffer).unwrap();
        assert_eq!(frame.width, 4);
        assert_eq!(frame.height, 4);
        assert_eq!(frame.len(), 4 * 4 * 4);
        assert!(
            frame
                .rgba
                .chunks_exact(4)
                .all(|p| p == [255, 255, 255, 255]),
            "{frame:?}"
        );
    }

    #[test]
    fn nv12_black_is_opaque_black() {
        let mut buffer = NV12Buffer::new(2, 2);
        {
            let (y, uv) = buffer.data_mut();
            y.fill(16);
            uv.fill(128);
        }
        let frame = nv12_to_rgba(&buffer);
        assert_eq!(frame.rgba, [0, 0, 0, 255].repeat(4), "{frame:?}");
    }

    #[test]
    fn black_maps_to_luma_16() {
        let rgba = vec![0_u8; 2 * 2 * 4];
        let buffer = rgba_to_i420(&rgba, 2, 2).unwrap();
        let (y, _, _) = buffer.data();
        assert!(y.iter().all(|&b| b == 16), "Y={y:?}");
    }
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

fn clamp_u8(value: f32) -> u8 {
    value.clamp(0.0, 255.0).round() as u8
}

/// Обратное BT.601 (limited range) преобразование одной точки.
fn yuv_pixel(luma: u8, u_sample: u8, v_sample: u8) -> [u8; 3] {
    let luma_offset = f32::from(luma) - Y_OFFSET;
    let blue_difference = f32::from(u_sample) - UV_OFFSET;
    let red_difference = f32::from(v_sample) - UV_OFFSET;
    let red = 1.164_f32.mul_add(luma_offset, 1.596 * red_difference);
    let green = 1.164_f32.mul_add(
        luma_offset,
        (-0.392_f32).mul_add(blue_difference, -0.813 * red_difference),
    );
    let blue = 1.164_f32.mul_add(luma_offset, 2.017 * blue_difference);
    [clamp_u8(red), clamp_u8(green), clamp_u8(blue)]
}

fn write_pixel(rgba: &mut [u8], index: usize, rgb: [u8; 3]) {
    let i = index * 4;
    rgba[i] = rgb[0];
    rgba[i + 1] = rgb[1];
    rgba[i + 2] = rgb[2];
    rgba[i + 3] = 255;
}

/// Конвертирует I420 (то, что отдаёт декодер libwebrtc) в RGBA8.
#[must_use]
pub fn i420_to_rgba(buffer: &I420Buffer) -> RgbaFrame {
    let width = buffer.width();
    let height = buffer.height();
    let (stride_y, stride_u, stride_v) = buffer.strides();
    let (data_y, data_u, data_v) = buffer.data();
    let (sy, su, sv) = (stride_y as usize, stride_u as usize, stride_v as usize);
    let (w, h) = (width as usize, height as usize);

    let mut rgba = vec![0_u8; w * h * 4];
    for y in 0..h {
        let luma_row = y * sy;
        let chroma_row_u = (y / 2) * su;
        let chroma_row_v = (y / 2) * sv;
        for x in 0..w {
            let luma = data_y.get(luma_row + x).copied().unwrap_or(16);
            let u = data_u.get(chroma_row_u + x / 2).copied().unwrap_or(128);
            let v = data_v.get(chroma_row_v + x / 2).copied().unwrap_or(128);
            write_pixel(&mut rgba, y * w + x, yuv_pixel(luma, u, v));
        }
    }
    RgbaFrame {
        width,
        height,
        rgba,
    }
}

/// Конвертирует I444 (хрома без прореживания) в RGBA8.
#[must_use]
pub fn i444_to_rgba(buffer: &I444Buffer) -> RgbaFrame {
    let width = buffer.width();
    let height = buffer.height();
    let (stride_y, stride_u, stride_v) = buffer.strides();
    let (data_y, data_u, data_v) = buffer.data();
    let (sy, su, sv) = (stride_y as usize, stride_u as usize, stride_v as usize);
    let (w, h) = (width as usize, height as usize);

    let mut rgba = vec![0_u8; w * h * 4];
    for y in 0..h {
        let row = y * sy;
        let row_u = y * su;
        let row_v = y * sv;
        for x in 0..w {
            let luma = data_y.get(row + x).copied().unwrap_or(16);
            let u = data_u.get(row_u + x).copied().unwrap_or(128);
            let v = data_v.get(row_v + x).copied().unwrap_or(128);
            write_pixel(&mut rgba, y * w + x, yuv_pixel(luma, u, v));
        }
    }
    RgbaFrame {
        width,
        height,
        rgba,
    }
}

/// Конвертирует NV12 (U и V вперемешку) в RGBA8.
#[must_use]
pub fn nv12_to_rgba(buffer: &NV12Buffer) -> RgbaFrame {
    let width = buffer.width();
    let height = buffer.height();
    let (stride_y, stride_uv) = buffer.strides();
    let (data_y, data_uv) = buffer.data();
    let (sy, suv) = (stride_y as usize, stride_uv as usize);
    let (w, h) = (width as usize, height as usize);

    let mut rgba = vec![0_u8; w * h * 4];
    for y in 0..h {
        let luma_row = y * sy;
        let chroma_row = (y / 2) * suv;
        for x in 0..w {
            let luma = data_y.get(luma_row + x).copied().unwrap_or(16);
            let uv = chroma_row + (x / 2) * 2;
            let u = data_uv.get(uv).copied().unwrap_or(128);
            let v = data_uv.get(uv + 1).copied().unwrap_or(128);
            write_pixel(&mut rgba, y * w + x, yuv_pixel(luma, u, v));
        }
    }
    RgbaFrame {
        width,
        height,
        rgba,
    }
}

/// Конвертирует любой поддержанный формат кадра в RGBA8.
///
/// # Errors
///
/// Формат кадра не поддержан (например, 10-битный I010).
pub fn buffer_to_rgba(buffer: &dyn VideoBuffer) -> VoiceResult<RgbaFrame> {
    if let Some(b) = buffer.as_i420() {
        return Ok(i420_to_rgba(b));
    }
    if let Some(b) = buffer.as_nv12() {
        return Ok(nv12_to_rgba(b));
    }
    if let Some(b) = buffer.as_i444() {
        return Ok(i444_to_rgba(b));
    }
    Err(VoiceError::Video(format!(
        "формат кадра не поддержан: {:?}",
        buffer.buffer_type()
    )))
}
