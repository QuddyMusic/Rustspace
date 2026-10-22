//! Базовый HTTP-клиент Backspace.
//!
//! Один `ApiClient` соответствует одному инстансу (`Origin`). Токен можно
//! выставить позже через [`ApiClient::set_token`] — это удобно для логина:
//! клиент создаётся до того, как токен получен.

use std::time::Duration;

use bs_types::Origin;
use reqwest::header::HeaderMap;
use reqwest::{Method, StatusCode, header};
use serde::Serialize;
use serde::de::DeserializeOwned;
use url::Url;

use crate::error::ApiError;

/// Таймаут на один HTTP-запрос по умолчанию.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// User-Agent, которым клиент представляется серверу.
pub const USER_AGENT: &str = concat!("backspace-rs/", env!("CARGO_PKG_VERSION"));

/// Обрезка тела ответа в `ApiError::Unexpected`.
const BODY_SNIPPET_LIMIT: usize = 512;

/// REST-клиент одного инстанса Backspace.
#[derive(Debug, Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    /// База вида `https://host` (без `/api` и без хвостового слэша).
    base: String,
    origin: Origin,
    token: Option<String>,
}

impl ApiClient {
    /// Создаёт клиент для указанного инстанса.
    ///
    /// `base` — абсолютный URL без `/api` (например, `https://chat.example.com`).
    pub fn new(origin: Origin, base: impl Into<String>) -> Result<Self, ApiError> {
        let raw = base.into();
        let parsed = Url::parse(&raw)?;
        let trimmed = parsed.as_str().trim_end_matches('/').to_owned();
        let http = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()?;
        Ok(Self {
            http,
            base: trimmed,
            origin,
            token: None,
        })
    }

    /// Создаёт клиент для домашнего инстанса по его базовому URL.
    pub fn home(base: impl Into<String>) -> Result<Self, ApiError> {
        Self::new(Origin::Home, base)
    }

    /// Инстанс, к которому привязан клиент.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// Базовый URL без хвостового слэша.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Устанавливает (или сбрасывает) Bearer-токен.
    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }

    /// `true`, если клиент сейчас авторизован.
    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    /// Собирает абсолютный URL эндпоинта. `path` начинается со `/` и уже
    /// включает `/api` (например, `/api/auth/login`).
    fn endpoint(&self, path: &str) -> Result<Url, ApiError> {
        let joined = format!("{}{}", self.base, path);
        Ok(Url::parse(&joined)?)
    }

    /// Универсальный запрос: отправляет `body` (если есть) и разбирает JSON-ответ.
    pub(crate) async fn request<T, B>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, ApiError>
    where
        T: DeserializeOwned,
        B: Serialize + Sync + ?Sized,
    {
        let (value, _headers) = self
            .request_with_headers::<T, B>(method, path, body)
            .await?;
        Ok(value)
    }

    /// Как [`ApiClient::request`], но дополнительно возвращает заголовки ответа.
    pub(crate) async fn request_with_headers<T, B>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<(T, HeaderMap), ApiError>
    where
        T: DeserializeOwned,
        B: Serialize + Sync + ?Sized,
    {
        let url = self.endpoint(path)?;
        let mut req = self
            .http
            .request(method, url)
            .header(header::ACCEPT, "application/json");
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        if let Some(body) = body {
            req = req.json(body);
        }

        let response = req.send().await?;
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.bytes().await?;

        if status.is_success() {
            let value = serde_json::from_slice(&bytes)
                .map_err(|source| ApiError::Decode { status, source })?;
            Ok((value, headers))
        } else {
            Err(decode_error(status, &bytes))
        }
    }

    /// GET с разбором JSON.
    pub(crate) async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        self.request::<T, ()>(Method::GET, path, None).await
    }

    /// GET с разбором JSON и заголовками ответа.
    pub(crate) async fn get_json_with_headers<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<(T, HeaderMap), ApiError> {
        self.request_with_headers::<T, ()>(Method::GET, path, None)
            .await
    }

    /// POST с JSON-телом и разбором JSON-ответа.
    pub(crate) async fn post_json<T: DeserializeOwned, B: Serialize + Sync + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        self.request(Method::POST, path, Some(body)).await
    }
}

/// Разбирает тело не-2xx ответа в [`ApiError`].
fn decode_error(status: StatusCode, bytes: &[u8]) -> ApiError {
    serde_json::from_slice::<bs_types::ApiErrorBody>(bytes).map_or_else(
        |_| {
            let text = String::from_utf8_lossy(bytes);
            let snippet = if text.chars().count() > BODY_SNIPPET_LIMIT {
                let truncated: String = text.chars().take(BODY_SNIPPET_LIMIT).collect();
                format!("{truncated}…")
            } else {
                text.into_owned()
            };
            ApiError::Unexpected {
                status,
                body: snippet,
            }
        },
        |body| ApiError::Server { status, body },
    )
}
