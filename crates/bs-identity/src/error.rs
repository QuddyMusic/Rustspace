//! Ошибки слоя идентичности.

use bs_api::ApiError;

/// Ошибка аутентификации/хранения учётных данных.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IdentityError {
    /// Ошибка HTTP-запроса к инстансу.
    #[error("ошибка api: {0}")]
    Api(#[from] ApiError),

    /// Не удалось собрать URL инстанса.
    #[error("некорректный url: {0}")]
    Url(#[from] url::ParseError),

    /// Для данного origin нет сохранённого токена.
    #[error("нет учётных данных для origin: {origin}")]
    NoCredentials {
        /// Origin, для которого не нашлось токена (пустая строка — Home).
        origin: String,
    },

    /// Токен есть, но сервер его отверг (401).
    #[error("токен отвергнут сервером")]
    TokenRejected,

    /// Ошибка ввода-вывода при работе с файлом токенов.
    #[error("ошибка файла токенов: {0}")]
    Io(#[from] std::io::Error),

    /// Файл токенов повреждён.
    #[error("файл токенов повреждён: {0}")]
    TokenFileCorrupted(String),
}
