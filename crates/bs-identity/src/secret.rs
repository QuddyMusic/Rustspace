//! Секреты (JWT-токены), затираемые в памяти при уничтожении.

use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

/// JWT-токен доступа к одному инстансу.
///
/// Реализация `Debug` и `Display` маскирует значение: токен не попадёт
/// в логи случайно. Содержимое затирается нулями при `drop`.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct AccessToken(String);

impl AccessToken {
    /// Создаёт токен из строки.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Отдаёт токен как срез — для подстановки в `Authorization`.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Длина токена в байтах.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Пустой ли токен.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Намеренно не печатаем содержимое.
        write!(f, "AccessToken(<redacted>, len={})", self.0.len())
    }
}

impl fmt::Display for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl From<String> for AccessToken {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for AccessToken {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}
