//! Базовый HTTP-клиент Backspace.
//!
//! Один `ApiClient` соответствует одному инстансу (`Origin`). Токен можно
//! выставить позже через [`ApiClient::set_token`] — это удобно для логина:
//! клиент создаётся до того, как токен получен.

use std::time::Duration;

use bs_types::{Origin, TransportPolicy};
use reqwest::header::HeaderMap;
use reqwest::{Method, StatusCode, header};
use serde::Serialize;
use serde::de::DeserializeOwned;
use url::Url;
use zeroize::Zeroizing;

use crate::error::ApiError;

/// Таймаут на один HTTP-запрос по умолчанию.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// User-Agent, которым клиент представляется серверу.
pub const USER_AGENT: &str = concat!("backspace-rs/", env!("CARGO_PKG_VERSION"));

/// Максимальный размер тела ответа, который клиент согласен прочитать.
///
/// Инстанс — внешний сервер: без лимита враждебный или сломанный ответ
/// в гигабайты съел бы всю память. Реальные ответы (страница истории из
/// 100 сообщений, `ready`) на порядки меньше.
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Обрезка тела ответа в `ApiError::Unexpected`.
const BODY_SNIPPET_LIMIT: usize = 512;

/// REST-клиент одного инстанса Backspace.
///
/// `Debug` намеренно написан руками: токен не должен попасть в логи.
#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    /// База вида `https://host` (без `/api` и без хвостового слэша).
    base: String,
    origin: Origin,
    policy: TransportPolicy,
    token: Option<Zeroizing<String>>,
}

impl std::fmt::Debug for ApiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiClient")
            .field("base", &self.base)
            .field("origin", &self.origin)
            .field("policy", &self.policy)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

impl ApiClient {
    /// Создаёт клиент для указанного инстанса с политикой [`TransportPolicy::Strict`].
    ///
    /// `base` — абсолютный URL без `/api` (например, `https://chat.example.com`).
    /// Открытый `http://` принимается только для loopback.
    pub fn new(origin: Origin, base: impl Into<String>) -> Result<Self, ApiError> {
        Self::with_policy(origin, base, TransportPolicy::Strict)
    }

    /// Как [`ApiClient::new`], но с явной политикой транспорта.
    pub fn with_policy(
        origin: Origin,
        base: impl Into<String>,
        policy: TransportPolicy,
    ) -> Result<Self, ApiError> {
        let raw = base.into();
        let parsed = Url::parse(&raw)?;
        policy.check(&parsed)?;
        let trimmed = parsed.as_str().trim_end_matches('/').to_owned();
        let http = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .user_agent(USER_AGENT)
            // Редирект с токеном или паролем — классический способ увести
            // секреты на чужой хост или на открытый http. Эндпоинты API
            // редиректов не используют, так что 3xx — это ошибка.
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            http,
            base: trimmed,
            origin,
            policy,
            token: None,
        })
    }

    /// Политика транспорта, с которой создан клиент.
    pub fn policy(&self) -> TransportPolicy {
        self.policy
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
        self.token = token.map(Zeroizing::new);
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
            req = req.bearer_auth(token.as_str());
        }
        if let Some(body) = body {
            req = req.json(body);
        }

        let response = req.send().await?;
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = read_limited(response, MAX_RESPONSE_BYTES).await?;

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

    /// PATCH с JSON-телом и разбором JSON-ответа.
    pub(crate) async fn patch_json<T: DeserializeOwned, B: Serialize + Sync + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        self.request(Method::PATCH, path, Some(body)).await
    }

    /// DELETE без тела с разбором JSON-ответа.
    pub(crate) async fn delete_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        self.request::<T, ()>(Method::DELETE, path, None).await
    }
}

/// Читает тело ответа, но не больше `limit` байт.
///
/// Лишний `Content-Length` отсекается сразу, а для ответов без него (chunked)
/// размер считается по мере поступления данных.
async fn read_limited(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, ApiError> {
    if response
        .content_length()
        .is_some_and(|len| len > limit as u64)
    {
        return Err(ApiError::ResponseTooLarge { limit });
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(ApiError::ResponseTooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
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
