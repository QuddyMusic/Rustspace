//! Ошибки WebSocket-клиента.

/// Ошибка WebSocket-соединения.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WsError {
    /// Не удалось установить соединение или сокет сломался.
    #[error("ошибка websocket: {0}")]
    Transport(String),

    /// Не удалось собрать URL из базы инстанса.
    #[error("некорректный URL: {0}")]
    Url(#[from] url::ParseError),

    /// Адрес не проходит политику транспорта (открытый `ws://`).
    #[error(transparent)]
    Insecure(#[from] bs_types::TransportError),

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

#[cfg(not(target_arch = "wasm32"))]
impl From<tokio_tungstenite::tungstenite::Error> for WsError {
    fn from(err: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::Transport(err.to_string())
    }
}
