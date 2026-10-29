//! Политика транспорта: когда разрешён незашифрованный `http://` / `ws://`.
//!
//! Через соединение идут пароль (при логине) и JWT (при каждом запросе и в
//! `auth` по WebSocket). Поэтому по умолчанию клиент говорит только по TLS,
//! а открытый текст допускается лишь на loopback — для локальной разработки.
//! Всё остальное требует явного согласия пользователя
//! ([`TransportPolicy::AllowInsecure`]).

use url::{Host, Url};

/// Что делать с незашифрованными адресами инстанса.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TransportPolicy {
    /// Только `https`/`wss`; `http`/`ws` лишь на loopback (`localhost`, `127.0.0.0/8`, `::1`).
    #[default]
    Strict,
    /// Пользователь сознательно разрешил открытый текст (например, инстанс в LAN).
    AllowInsecure,
}

/// Адрес не проходит политику транспорта.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// Открытый текст к нелокальному хосту без явного разрешения.
    #[error(
        "соединение с {host} без шифрования запрещено: пароль и токен ушли бы открытым текстом \
         (используйте https:// или явно разрешите http)"
    )]
    Insecure {
        /// Хост, к которому пытались подключиться.
        host: String,
    },
    /// Схема не `http`/`https`/`ws`/`wss`.
    #[error("неподдерживаемая схема адреса: {0}")]
    UnsupportedScheme(String),
    /// В адресе нет хоста.
    #[error("в адресе нет хоста")]
    MissingHost,
}

impl TransportPolicy {
    /// Проверяет `url` на соответствие политике.
    pub fn check(self, url: &Url) -> Result<(), TransportError> {
        let secure = match url.scheme() {
            "https" | "wss" => true,
            "http" | "ws" => false,
            other => return Err(TransportError::UnsupportedScheme(other.to_owned())),
        };
        let Some(host) = url.host() else {
            return Err(TransportError::MissingHost);
        };
        if secure || self == Self::AllowInsecure || is_loopback(&host) {
            return Ok(());
        }
        Err(TransportError::Insecure {
            host: host.to_string(),
        })
    }
}

/// `localhost`, `*.localhost`, `127.0.0.0/8` и `::1`.
fn is_loopback(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    }
}

#[cfg(test)]
mod tests {
    use super::{TransportError, TransportPolicy};
    use url::Url;

    fn check(policy: TransportPolicy, raw: &str) -> Result<(), TransportError> {
        policy.check(&Url::parse(raw).unwrap())
    }

    #[test]
    fn tls_is_always_allowed() {
        assert!(check(TransportPolicy::Strict, "https://chat.example.com").is_ok());
        assert!(check(TransportPolicy::Strict, "wss://chat.example.com/ws").is_ok());
    }

    #[test]
    fn plaintext_is_allowed_on_loopback_only() {
        for ok in [
            "http://localhost:3005",
            "http://app.localhost",
            "http://127.0.0.1:8080",
            "http://127.1.2.3",
            "http://[::1]:3005",
            "ws://localhost/ws",
        ] {
            assert!(check(TransportPolicy::Strict, ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://chat.example.com",
            "http://192.168.1.10:3005",
            "http://10.0.0.5",
            "ws://chat.example.com/ws",
            // Домен, лишь похожий на localhost.
            "http://localhost.evil.example",
            "http://notlocalhost",
        ] {
            assert!(
                matches!(
                    check(TransportPolicy::Strict, bad),
                    Err(TransportError::Insecure { .. })
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn explicit_consent_allows_plaintext() {
        assert!(check(TransportPolicy::AllowInsecure, "http://192.168.1.10:3005").is_ok());
    }

    #[test]
    fn other_schemes_are_rejected_even_with_consent() {
        assert!(matches!(
            check(TransportPolicy::AllowInsecure, "ftp://example.com"),
            Err(TransportError::UnsupportedScheme(_))
        ));
        assert!(matches!(
            check(TransportPolicy::AllowInsecure, "file:///etc/passwd"),
            Err(TransportError::UnsupportedScheme(_) | TransportError::MissingHost)
        ));
    }
}
