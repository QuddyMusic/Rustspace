//! Встраивания: предпросмотр ссылок из сообщений (`utils/embedResolver.ts`,
//! `utils/embedClassifier.ts`, `utils/metadataFetcher.ts`).

use std::io::Cursor;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use reqwest::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue, RANGE};
use scraper::{Html, Selector};
use serde_json::{Value, json};
use url::Url;

use crate::state::{AppState, now_ms};
use crate::{broadcast, net, params};

/// Сколько ссылок одного сообщения получают встраивание.
const MAX_EMBEDS_PER_MESSAGE: usize = 5;
/// Сколько байт картинки читается, чтобы узнать её размеры.
const PROBE_BYTES: usize = 32_768;
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// Предел на страницу при чтении её описания.
const PAGE_BYTES: usize = 512_000;
const PAGE_TIMEOUT: Duration = Duration::from_secs(5);

/// Где находится сообщение, для которого считаются встраивания.
#[derive(Debug, Clone)]
pub enum Place {
    /// Канал пространства.
    Channel {
        /// Пространство.
        space_id: String,
        /// Канал.
        channel_id: String,
    },
    /// Личная переписка.
    Dm {
        /// Переписка.
        dm_channel_id: String,
    },
}

/// Адрес стикера из текста сообщения-стикера (`stickerUrl`).
#[must_use]
pub fn sticker_url(value: &str) -> Option<&str> {
    static PATH: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"^/api/stickers/assets/[a-f0-9]{64}\.webp$").ok());
    let candidate = value.strip_prefix("sticker:")?;
    let url = Url::parse(candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    if url.query().is_some() || url.fragment().is_some() || !PATH.as_ref()?.is_match(url.path()) {
        return None;
    }
    (url.as_str() == candidate).then_some(candidate)
}

/// Ссылки из текста: без повторов, не больше пяти (`extractUrls`).
#[must_use]
pub fn extract_urls(content: Option<&str>) -> Vec<String> {
    static LINK: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r#"https?://[^\s<>"{}|\\^`\[\]]+"#).ok());
    let Some(content) =
        content.filter(|content| !content.is_empty() && sticker_url(content).is_none())
    else {
        return Vec::new();
    };
    let Some(link) = LINK.as_ref() else {
        return Vec::new();
    };
    let mut unique: Vec<String> = Vec::new();
    for found in link.find_iter(content) {
        if !unique.iter().any(|url| url == found.as_str()) {
            unique.push(found.as_str().to_owned());
        }
    }
    unique.truncate(MAX_EMBEDS_PER_MESSAGE);
    unique
}

/// Что известно о ссылке по одному её виду (`classifyUrl`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    /// Вид встраивания.
    pub embed_type: &'static str,
    /// Известный сервис.
    pub provider: Option<&'static str>,
    /// Адрес проигрывателя.
    pub embed_url: Option<String>,
    /// Нужно ли читать страницу.
    pub needs_metadata: bool,
}

fn by_extension(url: &str) -> Option<&'static str> {
    static KINDS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
        [
            ("image", "jpg|jpeg|png|gif|webp|avif"),
            ("audio", "mp3|ogg|wav|flac|opus"),
            ("video", "mp4|webm|mov"),
        ]
        .into_iter()
        .filter_map(|(kind, list)| {
            Regex::new(&format!(r"(?is)\.({list})(\?.*)?$"))
                .ok()
                .map(|regex| (kind, regex))
        })
        .collect()
    });
    KINDS
        .iter()
        .find(|(_, regex)| regex.is_match(url))
        .map(|(kind, _)| *kind)
}

fn id_after<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(prefix)?;
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

fn youtube_id(url: &Url, host: &str) -> Option<String> {
    if host == "youtu.be" {
        let id = url.path().get(1..)?.split('/').next()?;
        return (!id.is_empty()).then(|| id.to_owned());
    }
    let path = url.path();
    if path == "/watch" {
        return url
            .query_pairs()
            .find(|(key, _)| key == "v")
            .map(|(_, value)| value.into_owned());
    }
    ["/shorts/", "/embed/", "/v/"]
        .iter()
        .find_map(|prefix| id_after(path, prefix))
        .map(str::to_owned)
}

/// Разбор ссылки (`classifyUrl`).
#[must_use]
pub fn classify_url(raw: &str) -> Classification {
    let plain = |embed_type, needs_metadata| Classification {
        embed_type,
        provider: None,
        embed_url: None,
        needs_metadata,
    };
    if let Some(kind) = by_extension(raw) {
        return plain(kind, false);
    }
    let Ok(url) = Url::parse(raw) else {
        return plain("generic", true);
    };
    let full_host = url.host_str().unwrap_or_default();
    let host = full_host.strip_prefix("www.").unwrap_or(full_host);
    let known = |embed_type, provider, embed_url| Classification {
        embed_type,
        provider: Some(provider),
        embed_url: Some(embed_url),
        needs_metadata: true,
    };
    if matches!(host, "youtube.com" | "m.youtube.com" | "youtu.be")
        && let Some(id) = youtube_id(&url, host).filter(|id| !id.is_empty())
    {
        return known(
            "video",
            "youtube",
            format!("https://www.youtube-nocookie.com/embed/{id}"),
        );
    }
    if host == "vimeo.com" {
        let digits: String = url
            .path()
            .chars()
            .skip(1)
            .take_while(char::is_ascii_digit)
            .collect();
        if url.path().starts_with('/') && !digits.is_empty() {
            return known(
                "video",
                "vimeo",
                format!("https://player.vimeo.com/video/{digits}"),
            );
        }
    }
    if host == "open.spotify.com" {
        for kind in ["track", "album", "playlist"] {
            let id: String = url
                .path()
                .strip_prefix(&format!("/{kind}/"))
                .unwrap_or_default()
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            if !id.is_empty() {
                return known(
                    "rich",
                    "spotify",
                    format!("https://open.spotify.com/embed/{kind}/{id}"),
                );
            }
        }
    }
    plain("generic", true)
}

/// Описание страницы (`UrlMetadata`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    /// Заголовок.
    pub title: Option<String>,
    /// Описание.
    pub description: Option<String>,
    /// Картинка.
    pub image: Option<String>,
    /// Название сайта.
    pub site_name: Option<String>,
    /// Тип содержимого, если по ссылке не страница, а файл.
    pub content_type: Option<String>,
    /// Ширина картинки.
    pub image_width: Option<i64>,
    /// Высота картинки.
    pub image_height: Option<i64>,
}

impl Metadata {
    /// Ответ `GET /api/utils/metadata`.
    #[must_use]
    pub fn to_json(&self, url: &str) -> Value {
        let mut body = json!({
            "title": self.title,
            "description": self.description,
            "image": self.image,
            "siteName": self.site_name,
            "url": url,
        });
        for (name, value) in [
            ("contentType", self.content_type.clone().map(Value::from)),
            ("imageWidth", self.image_width.map(Value::from)),
            ("imageHeight", self.image_height.map(Value::from)),
        ] {
            if let Some(value) = value {
                body[name] = value;
            }
        }
        body
    }
}

/// `parseInt(text, 10)` для положительных чисел.
fn leading_int(text: &str) -> Option<i64> {
    let digits: String = text
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse::<i64>().ok().filter(|value| *value > 0)
}

/// Достаёт описание из разметки страницы.
#[must_use]
pub fn parse_page(html: &str) -> Metadata {
    let document = Html::parse_document(html);
    let attr = |selector: &str| {
        let selector = Selector::parse(selector).ok()?;
        document
            .select(&selector)
            .next()?
            .value()
            .attr("content")
            .map(str::to_owned)
    };
    let filled = |value: Option<String>| value.filter(|text| !text.is_empty());
    let title = || {
        let selector = Selector::parse("title").ok()?;
        Some(
            document
                .select(&selector)
                .flat_map(|node| node.text())
                .collect::<String>(),
        )
    };
    Metadata {
        title: filled(attr(r#"meta[property="og:title"]"#)).or_else(|| filled(title())),
        description: filled(attr(r#"meta[property="og:description"]"#))
            .or_else(|| filled(attr(r#"meta[name="description"]"#))),
        image: filled(attr(r#"meta[property="og:image"]"#)),
        site_name: filled(attr(r#"meta[property="og:site_name"]"#)),
        content_type: None,
        image_width: attr(r#"meta[property="og:image:width"]"#)
            .as_deref()
            .and_then(leading_int),
        image_height: attr(r#"meta[property="og:image:height"]"#)
            .as_deref()
            .and_then(leading_int),
    }
}

/// Читает описание страницы по ссылке (`fetchUrlMetadata`).
pub async fn fetch_metadata(url: &str) -> Option<Metadata> {
    let started = std::time::Instant::now();
    let fetched = net::safe_get(url, HeaderMap::new(), PAGE_TIMEOUT).await?;
    if !(200..300).contains(&fetched.status) {
        return None;
    }
    let content_type = fetched
        .headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if ["image/", "video/", "audio/"]
        .iter()
        .any(|prefix| content_type.starts_with(prefix))
    {
        return Some(Metadata {
            content_type: Some(content_type.to_owned()),
            ..Metadata::default()
        });
    }
    let declared = fetched
        .headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(leading_int);
    if declared.is_some_and(|length| length > 512_000) {
        return None;
    }
    let left = PAGE_TIMEOUT.checked_sub(started.elapsed())?;
    let (body, _) = net::read_limited(fetched.response, PAGE_BYTES, left).await;
    let html = String::from_utf8_lossy(&body).into_owned();
    tokio::task::spawn_blocking(move || parse_page(&html))
        .await
        .ok()
}

/// Размеры картинки по ссылке: читается только начало файла
/// (`probeRemoteImageDimensions`).
pub async fn probe_image(url: &str) -> Option<(i64, i64)> {
    let started = std::time::Instant::now();
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("image/*"));
    headers.insert(
        RANGE,
        HeaderValue::from_str(&format!("bytes=0-{}", PROBE_BYTES - 1)).ok()?,
    );
    let fetched = net::safe_get(url, headers, PROBE_TIMEOUT).await?;
    if !(200..300).contains(&fetched.status) {
        return None;
    }
    let left = PROBE_TIMEOUT.checked_sub(started.elapsed())?;
    let (body, _) = net::read_limited(fetched.response, PROBE_BYTES, left).await;
    tokio::task::spawn_blocking(move || {
        let (width, height) = image::ImageReader::new(Cursor::new(body))
            .with_guessed_format()
            .ok()?
            .into_dimensions()
            .ok()?;
        (width > 0 && height > 0).then_some((i64::from(width), i64::from(height)))
    })
    .await
    .ok()
    .flatten()
}

/// Встраивание, готовое к записи.
#[derive(Debug, Clone)]
struct Resolved {
    url: String,
    embed_type: &'static str,
    provider: Option<&'static str>,
    title: Option<String>,
    description: Option<String>,
    image: Option<String>,
    embed_url: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
}

async fn resolve_one(url: String) -> Option<Resolved> {
    let class = classify_url(&url);
    let mut out = Resolved {
        url: url.clone(),
        embed_type: class.embed_type,
        provider: class.provider,
        title: None,
        description: None,
        image: None,
        embed_url: class.embed_url.clone(),
        width: None,
        height: None,
    };
    if class.provider == Some("youtube")
        && let Some(id) = class
            .embed_url
            .as_deref()
            .and_then(|player| player.rsplit('/').next())
            .and_then(|id| id.split('?').next())
        && !id.is_empty()
    {
        // hqdefault.jpg всегда 480x360.
        out.image = Some(format!("https://img.youtube.com/vi/{id}/hqdefault.jpg"));
        out.width = Some(480);
        out.height = Some(360);
    }
    if class.embed_type == "image" {
        out.image = Some(url.clone());
    } else if class.needs_metadata {
        if let Some(meta) = fetch_metadata(&url).await {
            if let Some(content_type) = meta.content_type.as_deref().filter(|kind| !kind.is_empty())
            {
                if content_type.starts_with("image/") {
                    out.embed_type = "image";
                    out.image = Some(url.clone());
                } else if content_type.starts_with("video/") {
                    out.embed_type = "video";
                } else if content_type.starts_with("audio/") {
                    out.embed_type = "audio";
                }
            } else {
                out.title = meta.title;
                out.description = meta.description;
                out.image = meta.image.or(out.image);
                if let (Some(width), Some(height)) = (meta.image_width, meta.image_height) {
                    out.width = out.width.or(Some(width));
                    out.height = out.height.or(Some(height));
                }
            }
        }
        if out.embed_type == "generic" && out.title.is_none() {
            return None;
        }
    }
    if out.embed_type == "image"
        && out.width.is_none()
        && let Some(image) = out.image.as_deref()
        && let Some((width, height)) = probe_image(image).await
    {
        out.width = Some(width);
        out.height = Some(height);
    }
    Some(out)
}

async fn resolve(state: AppState, message_id: String, content: Option<String>, place: Place) {
    let urls = extract_urls(content.as_deref());
    if urls.is_empty() {
        return;
    }
    let mut resolved = Vec::new();
    for url in urls {
        if let Some(embed) = resolve_one(url).await {
            resolved.push((state.ids.next(), embed));
        }
    }
    if resolved.is_empty() {
        return;
    }
    let hub = state.hub.clone();
    let done = state
        .run(move |tx, out| {
            let is_dm = matches!(place, Place::Dm { .. });
            // Пока шла работа, сообщение могли удалить.
            let table = if is_dm { "dm_messages" } else { "messages" };
            if tx.query_one(&format!("SELECT id FROM {table} WHERE id = ?"), params![&message_id])?.is_none() {
                return Ok(());
            }
            let now = now_ms();
            let (in_channel, in_dm) = if is_dm { (None, Some(&message_id)) } else { (Some(&message_id), None) };
            let mut embeds = Vec::new();
            for (id, embed) in &resolved {
                tx.execute(
                    "INSERT INTO embeds (id, message_id, dm_message_id, url, embed_type, provider, title, description, \
                     image, embed_url, width, height, color, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?)",
                    params![
                        id,
                        in_channel,
                        in_dm,
                        &embed.url,
                        embed.embed_type,
                        embed.provider,
                        &embed.title,
                        &embed.description,
                        &embed.image,
                        &embed.embed_url,
                        embed.width,
                        embed.height,
                        now
                    ],
                )?;
                embeds.push(json!({
                    "id": id,
                    "messageId": in_channel,
                    "dmMessageId": in_dm,
                    "url": embed.url,
                    "embedType": embed.embed_type,
                    "provider": embed.provider,
                    "title": embed.title,
                    "description": embed.description,
                    "image": embed.image,
                    "embedUrl": embed.embed_url,
                    "width": embed.width,
                    "height": embed.height,
                    "color": null,
                    "createdAt": now,
                }));
            }
            match &place {
                Place::Dm { dm_channel_id } => {
                    let event = json!({
                        "type": "dm_embeds_resolved",
                        "messageId": message_id,
                        "dmChannelId": dm_channel_id,
                        "embeds": embeds,
                    });
                    broadcast::to_dm_members(tx, out, dm_channel_id, &event, None)?;
                }
                Place::Channel { space_id, channel_id } => {
                    let event = json!({
                        "type": "embeds_resolved",
                        "messageId": message_id,
                        "channelId": channel_id,
                        "embeds": embeds,
                    });
                    broadcast::to_channel(tx, &hub, out, space_id, channel_id, &event, None)?;
                }
            }
            Ok(())
        })
        .await;
    if done.is_err() {
        tracing::debug!("встраивания не записаны");
    }
}

/// Считает встраивания для сообщения в фоне и рассылает их, когда готовы.
pub fn schedule(state: &AppState, message_id: String, content: Option<String>, place: Place) {
    if extract_urls(content.as_deref()).is_empty() {
        return;
    }
    tokio::spawn(resolve(state.clone(), message_id, content, place));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_found_once_and_capped() {
        assert_eq!(
            extract_urls(Some(
                "см. https://a.example/x?y=1, <http://b.example> и https://a.example/x?y=1,"
            )),
            vec!["https://a.example/x?y=1,", "http://b.example"]
        );
        let many = (0..9)
            .map(|n| format!("https://e.example/{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(extract_urls(Some(&many)).len(), 5);
        assert!(extract_urls(Some("без ссылок ftp://x")).is_empty());
        assert!(extract_urls(None).is_empty());
    }

    #[test]
    fn stickers_are_not_links() {
        let id = "a".repeat(64);
        let sticker = format!("sticker:https://host.example/api/stickers/assets/{id}.webp");
        assert!(sticker_url(&sticker).is_some());
        assert!(extract_urls(Some(&sticker)).is_empty());
        for bad in [
            format!("sticker:https://u:p@host.example/api/stickers/assets/{id}.webp"),
            format!("sticker:https://host.example/api/stickers/assets/{id}.webp?x=1"),
            format!("sticker:https://HOST.example/api/stickers/assets/{id}.webp"),
            format!("sticker:ftp://host.example/api/stickers/assets/{id}.webp"),
            "sticker:https://host.example/api/stickers/assets/apple.webp".to_owned(),
        ] {
            assert!(sticker_url(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn known_services_and_file_kinds() {
        assert_eq!(
            classify_url("https://x.example/a.PNG?w=1").embed_type,
            "image"
        );
        assert_eq!(classify_url("https://x.example/a.opus").embed_type, "audio");
        assert_eq!(classify_url("https://x.example/a.mov").embed_type, "video");
        let youtube = classify_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1");
        assert_eq!(
            youtube.embed_url.as_deref(),
            Some("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ")
        );
        assert_eq!(
            classify_url("https://youtu.be/abc_-1/x")
                .embed_url
                .as_deref(),
            Some("https://www.youtube-nocookie.com/embed/abc_-1")
        );
        assert_eq!(
            classify_url("https://m.youtube.com/shorts/Ab1?x").provider,
            Some("youtube")
        );
        assert_eq!(classify_url("https://youtube.com/watch").provider, None);
        assert_eq!(
            classify_url("https://vimeo.com/12345/abc")
                .embed_url
                .as_deref(),
            Some("https://player.vimeo.com/video/12345")
        );
        assert_eq!(classify_url("https://vimeo.com/about").provider, None);
        assert_eq!(
            classify_url("https://open.spotify.com/album/Ab12?si=1")
                .embed_url
                .as_deref(),
            Some("https://open.spotify.com/embed/album/Ab12")
        );
        let generic = classify_url("https://example.com/page");
        assert_eq!(
            (generic.embed_type, generic.needs_metadata),
            ("generic", true)
        );
    }

    #[test]
    fn page_description_is_read_from_markup() {
        let page = r#"<html><head><title>Обычный &amp; заголовок</title>
            <meta property="og:title" content="Заголовок"><meta name="description" content="Описание">
            <meta property="og:image" content="https://x.example/i.png">
            <meta property="og:image:width" content="1200px"><meta property="og:image:height" content="0">
            </head><body><title>не тот</title></body></html>"#;
        let meta = parse_page(page);
        assert_eq!(meta.title.as_deref(), Some("Заголовок"));
        assert_eq!(meta.description.as_deref(), Some("Описание"));
        assert_eq!(meta.image.as_deref(), Some("https://x.example/i.png"));
        assert_eq!((meta.image_width, meta.image_height), (Some(1200), None));
        assert_eq!(
            parse_page("<title>Только &lt;title&gt;</title>")
                .title
                .as_deref(),
            Some("Только <title>")
        );
        assert_eq!(parse_page("просто текст"), Metadata::default());
    }
}
