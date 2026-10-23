//! Ошибки реестра инстансов и фоновых задач.

use bs_identity::IdentityError;
use bs_ws::WsError;

/// Ошибка подключения/работы инстанса.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InstanceError {
    /// Ошибка слоя идентичности (логин, хранение токена).
    #[error("ошибка идентичности: {0}")]
    Identity(#[from] IdentityError),

    /// Ошибка WebSocket-соединения.
    #[error("ошибка websocket: {0}")]
    Ws(#[from] WsError),

    /// Ошибка REST-запроса (например, `reattach`).
    #[error("ошибка api: {0}")]
    Api(#[from] bs_api::ApiError),

    /// Инстанс с таким origin не найден в реестре.
    #[error("инстанс не найден: {origin}")]
    UnknownInstance {
        /// Origin запрошенного инстанса.
        origin: String,
    },

    /// Фоновый диспетчер уже завершился.
    #[error("диспетчер инстанса остановлен")]
    DispatcherClosed,

    /// Не удалось запустить async-задачу.
    #[error("не удалось запустить задачу: {0}")]
    Join(#[from] tokio::task::JoinError),
}
