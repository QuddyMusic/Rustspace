//! Ошибки слоя состояния.

use bs_instances::InstanceError;

/// Ошибка сессии/кэша.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CoreError {
    /// Ошибка подключения к инстансу.
    #[error("ошибка инстанса: {0}")]
    Instance(#[from] InstanceError),

    /// Поток событий инстанса закрылся (диспетчер остановлен).
    #[error("поток событий закрыт")]
    EventStreamClosed,

    /// Фоновый насос событий уже остановлен.
    #[error("насос событий остановлен")]
    PumpStopped,

    /// Домашний инстанс не является удалённым.
    #[error("домашний инстанс не является удалённым")]
    HomeCredentialNotRemote,

    /// Нет сохранённых учётных данных для инстанса.
    #[error("нет учётных данных: {0}")]
    Credentials(#[from] bs_identity::IdentityError),
}
