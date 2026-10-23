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

    /// Сервер отверг токен; причина классифицирована по тексту.
    #[error("сервер отклонил аутентификацию ({failure:?}): {message}")]
    AuthFailed {
        /// Классифицированная причина.
        failure: crate::auth::AuthFailure,
        /// Сообщение из события `error`.
        message: String,
    },

    /// Соединение разорвано.
    #[error("соединение разорвано")]
    Disconnected,
}
