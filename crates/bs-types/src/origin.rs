//! Понятие origin для мультиинстансности.
//!
//! В вебе origin — это строка: `''` для домашнего инстанса, `'https://...'`
//! для удалённых. Мы моделируем это enum-ом, чтобы исключить ошибки.

use serde::{Deserialize, Serialize};
use url::Url;

/// Источник (инстанс) Backspace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Домашний инстанс пользователя (в вебе — `''`).
    Home,
    /// Удалённый инстанс с абсолютным URL.
    Remote(Url),
}

impl Origin {
    /// Строковое представление как в вебе: `''` для Home, абсолютный URL для Remote.
    pub fn as_web_str(&self) -> String {
        match self {
            Self::Home => String::new(),
            Self::Remote(url) => url.as_str().trim_end_matches('/').to_owned(),
        }
    }

    /// Разбор строки из веб-формата: `''` → Home, иначе — Remote.
    pub fn parse_web_str(s: &str) -> Result<Self, url::ParseError> {
        if s.is_empty() {
            Ok(Self::Home)
        } else {
            Url::parse(s).map(Self::Remote)
        }
    }

    /// Удобный предикат.
    pub fn is_home(&self) -> bool {
        matches!(self, Self::Home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_roundtrip() {
        let origin = Origin::parse_web_str("").unwrap();
        assert_eq!(origin, Origin::Home);
        assert_eq!(origin.as_web_str(), "");
    }

    #[test]
    fn remote_roundtrip() {
        let origin = Origin::parse_web_str("https://chat.example.com").unwrap();
        assert!(matches!(origin, Origin::Remote(_)));
        assert_eq!(origin.as_web_str(), "https://chat.example.com");
    }

    #[test]
    fn strips_trailing_slash() {
        let origin = Origin::parse_web_str("https://chat.example.com/").unwrap();
        assert_eq!(origin.as_web_str(), "https://chat.example.com");
    }
}
