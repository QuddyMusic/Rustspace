//! Ограничение частоты запросов по адресу клиента, как `@fastify/rate-limit`
//! оригинала: общий предел и свои пределы у чувствительных маршрутов.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

use crate::error::ApiError;

/// Предел: не больше `max` запросов за `window`.
#[derive(Debug, Clone, Copy)]
pub struct Limit {
    /// Имя счётчика (у каждого маршрута с пределом — своё).
    pub name: &'static str,
    /// Сколько запросов.
    pub max: u32,
    /// За какое время.
    pub window: Duration,
}

impl Limit {
    /// Общий предел сервера: 200 в минуту.
    pub const GLOBAL: Self = Self {
        name: "global",
        max: 200,
        window: Duration::from_mins(1),
    };
}

/// (счётчик, адрес) → (начало окна, сколько запросов).
type Hits = HashMap<(&'static str, IpAddr), (Instant, u32)>;

/// Счётчики по адресам.
#[derive(Debug, Clone, Default)]
pub struct RateLimiter {
    disabled: bool,
    hits: Arc<Mutex<Hits>>,
}

impl RateLimiter {
    /// Ограничитель; `disabled` — для тестовых стендов (`DISABLE_RATE_LIMITS`).
    #[must_use]
    pub fn new(disabled: bool) -> Self {
        Self {
            disabled,
            hits: Arc::default(),
        }
    }

    /// Учитывает запрос.
    ///
    /// # Errors
    /// Предел исчерпан: 429 с `retryAfter` в секундах.
    pub fn check(&self, limit: Limit, ip: IpAddr) -> Result<(), ApiError> {
        if self.disabled {
            return Ok(());
        }
        let now = Instant::now();
        let mut hits = self.hits.lock().unwrap_or_else(PoisonError::into_inner);
        // Чтобы таблица не росла без конца, изредка выметаем старое.
        if hits.len() > 50_000 {
            hits.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_mins(15));
        }
        let entry = hits.entry((limit.name, ip)).or_insert((now, 0));
        if now.duration_since(entry.0) >= limit.window {
            *entry = (now, 0);
        }
        entry.1 += 1;
        let (start, count) = *entry;
        drop(hits);
        if count > limit.max {
            let left = limit.window.saturating_sub(now.duration_since(start));
            return Err(
                ApiError::new(axum::http::StatusCode::TOO_MANY_REQUESTS, "rate_limited")
                    .extra("retryAfter", left.as_secs().max(1)),
            );
        }
        Ok(())
    }
}

/// Адрес клиента с учётом доверенных прокси (`trustProxy: N` у Fastify):
/// доверяем `hops` ближайшим узлам, считая от сокета; первый недоверенный
/// адрес в цепочке и есть клиент.
#[must_use]
pub fn client_ip(headers: &HeaderMap, socket: SocketAddr, hops: u32) -> IpAddr {
    let forwarded = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|part| part.trim().parse::<IpAddr>().ok());
    // Цепочка от ближайшего узла к дальнему: сокет, затем XFF справа налево.
    let chain: Vec<IpAddr> = std::iter::once(socket.ip())
        .chain(forwarded.rev())
        .collect();
    let index = (hops as usize).min(chain.len() - 1);
    chain[index]
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    fn headers(xff: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert("x-forwarded-for", xff.parse().expect("заголовок"));
        map
    }

    #[test]
    fn client_ip_trusts_only_configured_hops() {
        let socket: SocketAddr = "10.0.0.1:5000".parse().expect("адрес");
        let chain = headers("203.0.113.9, 198.51.100.7");
        // Без доверия заголовок не важен — его мог написать сам клиент.
        assert_eq!(client_ip(&chain, socket, 0), socket.ip());
        assert_eq!(client_ip(&chain, socket, 1).to_string(), "198.51.100.7");
        assert_eq!(client_ip(&chain, socket, 2).to_string(), "203.0.113.9");
        assert_eq!(client_ip(&chain, socket, 4).to_string(), "203.0.113.9");
        assert_eq!(client_ip(&HeaderMap::new(), socket, 1), socket.ip());
    }

    #[test]
    fn limit_blocks_after_max_and_separates_clients() {
        let limiter = RateLimiter::new(false);
        let limit = Limit {
            name: "test",
            max: 2,
            window: Duration::from_mins(1),
        };
        let a: IpAddr = "192.0.2.1".parse().expect("адрес");
        let b: IpAddr = "192.0.2.2".parse().expect("адрес");
        assert!(limiter.check(limit, a).is_ok());
        assert!(limiter.check(limit, a).is_ok());
        let blocked = limiter.check(limit, a).expect_err("предел");
        assert_eq!(blocked.code(), "rate_limited");
        assert!(limiter.check(limit, b).is_ok());
        assert!(RateLimiter::new(true).check(limit, a).is_ok());
    }
}
