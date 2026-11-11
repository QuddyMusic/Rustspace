//! Исходящие запросы к чужим адресам из сообщений: защита от обращения во
//! внутреннюю сеть (`utils/ssrf.ts`, `utils/ipClass.ts`).
//!
//! Оригинал проверяет адрес отдельным запросом DNS, а соединяется по имени:
//! между проверкой и соединением имя может начать указывать на другой адрес.
//! Здесь имя разрешается один раз, проверяются все полученные адреса, и
//! соединение идёт строго на проверенный.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use reqwest::header::{HeaderMap, LOCATION};
use url::{Host, Url};

/// Сколько перенаправлений проходит запрос.
const MAX_REDIRECTS: usize = 5;

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 127
        || a == 10
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 169 && b == 254)
        || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn public_v6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return public_v4(mapped);
    }
    let groups = ip.segments();
    if ip.is_unspecified() || ip.is_loopback() {
        return false;
    }
    let first = groups[0];
    if first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80 || first & 0xff00 == 0xff00 {
        return false;
    }
    // Адреса, внутри которых лежит IPv4 (NAT64, 6to4, совместимые): судим по
    // вложенному адресу — иначе через них достижима внутренняя сеть.
    let embedded = |hi: u16, lo: u16| {
        Ipv4Addr::new(
            (hi >> 8) as u8,
            (hi & 0xff) as u8,
            (lo >> 8) as u8,
            (lo & 0xff) as u8,
        )
    };
    if groups[..6] == [0x64, 0xff9b, 0, 0, 0, 0] || groups[..6] == [0, 0, 0, 0, 0, 0] {
        return public_v4(embedded(groups[6], groups[7]));
    }
    if first == 0x2002 {
        return public_v4(embedded(groups[1], groups[2]));
    }
    true
}

/// Публичный ли адрес (`classifyAddress(ip) === 'public'`).
#[must_use]
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => public_v6(ip),
    }
}

/// Ответ внешнего сервера.
#[derive(Debug)]
pub struct Fetched {
    /// Код ответа.
    pub status: u16,
    /// Заголовки.
    pub headers: HeaderMap,
    /// Тело, читается по частям.
    pub response: reqwest::Response,
}

/// Куда соединяться для адреса: имя узла и проверенный адрес (для имени).
async fn checked_target(url: &Url) -> Option<Option<(String, SocketAddr)>> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let port = url.port_or_known_default()?;
    match url.host()? {
        Host::Ipv4(ip) => public_v4(ip).then_some(None),
        Host::Ipv6(ip) => public_v6(ip).then_some(None),
        Host::Domain(name) => {
            let found: Vec<SocketAddr> =
                tokio::net::lookup_host((name, port)).await.ok()?.collect();
            if found.is_empty() || !found.iter().all(|address| is_public(address.ip())) {
                return None;
            }
            Some(Some((name.to_owned(), found[0])))
        }
    }
}

/// Запрос `GET` к внешнему адресу с проверкой каждого шага перенаправления
/// (`safeFetch`). `None` — адрес недопустим, недоступен или ответ не получен
/// за `timeout`.
pub async fn safe_get(url: &str, headers: HeaderMap, timeout: Duration) -> Option<Fetched> {
    let work = async {
        let mut current = Url::parse(url).ok()?;
        for _ in 0..=MAX_REDIRECTS {
            let target = checked_target(&current).await?;
            let mut builder = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .connect_timeout(timeout)
                .user_agent("BackspaceBot/1.0");
            if let Some((name, address)) = target {
                builder = builder.resolve(&name, address);
            }
            let client = builder.build().ok()?;
            let response = client
                .get(current.clone())
                .headers(headers.clone())
                .send()
                .await
                .ok()?;
            let status = response.status().as_u16();
            if (300..400).contains(&status)
                && let Some(location) = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
            {
                current = current.join(location).ok()?;
                continue;
            }
            return Some(Fetched {
                status,
                headers: response.headers().clone(),
                response,
            });
        }
        None
    };
    tokio::time::timeout(timeout, work).await.ok().flatten()
}

/// Читает тело ответа, но не больше `limit` байт. Второе значение — тело
/// оказалось длиннее.
pub async fn read_limited(
    mut response: reqwest::Response,
    limit: usize,
    timeout: Duration,
) -> (Vec<u8>, bool) {
    let mut body = Vec::new();
    let read = async {
        while let Ok(Some(chunk)) = response.chunk().await {
            if body.len() + chunk.len() > limit {
                let room = limit - body.len();
                body.extend_from_slice(&chunk[..room]);
                return true;
            }
            body.extend_from_slice(&chunk);
        }
        false
    };
    let truncated = tokio::time::timeout(timeout, read).await.unwrap_or(false);
    (body, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public(text: &str) -> bool {
        text.parse().is_ok_and(is_public)
    }

    #[test]
    fn inner_networks_are_not_public() {
        for address in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "0.1.2.3",
            "192.0.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "198.51.100.7",
            "203.0.113.9",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "2002:7f00:1::1",
            "::7f00:1",
        ] {
            assert!(!public(address), "{address}");
        }
        for address in [
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "2606:4700::1111",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
        ] {
            assert!(public(address), "{address}");
        }
    }

    #[tokio::test]
    async fn local_targets_are_refused_before_any_connection() {
        let timeout = Duration::from_secs(2);
        for url in [
            "http://127.0.0.1:1/x",
            "http://localhost:1/x",
            "http://[::1]:1/x",
            "http://169.254.169.254/latest/meta-data",
            "http://0x7f.1/x",
            "http://2130706433/x",
            "ftp://example.com/x",
            "file:///etc/passwd",
            "not a url",
        ] {
            assert!(
                safe_get(url, HeaderMap::new(), timeout).await.is_none(),
                "{url}"
            );
        }
    }
}
