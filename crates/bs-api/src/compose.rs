//! Всё, что нужно композеру: загрузка файлов (tus), GIF, личные стикеры и
//! отправка сообщения с вложениями.
//!
//! Загрузка устроена как у веб-клиента: `POST /api/files/` создаёт сессию,
//! затем `PATCH` порциями; ответ на последний `PATCH` — готовое вложение,
//! чей `id` потом передаётся в сообщение. Токен уходит только на свой инстанс.

use bs_types::{Attachment, MessageWithUser};
use reqwest::{Method, header};
use serde::{Deserialize, Serialize};

use crate::client::{ApiClient, MAX_RESPONSE_BYTES, read_limited};
use crate::error::ApiError;

/// Размер одной порции загрузки.
pub const MAX_UPLOAD_CHUNK: usize = 1024 * 1024;

const TUS_VERSION: &str = "1.0.0";

/// Один GIF из поиска.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GifItem {
    /// Идентификатор.
    pub id: String,
    /// Подпись.
    #[serde(default)]
    pub title: String,
    /// Маленькая картинка для сетки.
    pub preview_url: String,
    /// Полный GIF; именно этот адрес уходит в сообщение.
    pub url: String,
    /// Ширина превью.
    #[serde(default)]
    pub width: u32,
    /// Высота превью.
    #[serde(default)]
    pub height: u32,
}

/// Страница GIF.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct GifPage {
    /// Найденное.
    #[serde(default)]
    pub results: Vec<GifItem>,
    /// Курсор следующей страницы; пустой — конец.
    #[serde(default)]
    pub next: String,
}

/// Личный стикер.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PersonalSticker {
    /// Идентификатор.
    pub id: String,
    /// Имя.
    pub name: String,
    /// Токен для сообщения: `sticker:<url>`.
    pub token: String,
}

/// Тело `POST /api/channels/:id/messages`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageBody {
    /// Текст (может быть пустым при вложениях).
    pub content: String,
    /// Идентификаторы загруженных вложений.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    /// Ответ на сообщение.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_id: Option<String>,
}

#[derive(Serialize)]
struct StickerUpload<'a> {
    name: &'a str,
    image: String,
}

#[derive(Deserialize)]
struct Enabled {
    enabled: bool,
}

/// Base64 (стандартный алфавит, с `=`).
#[must_use]
pub fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18_u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `Upload-Metadata`: пары `ключ base64(значение)` через запятую.
fn tus_metadata(filename: &str, filetype: &str) -> String {
    format!(
        "filename {},filetype {}",
        base64(filename.as_bytes()),
        base64(filetype.as_bytes())
    )
}

/// Путь запроса GIF: популярное или поиск.
fn gif_path(query: &str, limit: u8, pos: &str) -> String {
    let mut params = url::form_urlencoded::Serializer::new(String::new());
    params.append_pair("limit", &limit.to_string());
    if !pos.is_empty() {
        params.append_pair("pos", pos);
    }
    if query.trim().is_empty() {
        format!("/api/gif/trending?{}", params.finish())
    } else {
        params.append_pair("q", query.trim());
        format!("/api/gif/search?{}", params.finish())
    }
}

/// Принимает от сервера только путь `Location` на своём же хосте.
fn location_path(location: &str) -> Option<String> {
    if location.starts_with("/api/files/") && !location.contains("..") {
        Some(location.to_owned())
    } else {
        None
    }
}

impl ApiClient {
    /// Загружает файл по tus и возвращает готовое вложение.
    ///
    /// `progress(отправлено, всего)` вызывается после каждой порции.
    ///
    /// # Errors
    /// Сетевые ошибки, отказ сервера (размер, лимиты, права) и некорректный
    /// ответ. Пустые файлы не принимаются.
    pub async fn upload_file(
        &self,
        filename: &str,
        mimetype: &str,
        bytes: &[u8],
        mut progress: impl FnMut(u64, u64) + Send,
    ) -> Result<Attachment, ApiError> {
        let total = bytes.len() as u64;
        let create = self
            .http()
            .post(self.endpoint("/api/files/")?)
            .header("Tus-Resumable", TUS_VERSION)
            .header("Upload-Length", total.to_string())
            .header("Upload-Metadata", tus_metadata(filename, mimetype))
            .header(header::CONTENT_LENGTH, "0");
        let create = self.authorize(create).send().await?;
        let status = create.status();
        if !status.is_success() {
            let body = read_limited(create, 4096).await.unwrap_or_default();
            return Err(ApiError::Unexpected {
                status,
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }
        let location = create
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .and_then(location_path)
            .ok_or(ApiError::Unexpected {
                status,
                body: "сервер не прислал адрес загрузки".to_owned(),
            })?;
        let mut offset = 0_usize;
        progress(0, total);
        loop {
            let end = (offset + MAX_UPLOAD_CHUNK).min(bytes.len());
            let patch = self
                .http()
                .request(Method::PATCH, self.endpoint(&location)?)
                .header("Tus-Resumable", TUS_VERSION)
                .header("Upload-Offset", offset.to_string())
                .header(header::CONTENT_TYPE, "application/offset+octet-stream")
                .body(bytes[offset..end].to_vec());
            let response = self.authorize(patch).send().await?;
            let status = response.status();
            if !status.is_success() {
                let body = read_limited(response, 4096).await.unwrap_or_default();
                return Err(ApiError::Unexpected {
                    status,
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
            }
            offset = end;
            progress(offset as u64, total);
            if offset >= bytes.len() {
                let body = read_limited(response, MAX_RESPONSE_BYTES).await?;
                return serde_json::from_slice(&body)
                    .map_err(|source| ApiError::Decode { status, source });
            }
        }
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.bearer() {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    /// Отправка сообщения в канал пространства (с вложениями).
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn send_channel_message(
        &self,
        channel_id: &str,
        body: &SendMessageBody,
    ) -> Result<MessageWithUser, ApiError> {
        self.post_json(&format!("/api/channels/{channel_id}/messages"), body)
            .await
    }

    /// Включён ли на инстансе поиск GIF.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn gif_enabled(&self) -> Result<bool, ApiError> {
        Ok(self.get_json::<Enabled>("/api/gif/enabled").await?.enabled)
    }

    /// Популярные GIF (`query` пуст) или поиск.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn gifs(&self, query: &str, limit: u8, pos: &str) -> Result<GifPage, ApiError> {
        let path = gif_path(query, limit, pos);
        self.get_json(&path).await
    }

    /// Личные стикеры.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn stickers(&self) -> Result<Vec<PersonalSticker>, ApiError> {
        self.get_json("/api/stickers").await
    }

    /// Загружает личный стикер.
    ///
    /// # Errors
    /// Ошибки запроса (неверная картинка, коллекция полна).
    pub async fn upload_sticker(
        &self,
        name: &str,
        image: &[u8],
    ) -> Result<PersonalSticker, ApiError> {
        let body = StickerUpload {
            name,
            image: base64(image),
        };
        self.post_json("/api/stickers", &body).await
    }

    /// Загружает иконку игры для активности (`bs-server`, PR оригинала
    /// #482); возвращает ссылку на неё на этом инстансе.
    ///
    /// # Errors
    /// Ошибки запроса (сервер без маршрута отвечает 404).
    pub async fn upload_game_icon(&self, image: &[u8]) -> Result<String, ApiError> {
        #[derive(Serialize)]
        struct Upload {
            image: String,
        }
        #[derive(Deserialize)]
        struct Uploaded {
            url: String,
        }
        let uploaded: Uploaded = self
            .post_json(
                "/api/game-icons",
                &Upload {
                    image: base64(image),
                },
            )
            .await?;
        Ok(uploaded.url)
    }

    /// Удаляет личный стикер.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn delete_sticker(&self, id: &str) -> Result<serde_json::Value, ApiError> {
        self.delete_json(&format!("/api/stickers/{id}")).await
    }

    /// Переименовывает личный стикер (`PATCH /api/stickers/:id`, 1.11).
    ///
    /// # Errors
    /// Ошибки запроса; `sticker_name_invalid` — имя не годится.
    pub async fn rename_sticker(&self, id: &str, name: &str) -> Result<(), ApiError> {
        let _: serde_json::Value = self
            .patch_json(
                &format!("/api/stickers/{id}"),
                &serde_json::json!({ "name": name }),
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{base64, gif_path, location_path, tus_metadata};

    #[test]
    fn base64_matches_reference() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
    }

    #[test]
    fn gif_paths() {
        assert_eq!(gif_path("  ", 30, ""), "/api/gif/trending?limit=30");
        assert_eq!(
            gif_path("кот", 30, "p2"),
            "/api/gif/search?limit=30&pos=p2&q=%D0%BA%D0%BE%D1%82"
        );
    }

    #[test]
    fn metadata_and_location() {
        assert_eq!(tus_metadata("a", "b"), "filename YQ==,filetype Yg==");
        assert_eq!(
            location_path("/api/files/abc").as_deref(),
            Some("/api/files/abc")
        );
        assert!(location_path("https://evil/api/files/x").is_none());
        assert!(location_path("/api/files/../x").is_none());
    }
}
