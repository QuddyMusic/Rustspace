//! Превью трансляции: маленький кадр того, кто показывает экран в голосовом
//! канале (`/api/rustspace/voice/{channel}/preview`).

use reqwest::{StatusCode, header};

use crate::client::{ApiClient, read_limited};
use crate::error::ApiError;

/// Самый большой кадр превью, который примет сервер.
pub const MAX_PREVIEW_BYTES: usize = 256 * 1024;

impl ApiClient {
    /// Кладёт свой свежий кадр (JPEG) в канал, где мы сейчас транслируем.
    ///
    /// # Errors
    /// Ошибки запроса или отказ сервера.
    pub async fn put_stream_preview(
        &self,
        channel_id: &str,
        jpeg: Vec<u8>,
    ) -> Result<(), ApiError> {
        let url = self.endpoint(&format!("/api/rustspace/voice/{channel_id}/preview"))?;
        let mut request = self
            .http()
            .put(url)
            .header(header::CONTENT_TYPE, "image/jpeg")
            .body(jpeg);
        if let Some(token) = self.bearer() {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = read_limited(response, 4096).await.unwrap_or_default();
        Err(ApiError::Unexpected {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }

    /// Свежий кадр трансляции пользователя в канале; `None` — кадра нет
    /// (не транслирует, старый клиент или сервер без превью).
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn stream_preview(
        &self,
        channel_id: &str,
        user_id: &str,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        let url = self.endpoint(&format!(
            "/api/rustspace/voice/{channel_id}/preview/{user_id}"
        ))?;
        let mut request = self.http().get(url).header(header::ACCEPT, "image/*");
        if let Some(token) = self.bearer() {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        let status = response.status();
        if matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ) {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(ApiError::Unexpected {
                status,
                body: String::new(),
            });
        }
        read_limited(response, MAX_PREVIEW_BYTES).await.map(Some)
    }
}
