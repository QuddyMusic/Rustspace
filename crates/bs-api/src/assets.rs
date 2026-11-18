//! Файлы инстанса: аватары, баннеры, вложения и стикеры.
//!
//! Сервер отдаёт их без авторизации по двум путям — `/api/uploads/<имя>` и
//! `/api/stickers/assets/<имя>`. Клиент запрашивает **только** такие пути и
//! только на своём инстансе: значение из профиля или сообщения (имя файла,
//! путь, полный URL, `sticker:<url>`) сначала приводится к [`AssetPath`], а
//! всё, что не подходит, отбрасывается. Так чужой сервер не может заставить
//! клиент ходить на произвольные адреса или присылать токен куда-то ещё.

use reqwest::header;
use url::Url;

use crate::client::{ApiClient, read_limited};
use crate::error::ApiError;

/// Префикс загруженных файлов.
const UPLOADS: &str = "/api/uploads/";
/// Префикс ассетов стикеров.
const STICKERS: &str = "/api/stickers/assets/";
/// Префикс иконок игр активности (`bs-server`, PR оригинала #482).
const GAME_ICONS: &str = "/api/game-icons/assets/";
/// Префикс токена стикера в тексте сообщения.
pub const STICKER_PREFIX: &str = "sticker:";
/// Префикс картинки превью ссылки: клиент сам помечает так `image` карточки,
/// чтобы её можно было скачать с любого сайта (см. [`AssetPath::preview`]).
pub const PREVIEW_PREFIX: &str = "preview:";
/// Самая длинная ссылка на картинку превью.
const MAX_PREVIEW_LEN: usize = 2048;
/// Самое длинное допустимое значение поля с файлом.
const MAX_VALUE_LEN: usize = 512;
/// Самое длинное допустимое имя файла.
const MAX_NAME_LEN: usize = 128;

/// Хосты, с которых клиент согласен скачивать GIF напрямую.
///
/// Это CDN сервиса Klipy (и прежний Tenor): GIF в сообщении — это ссылка на
/// такой файл, а поиск идёт через свой инстанс, который хранит ключ сервиса.
pub const GIF_HOSTS: [&str; 2] = ["static.klipy.com", "media.tenor.com"];

/// Хосты обложек в активности «слушает».
///
/// Это CDN музыкальных сервисов, откуда плееры берут картинку трека. Другие
/// адреса из чужой активности клиент не открывает (иначе любой мог бы узнать
/// IP зрителя своего профиля).
pub const ART_HOSTS: [&str; 16] = [
    "i.scdn.co",
    "mosaic.scdn.co",
    "lh3.googleusercontent.com",
    "i.ytimg.com",
    "avatars.yandex.net",
    "p1.music.126.net",
    "p2.music.126.net",
    "y.gtimg.cn",
    "resources.tidal.com",
    "cdn-images.dzcdn.net",
    "e-cdns-images.dzcdn.net",
    "is1-ssl.mzstatic.com",
    "is2-ssl.mzstatic.com",
    "is3-ssl.mzstatic.com",
    "f4.bcbits.com",
    "i1.sndcdn.com",
];
/// Самая длинная допустимая внешняя ссылка.
const MAX_EXTERNAL_LEN: usize = 300;

/// Проверенный путь к файлу: на своём инстансе (`/api/uploads/…`) или
/// полная ссылка `https` на хост из [`GIF_HOSTS`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssetPath {
    path: String,
    /// Картинка превью ссылки с произвольного сайта.
    preview: bool,
}

impl AssetPath {
    /// Разбирает путь вида `/api/uploads/<имя>` или `/api/stickers/assets/<имя>`.
    #[must_use]
    pub fn parse(path: &str) -> Option<Self> {
        let name = path
            .strip_prefix(UPLOADS)
            .or_else(|| path.strip_prefix(STICKERS))
            .or_else(|| path.strip_prefix(GAME_ICONS))?;
        let valid = !name.is_empty()
            && name.len() <= MAX_NAME_LEN
            && !name.starts_with('.')
            && !name.contains("..")
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        valid.then(|| Self::plain(path.to_owned()))
    }

    /// Разбирает ссылку на GIF: только `https`, только хосты из
    /// [`GIF_HOSTS`], без логина, порта, параметров и фрагмента.
    #[must_use]
    pub fn external(value: &str) -> Option<Self> {
        if let Some(link) = value.trim().strip_prefix(PREVIEW_PREFIX) {
            return Self::preview(link);
        }
        let value = value.trim();
        if value.is_empty() || value.len() > MAX_EXTERNAL_LEN {
            return None;
        }
        let url = Url::parse(value).ok()?;
        let allowed = url.scheme() == "https"
            && url
                .host_str()
                .is_some_and(|h| GIF_HOSTS.contains(&h) || ART_HOSTS.contains(&h))
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path().len() > 1
            && url.path().bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-' | b'%' | b'~')
            });
        allowed.then(|| Self::plain(url.to_string()))
    }

    fn plain(path: String) -> Self {
        Self {
            path,
            preview: false,
        }
    }

    /// Картинка превью ссылки (`og:image`) с любого сайта, как в веб-клиенте
    /// оригинала: только `https`, только доменное имя (не IP-адрес и не
    /// локальное имя), порт по умолчанию, без логина. Параметры запроса
    /// допустимы — без них такие картинки часто не отдаются.
    #[must_use]
    pub fn preview(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() || value.len() > MAX_PREVIEW_LEN {
            return None;
        }
        let mut url = Url::parse(value).ok()?;
        let domain = match url.host()? {
            url::Host::Domain(domain) => domain.to_ascii_lowercase(),
            url::Host::Ipv4(_) | url::Host::Ipv6(_) => return None,
        };
        let local = !domain.contains('.')
            || [".local", ".localhost", ".internal", ".lan", ".home.arpa"]
                .iter()
                .any(|suffix| domain.ends_with(suffix));
        if url.scheme() != "https"
            || local
            || url.port().is_some_and(|p| p != 443)
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        url.set_fragment(None);
        Some(Self {
            path: url.to_string(),
            preview: true,
        })
    }

    /// Открытый файл чужого инстанса из общего каталога (иконка, баннер
    /// пространства): `https://<инстанс>/api/uploads/<имя>`, без токена.
    #[must_use]
    pub fn public(origin: &str, value: &str) -> Option<Self> {
        let base = Url::parse(origin).ok()?;
        if base.scheme() != "https" || base.host_str().is_none() {
            return None;
        }
        let path = resolve_asset(origin, value)?;
        let url = base.join(path.as_str()).ok()?;
        Some(Self::plain(url.to_string()))
    }

    /// Это внешняя ссылка, а не путь на инстансе.
    #[must_use]
    pub fn is_external(&self) -> bool {
        self.path.starts_with("https://")
    }

    /// Путь для запроса.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.path
    }
}

/// Клиент для внешних хостов: без редиректов, только `https`, без токенов.
fn external_client() -> Result<&'static reqwest::Client, ApiError> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(crate::client::USER_AGENT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            // CDN иногда отвечает перенаправлением на соседний путь; идём
            // только по ссылкам, которые сами бы приняли.
            if attempt.previous().len() < 3 && AssetPath::external(attempt.url().as_str()).is_some()
            {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .https_only(true)
        .build()?;
    Ok(CLIENT.get_or_init(|| client))
}

/// Клиент для картинок превью: перенаправления — только на адреса, которые
/// прошли бы [`AssetPath::preview`].
fn preview_client() -> Result<&'static reqwest::Client, ApiError> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(crate::client::USER_AGENT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() < 5 && AssetPath::preview(attempt.url().as_str()).is_some()
            {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .https_only(true)
        .build()?;
    Ok(CLIENT.get_or_init(|| client))
}

/// Скачивает внешний файл (GIF), но не больше `limit` байт.
///
/// Принимает только пути, прошедшие [`AssetPath::external`]: никакого токена,
/// перенаправлений и посторонних адресов.
///
/// # Errors
/// Сетевые ошибки, не-2xx ответ, превышение `limit`, путь не внешний.
pub async fn fetch_external(asset: &AssetPath, limit: usize) -> Result<Vec<u8>, ApiError> {
    if !asset.is_external() {
        return Err(ApiError::Url(url::ParseError::RelativeUrlWithoutBase));
    }
    let client = if asset.preview {
        preview_client()?
    } else {
        external_client()?
    };
    let response = client
        .get(asset.as_str())
        .header(
            header::ACCEPT,
            "image/webp,image/gif,image/*;q=0.8,*/*;q=0.5",
        )
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(ApiError::Unexpected {
            status,
            body: String::new(),
        });
    }
    read_limited(response, limit).await
}

/// Приводит значение поля (аватар, баннер, иконка, `filename` вложения,
/// `sticker:<url>`) к пути на инстансе `base`. Всё лишнее даёт `None`.
#[must_use]
pub fn resolve_asset(base: &str, value: &str) -> Option<AssetPath> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_VALUE_LEN {
        return None;
    }
    let value = value.strip_prefix(STICKER_PREFIX).unwrap_or(value);
    if value.contains("://") {
        let url = Url::parse(value).ok()?;
        let base = Url::parse(base).ok()?;
        let same_origin = url.scheme() == base.scheme()
            && url.host_str() == base.host_str()
            && url.port_or_known_default() == base.port_or_known_default();
        if !same_origin
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        return AssetPath::parse(url.path());
    }
    if value.starts_with('/') {
        return AssetPath::parse(value);
    }
    AssetPath::parse(&format!("{UPLOADS}{value}"))
}

impl ApiClient {
    /// Скачивает файл целиком, но не больше `limit` байт.
    ///
    /// Токен не отправляется: файлы публичны, а запрос не должен нести
    /// секреты. Редиректы выключены в самом клиенте.
    ///
    /// # Errors
    /// Сетевые ошибки, не-2xx ответ или превышение `limit`.
    pub async fn fetch_asset(&self, asset: &AssetPath, limit: usize) -> Result<Vec<u8>, ApiError> {
        if asset.is_external() {
            return fetch_external(asset, limit).await;
        }
        let url = self.endpoint(asset.as_str())?;
        let response = self
            .http()
            .get(url)
            .header(header::ACCEPT, "image/*")
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(ApiError::Unexpected {
                status,
                body: String::new(),
            });
        }
        read_limited(response, limit).await
    }
}

/// Сколько секунд даём на скачивание одного файла (обычный таймаут клиента
/// рассчитан на короткие запросы и оборвал бы большой файл).
const DOWNLOAD_TIMEOUT_SECS: u64 = 3600;

impl ApiClient {
    /// Скачивает файл на диск потоком, не держа его целиком в памяти.
    ///
    /// Данные пишутся во временный `<dest>.part`; после полного получения он
    /// переименовывается в `dest` (существующий файл заменяется). При любой
    /// ошибке временный файл удаляется. `progress` получает число принятых
    /// байт и общий размер, если сервер его сообщил. Токен не отправляется.
    ///
    /// # Errors
    /// Сетевые ошибки, не-2xx ответ, внешний путь, ошибки записи на диск.
    pub async fn download_asset(
        &self,
        asset: &AssetPath,
        dest: &std::path::Path,
        mut progress: impl FnMut(u64, Option<u64>) + Send,
    ) -> Result<u64, ApiError> {
        use std::io::Write as _;

        if asset.is_external() {
            return Err(ApiError::Url(url::ParseError::RelativeUrlWithoutBase));
        }
        let url = self.endpoint(asset.as_str())?;
        let mut response = self
            .http()
            .get(url)
            .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(ApiError::Unexpected {
                status,
                body: String::new(),
            });
        }
        let total = response.content_length();
        let mut tmp = dest.as_os_str().to_owned();
        tmp.push(".part");
        let tmp = std::path::PathBuf::from(tmp);
        // Запись идёт кусками по мере прихода; куски небольшие, поэтому
        // блокирующая запись на диск не задерживает исполнитель заметно.
        let result = async {
            let mut file = std::fs::File::create(&tmp).map_err(ApiError::Io)?;
            let mut received: u64 = 0;
            progress(0, total);
            while let Some(chunk) = response.chunk().await? {
                file.write_all(&chunk).map_err(ApiError::Io)?;
                received += chunk.len() as u64;
                progress(received, total);
            }
            file.flush().map_err(ApiError::Io)?;
            drop(file);
            std::fs::rename(&tmp, dest).map_err(ApiError::Io)?;
            Ok(received)
        }
        .await;
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{AssetPath, resolve_asset};

    #[test]
    fn gif_links_only_from_allowed_hosts_over_https() {
        let ok = AssetPath::external("https://static.klipy.com/ii/abc/def.gif");
        assert!(ok.is_some_and(|p| p.is_external()));
        let preview = AssetPath::external("preview:https://equixly.com/og.png?v=2#x");
        assert_eq!(
            preview.as_ref().map(AssetPath::as_str),
            Some("https://equixly.com/og.png?v=2")
        );
        for bad in [
            "preview:http://equixly.com/og.png",
            "preview:https://192.168.1.1/a.png",
            "preview:https://router.local/a.png",
            "preview:https://localhost/a.png",
            "preview:https://user:pw@equixly.com/a.png",
            "preview:https://equixly.com:8443/a.png",
            // Без пометки произвольный сайт по-прежнему не годится.
            "https://equixly.com/og.png",
        ] {
            assert!(AssetPath::external(bad).is_none(), "{bad}");
        }
        assert!(AssetPath::external("https://media.tenor.com/xyz/cat.gif").is_some());
        for bad in [
            "http://static.klipy.com/a.gif",
            "https://evil.example/a.gif",
            "https://static.klipy.com.evil.example/a.gif",
            "https://static.klipy.com:8443/a.gif",
            "https://user@static.klipy.com/a.gif",
            "https://static.klipy.com/a.gif?x=1",
            "https://static.klipy.com/",
            "/api/uploads/a.png",
        ] {
            assert!(AssetPath::external(bad).is_none(), "{bad}");
        }
    }

    const BASE: &str = "https://chat.example.com";

    fn path(value: &str) -> Option<String> {
        resolve_asset(BASE, value).map(|p| p.as_str().to_owned())
    }

    #[test]
    fn bare_filename_goes_to_uploads() {
        assert_eq!(
            path("abc123.webp").as_deref(),
            Some("/api/uploads/abc123.webp")
        );
    }

    #[test]
    fn same_origin_url_and_path_are_accepted() {
        assert_eq!(
            path("https://chat.example.com/api/uploads/a.png").as_deref(),
            Some("/api/uploads/a.png")
        );
        assert_eq!(
            path("/api/uploads/a.png").as_deref(),
            Some("/api/uploads/a.png")
        );
    }

    #[test]
    fn sticker_token_resolves_to_asset() {
        let id = "a".repeat(64);
        let value = format!("sticker:https://chat.example.com/api/stickers/assets/{id}.webp");
        assert_eq!(
            path(&value),
            Some(format!("/api/stickers/assets/{id}.webp"))
        );
    }

    #[test]
    fn foreign_hosts_and_schemes_are_rejected() {
        assert_eq!(path("https://evil.example.org/api/uploads/a.png"), None);
        assert_eq!(path("http://chat.example.com/api/uploads/a.png"), None);
        assert_eq!(
            path("https://chat.example.com:8443/api/uploads/a.png"),
            None
        );
        assert_eq!(
            path("https://user:pw@chat.example.com/api/uploads/a.png"),
            None
        );
        assert_eq!(path("https://chat.example.com/api/uploads/a.png?x=1"), None);
        assert_eq!(path("data:image/png;base64,AAAA"), None);
    }

    #[test]
    fn traversal_and_odd_names_are_rejected() {
        assert_eq!(path("../secret"), None);
        assert_eq!(path("/api/uploads/../auth/login"), None);
        assert_eq!(path("/api/uploads/.hidden"), None);
        assert_eq!(path("a/b.png"), None);
        assert_eq!(path("a b.png"), None);
        assert_eq!(path("a%2fb.png"), None);
        assert_eq!(path(""), None);
        assert_eq!(path("/api/auth/login"), None);
        assert_eq!(path(&"x".repeat(200)), None);
    }

    #[test]
    fn parse_accepts_only_known_prefixes() {
        assert!(AssetPath::parse("/api/uploads/x.gif").is_some());
        assert!(AssetPath::parse("/api/stickers/assets/x.webp").is_some());
        assert!(AssetPath::parse("/api/users/@me").is_none());
    }
}
