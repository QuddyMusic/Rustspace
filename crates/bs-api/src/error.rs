//! Ошибки REST-клиента.
//!
//! Сервер отдаёт ошибки как `{ error, code?, details?, statusCode? }`
//! (см. `bs_types::ApiErrorBody`). Мы сохраняем тело целиком, чтобы
//! вызывающий код реагировал на `code`, а не на текст сообщения.

use bs_types::ApiErrorBody;
use reqwest::StatusCode;

/// Ошибка обращения к REST API Backspace.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ApiError {
    /// Транспортная ошибка: соединение, TLS, таймаут и т.п.
    #[error("сетевая ошибка: {0}")]
    Transport(#[from] reqwest::Error),

    /// Не удалось собрать URL эндпоинта из базового адреса.
    #[error("некорректный URL: {0}")]
    Url(#[from] url::ParseError),

    /// Сервер ответил не-2xx и прислал разбираемое тело ошибки.
    #[error("сервер вернул HTTP {status}: {}", body.error)]
    Server {
        /// HTTP-статус ответа.
        status: StatusCode,
        /// Тело ошибки, как его прислал сервер.
        body: ApiErrorBody,
    },

    /// Ответ 2xx не разобрался как ожидаемый тип.
    #[error("не удалось разобрать ответ (HTTP {status}): {source}")]
    Decode {
        /// HTTP-статус ответа.
        status: StatusCode,
        /// Ошибка serde.
        #[source]
        source: serde_json::Error,
    },

    /// Не-2xx ответ с телом, которое не является `ApiErrorBody`.
    #[error("неожиданный ответ HTTP {status}: {body}")]
    Unexpected {
        /// HTTP-статус ответа.
        status: StatusCode,
        /// Начало тела ответа (обрезано).
        body: String,
    },
}

impl ApiError {
    /// HTTP-статус, если он известен.
    pub fn status(&self) -> Option<StatusCode> {
        match self {
            Self::Server { status, .. }
            | Self::Decode { status, .. }
            | Self::Unexpected { status, .. } => Some(*status),
            Self::Transport(err) => err.status(),
            Self::Url(_) => None,
        }
    }

    /// Машинный код ошибки из тела ответа (`ApiErrorBody::code`).
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Server { body, .. } => body.code.as_deref(),
            _ => None,
        }
    }

    /// Тело ошибки, если сервер прислал разбираемое.
    pub fn body(&self) -> Option<&ApiErrorBody> {
        match self {
            Self::Server { body, .. } => Some(body),
            _ => None,
        }
    }

    /// `true`, если сервер ответил 401 (токен отсутствует или протух).
    pub fn is_unauthorized(&self) -> bool {
        self.status() == Some(StatusCode::UNAUTHORIZED)
    }

    /// `true`, если сервер ответил 429 (сработал rate limit).
    pub fn is_rate_limited(&self) -> bool {
        self.status() == Some(StatusCode::TOO_MANY_REQUESTS)
    }
}
