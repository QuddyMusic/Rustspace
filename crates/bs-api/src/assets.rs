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
/// Префикс токена стикера в тексте сообщения.
pub const STICKER_PREFIX: &str = "sticker:";
/// Самое длинное допустимое значение поля с файлом.
const MAX_VALUE_LEN: usize = 512;
/// Самое длинное допустимое имя файла.
const MAX_NAME_LEN: usize = 128;

/// Хосты, с которых клиент согласен скачивать GIF напрямую.
///
/// Это CDN сервиса Klipy (и прежний Tenor): GIF в сообщении — это ссылка на
/// такой файл, а поиск идёт через свой инстанс, который хранит ключ сервиса.
pub const GIF_HOSTS: [&str; 2] = ["static.klipy.com", "media.tenor.com"];
/// Самая длинная допустимая внешняя ссылка.
const MAX_EXTERNAL_LEN: usize = 300;

/// Проверенный путь к файлу: на своём инстансе (`/api/uploads/…`) или
/// полная ссылка `https` на хост из [`GIF_HOSTS`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssetPath(String);

impl AssetPath {
    /// Разбирает путь вида `/api/uploads/<имя>` или `/api/stickers/assets/<имя>`.
    #[must_use]
    pub fn parse(path: &str) -> Option<Self> {
        let name = path
            .strip_prefix(UPLOADS)
            .or_else(|| path.strip_prefix(STICKERS))?;
        let valid = !name.is_empty()
            && name.len() <= MAX_NAME_LEN
            && !name.starts_with('.')
            && !name.contains("..")
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        valid.then(|| Self(path.to_owned()))
    }

    /// Разбирает ссылку на GIF: только `https`, только хосты из
    /// [`GIF_HOSTS`], без логина, порта, параметров и фрагмента.
    #[must_use]
    pub fn external(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() || value.len() > MAX_EXTERNAL_LEN {
            return None;
        }
        let url = Url::parse(value).ok()?;
        let allowed = url.scheme() == "https"
            && url.host_str().is_some_and(|h| GIF_HOSTS.contains(&h))
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path().len() > 1
            && url.path().bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-' | b'%' | b'~')
            });
        allowed.then(|| Self(url.to_string()))
    }

    /// Это внешняя ссылка, а не путь на инстансе.
    #[must_use]
    pub fn is_external(&self) -> bool {
        self.0.starts_with("https://")
    }

    /// Путь для запроса.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
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
            if attempt.previous().len() < 3
                && AssetPath::external(attempt.url().as_str()).is_some()
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
    let response = external_client()?
        .get(asset.as_str())
        .header(header::ACCEPT, "image/webp,image/gif,image/*;q=0.8,*/*;q=0.5")
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

#[cfg(test)]
mod tests {
    use super::{AssetPath, resolve_asset};

    #[test]
    fn gif_links_only_from_allowed_hosts_over_https() {
        let ok = AssetPath::external("https://static.klipy.com/ii/abc/def.gif");
        assert!(ok.is_some_and(|p| p.is_external()));
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
