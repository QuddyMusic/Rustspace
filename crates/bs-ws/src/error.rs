//! Ошибки WebSocket-клиента.

use tokio_tungstenite::tungstenite;

/// Ошибка WebSocket-соединения.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WsError {
    /// Не удалось установить соединение.
    #[error("ошибка websocket: {0}")]
    Transport(#[from] tungstenite::Error),

    /// Не удалось собрать URL из базы инстанса.
    #[error("некорректный URL: {0}")]
    Url(#[from] url::ParseError),

    /// Сервер закрыл соединение до прихода `ready`.
    #[error("соединение закрыто до ready")]
    ClosedBeforeReady,

    /// Сервер прислал `error` вместо `ready` при аутентификации.
    #[error("сервер отклонил аутентификацию: {message}")]
    AuthRejected {
        /// Сообщение из события `error`.
        message: String,
        /// Машинный код, если был.
        code: Option<String>,
    },

    /// Соединение разорвано.
    #[error("соединение разорвано")]
    Disconnected,
}
