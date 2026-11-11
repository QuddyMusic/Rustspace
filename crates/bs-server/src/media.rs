//! Обработка загруженных файлов: размеры картинок профиля, миниатюры,
//! сведения о видео и звуке (`utils/thumbnail.ts`, `utils/mediaPlayable.ts`).
//!
//! Картинки разбираются крейтом `image` (JPEG, PNG, GIF, WebP, TIFF). Для
//! видео и звука вызываются `ffprobe` и `ffmpeg`, если они установлены; без
//! них файл принимается без длительности и миниатюры — как у оригинала.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::imageops::FilterType;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use tokio::process::Command;

use crate::files::ProfileImage;

/// Наибольшая ширина миниатюры.
const THUMBNAIL_MAX_WIDTH: u32 = 800;
/// Предел на внешнюю программу.
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(10);
/// Кадр видео больше этого не разбирается.
const MAX_FRAME_BYTES: usize = 50 * 1024 * 1024;
/// Картинки больше этого числа точек не декодируются: защита от «бомб».
const MAX_PIXELS: u64 = 100_000_000;

/// Что удалось узнать о файле.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Processed {
    /// Имя файла миниатюры в каталоге загрузок.
    pub thumbnail: Option<String>,
    /// Ширина.
    pub width: Option<i64>,
    /// Высота.
    pub height: Option<i64>,
    /// Длительность в секундах.
    pub duration: Option<f64>,
    /// Сможет ли браузер воспроизвести видео; `None` — неизвестно.
    pub playable: Option<bool>,
}

/// Типы, из которых делается миниатюра (`RESIZABLE_MIMETYPES`).
#[must_use]
pub fn is_resizable_image(mimetype: &str) -> bool {
    matches!(
        mimetype,
        "image/jpeg" | "image/png" | "image/webp" | "image/gif" | "image/avif" | "image/tiff"
    )
}

/// Воспроизводимость видео по контейнеру и кодеку (`classifyVideoPlayable`).
#[must_use]
pub fn classify_video_playable(mimetype: &str, codec: Option<&str>) -> Option<bool> {
    const UNPLAYABLE: [&str; 15] = [
        "hevc",
        "hvc1",
        "hev1",
        "h265",
        "prores",
        "wmv1",
        "wmv2",
        "wmv3",
        "vc1",
        "mpeg1video",
        "mpeg2video",
        "dnxhd",
        "mjpeg",
        "vp6",
        "vp6f",
    ];
    const PLAYABLE: [&str; 7] = ["h264", "avc1", "vp8", "vp9", "av1", "av01", "theora"];
    if !mimetype.starts_with("video/") {
        return None;
    }
    let codec = codec.filter(|codec| !codec.is_empty())?.to_lowercase();
    if UNPLAYABLE.contains(&codec.as_str()) {
        return Some(false);
    }
    if matches!(mimetype, "video/mp4" | "video/webm" | "video/ogg")
        && PLAYABLE.contains(&codec.as_str())
    {
        return Some(true);
    }
    None
}

fn reader(path: &Path) -> Option<ImageReader<std::io::BufReader<std::fs::File>>> {
    ImageReader::open(path).ok()?.with_guessed_format().ok()
}

/// Сколько кадров в картинке: больше одного — анимация.
fn is_animated(path: &Path, format: ImageFormat) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let file = std::io::BufReader::new(file);
    match format {
        ImageFormat::Gif => GifDecoder::new(file)
            .is_ok_and(|decoder| decoder.into_frames().take(2).flatten().count() > 1),
        ImageFormat::WebP => WebPDecoder::new(file).is_ok_and(|decoder| decoder.has_animation()),
        _ => false,
    }
}

fn decode(path: &Path) -> Option<(DynamicImage, ImageFormat)> {
    let reader = reader(path)?;
    let format = reader.format()?;
    let decoder = reader.into_decoder().ok()?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return None;
    }
    Some((DynamicImage::from_decoder(decoder).ok()?, format))
}

fn scaled_height(width: u32, height: u32, target: u32) -> u32 {
    let scaled = (f64::from(height) * f64::from(target) / f64::from(width)).round();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "высота меньше исходной"
    )]
    let scaled = scaled as u32;
    scaled.max(1)
}

fn write_webp(image: &DynamicImage, target: &Path) -> bool {
    let (width, height) = (image.width(), image.height());
    let small = if width > THUMBNAIL_MAX_WIDTH {
        image.resize_exact(
            THUMBNAIL_MAX_WIDTH,
            scaled_height(width, height, THUMBNAIL_MAX_WIDTH),
            FilterType::Triangle,
        )
    } else {
        image.clone()
    };
    // Кодировщик WebP в `image` пишет без потерь и не знает 16-битных точек.
    let small = if small.color().has_alpha() {
        DynamicImage::ImageRgba8(small.to_rgba8())
    } else {
        DynamicImage::ImageRgb8(small.to_rgb8())
    };
    let saved = small.save_with_format(target, ImageFormat::WebP).is_ok();
    if !saved {
        let _ = std::fs::remove_file(target);
    }
    saved
}

/// Размеры картинки (`probeImageDimensions`).
fn image_dimensions(path: &Path) -> Option<(u32, u32)> {
    reader(path)?
        .into_dimensions()
        .ok()
        .filter(|(width, height)| *width > 0 && *height > 0)
}

/// Миниатюра для широкой неподвижной картинки (`generateThumbnail`).
fn image_thumbnail(path: &Path, target: &Path) -> bool {
    let Some((width, _)) = image_dimensions(path) else {
        return false;
    };
    if width <= THUMBNAIL_MAX_WIDTH {
        return false;
    }
    let Some(format) = reader(path).and_then(|reader| reader.format()) else {
        return false;
    };
    if is_animated(path, format) {
        return false;
    }
    decode(path).is_some_and(|(image, _)| write_webp(&image, target))
}

async fn run(program: &str, args: &[&str], path: &Path) -> Option<Vec<u8>> {
    let child = Command::new(program)
        .args(args)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let output = tokio::time::timeout(FFMPEG_TIMEOUT, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Установлен ли `ffprobe` (`checkFfmpeg`); проверяется один раз.
async fn ffmpeg_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    if let Some(known) = AVAILABLE.get() {
        return *known;
    }
    let found = Command::new("ffprobe")
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status();
    let found = tokio::time::timeout(Duration::from_secs(5), found)
        .await
        .is_ok_and(|status| status.is_ok_and(|s| s.success()));
    if !found {
        tracing::warn!("ffmpeg/ffprobe не найдены: миниатюры видео и длительность не определяются");
    }
    *AVAILABLE.get_or_init(|| found)
}

#[derive(Debug, Default)]
struct MediaMeta {
    width: Option<i64>,
    height: Option<i64>,
    duration: Option<f64>,
    codec: Option<String>,
}

/// Сведения о видео или звуке через `ffprobe` (`probeMediaMeta`).
async fn probe_media(path: &Path, video: bool) -> Option<MediaMeta> {
    if !ffmpeg_available().await {
        return None;
    }
    let mut meta = MediaMeta::default();
    if video {
        let args = [
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,codec_name",
            "-of",
            "json",
            "-i",
        ];
        if let Some(out) = run("ffprobe", &args, path).await
            && let Ok(data) = serde_json::from_slice::<serde_json::Value>(&out)
        {
            let stream = &data["streams"][0];
            if let (Some(width), Some(height)) =
                (stream["width"].as_i64(), stream["height"].as_i64())
                && width > 0
                && height > 0
            {
                meta.width = Some(width);
                meta.height = Some(height);
            }
            meta.codec = stream["codec_name"]
                .as_str()
                .filter(|codec| !codec.is_empty())
                .map(str::to_owned);
        }
    }
    let args = [
        "-v",
        "error",
        "-show_entries",
        "format=duration",
        "-of",
        "json",
        "-i",
    ];
    if let Some(out) = run("ffprobe", &args, path).await
        && let Ok(data) = serde_json::from_slice::<serde_json::Value>(&out)
        && let Some(duration) = data["format"]["duration"]
            .as_str()
            .and_then(|text| text.trim().parse::<f64>().ok())
        && duration.is_finite()
    {
        meta.duration = Some((duration * 100.0).round() / 100.0);
    }
    (meta.width.is_some() || meta.duration.is_some() || meta.codec.is_some()).then_some(meta)
}

/// Кадр видео как миниатюра (`generateVideoThumbnail`): размеры кадра.
async fn video_thumbnail(path: &Path, target: PathBuf) -> Option<(i64, i64)> {
    if !ffmpeg_available().await {
        return None;
    }
    let mut frame = None;
    for seek in ["1", "0"] {
        let child = Command::new("ffmpeg")
            .args(["-ss", seek, "-i"])
            .arg(path)
            .args(["-frames:v", "1", "-f", "image2pipe", "-vcodec", "png", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        if let Ok(Ok(output)) = tokio::time::timeout(FFMPEG_TIMEOUT, child.wait_with_output()).await
            && output.status.success()
            && !output.stdout.is_empty()
            && output.stdout.len() <= MAX_FRAME_BYTES
        {
            frame = Some(output.stdout);
            break;
        }
    }
    let frame = frame?;
    tokio::task::spawn_blocking(move || {
        let image = ImageReader::with_format(Cursor::new(frame), ImageFormat::Png)
            .decode()
            .ok()?;
        let size = (i64::from(image.width()), i64::from(image.height()));
        write_webp(&image, &target).then_some(size)
    })
    .await
    .ok()
    .flatten()
}

/// Обрабатывает принятый файл: миниатюра `thumb_name` в каталоге загрузок,
/// размеры, длительность. Ошибки не мешают загрузке.
pub async fn process(
    source: &Path,
    mimetype: &str,
    upload_dir: &Path,
    thumb_name: &str,
) -> Processed {
    let target = upload_dir.join(thumb_name);
    let mut result = Processed::default();
    if is_resizable_image(mimetype) {
        let (path, thumb) = (source.to_owned(), target.clone());
        let done = tokio::task::spawn_blocking(move || {
            (image_thumbnail(&path, &thumb), image_dimensions(&path))
        })
        .await;
        if let Ok((made, size)) = done {
            result.thumbnail = made.then(|| thumb_name.to_owned());
            if let Some((width, height)) = size {
                result.width = Some(i64::from(width));
                result.height = Some(i64::from(height));
            }
        }
    } else if mimetype.starts_with("video/") {
        if let Some((width, height)) = video_thumbnail(source, target).await {
            result.thumbnail = Some(thumb_name.to_owned());
            result.width = Some(width);
            result.height = Some(height);
        }
        if let Some(meta) = probe_media(source, true).await {
            result.duration = meta.duration;
            result.width = result.width.or(meta.width);
            result.height = result.height.or(meta.height);
            result.playable = classify_video_playable(mimetype, meta.codec.as_deref());
        }
    } else if mimetype.starts_with("audio/") {
        result.duration = probe_media(source, false)
            .await
            .and_then(|meta| meta.duration)
            .filter(|d| *d != 0.0);
    }
    result
}

fn resize_in_place(path: &Path, max_width: u32) {
    let Some((width, height)) = image_dimensions(path) else {
        return;
    };
    if width <= max_width {
        return;
    }
    let Some(format) = reader(path).and_then(|reader| reader.format()) else {
        return;
    };
    // Анимацию крейт `image` при пересборке потерял бы: такие файлы остаются как есть.
    if is_animated(path, format) || !format.writing_enabled() {
        return;
    }
    let Some((image, _)) = decode(path) else {
        return;
    };
    let small = image.resize_exact(
        max_width,
        scaled_height(width, height, max_width),
        FilterType::Lanczos3,
    );
    let small = match format {
        ImageFormat::WebP if small.color().has_alpha() => {
            DynamicImage::ImageRgba8(small.to_rgba8())
        }
        ImageFormat::Jpeg | ImageFormat::WebP => DynamicImage::ImageRgb8(small.to_rgb8()),
        _ => small,
    };
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    if small.save_with_format(&temp, format).is_ok() && std::fs::rename(&temp, path).is_ok() {
        return;
    }
    tracing::warn!(file = ?path.file_name(), "не удалось уменьшить картинку профиля");
    let _ = std::fs::remove_file(&temp);
}

/// Приводит файл картинки профиля к рабочему размеру на месте
/// (`resizeProfileImage`): аватар и значок — 256, обложка — 1280 по ширине.
pub async fn resize_profile_image(path: PathBuf, kind: ProfileImage) {
    let max_width = match kind {
        ProfileImage::Avatar | ProfileImage::Icon => 256,
        ProfileImage::Banner => 1280,
    };
    let _ = tokio::task::spawn_blocking(move || resize_in_place(&path, max_width)).await;
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "в тестах сбой подготовки — это провал теста"
)]
mod tests {
    use super::*;

    fn picture(dir: &Path, name: &str, width: u32, height: u32, format: ImageFormat) -> PathBuf {
        let path = dir.join(name);
        let image = DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
            #[allow(clippy::cast_possible_truncation, reason = "остаток от деления на 256")]
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        }));
        image
            .save_with_format(&path, format)
            .expect("картинка записана");
        path
    }

    #[tokio::test]
    async fn wide_pictures_get_a_thumbnail_and_narrow_ones_do_not() {
        let dir = tempfile::tempdir().expect("каталог");
        let wide = picture(dir.path(), "wide", 1600, 400, ImageFormat::Png);
        let done = process(&wide, "image/png", dir.path(), "1_thumb.webp").await;
        assert_eq!(done.thumbnail.as_deref(), Some("1_thumb.webp"));
        assert_eq!((done.width, done.height), (Some(1600), Some(400)));
        assert_eq!(
            image_dimensions(&dir.path().join("1_thumb.webp")),
            Some((800, 200))
        );

        let narrow = picture(dir.path(), "narrow", 800, 600, ImageFormat::Jpeg);
        let done = process(&narrow, "image/jpeg", dir.path(), "2_thumb.webp").await;
        assert_eq!(done.thumbnail, None);
        assert_eq!((done.width, done.height), (Some(800), Some(600)));
        assert!(!dir.path().join("2_thumb.webp").exists());
    }

    #[tokio::test]
    async fn broken_and_foreign_files_are_accepted_without_details() {
        let dir = tempfile::tempdir().expect("каталог");
        let path = dir.path().join("broken");
        std::fs::write(&path, b"not a picture").expect("файл");
        assert_eq!(
            process(&path, "image/png", dir.path(), "3_thumb.webp").await,
            Processed::default()
        );
        assert_eq!(
            process(&path, "application/pdf", dir.path(), "4_thumb.webp").await,
            Processed::default()
        );
    }

    #[tokio::test]
    async fn profile_pictures_are_shrunk_in_place() {
        let dir = tempfile::tempdir().expect("каталог");
        let avatar = picture(dir.path(), "a.png", 1024, 512, ImageFormat::Png);
        resize_profile_image(avatar.clone(), ProfileImage::Avatar).await;
        assert_eq!(image_dimensions(&avatar), Some((256, 128)));
        let banner = picture(dir.path(), "b.jpg", 1000, 500, ImageFormat::Jpeg);
        resize_profile_image(banner.clone(), ProfileImage::Banner).await;
        assert_eq!(image_dimensions(&banner), Some((1000, 500)));
    }

    #[test]
    fn playability_follows_container_and_codec() {
        assert_eq!(
            classify_video_playable("video/mp4", Some("H264")),
            Some(true)
        );
        assert_eq!(
            classify_video_playable("video/quicktime", Some("hevc")),
            Some(false)
        );
        assert_eq!(
            classify_video_playable("video/quicktime", Some("h264")),
            None
        );
        assert_eq!(classify_video_playable("video/mp4", None), None);
        assert_eq!(classify_video_playable("audio/mpeg", Some("h264")), None);
    }
}
