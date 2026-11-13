//! Запросы к другим инстансам (`utils/federationFetch.ts`, `federationEpoch.ts`,
//! `federationRecovery.ts`).

use std::net::SocketAddr;
use std::time::Duration;

use axum::http::HeaderMap;
use serde_json::Value;
use url::{Host, Url};

use crate::config::Config;
use crate::federation_auth;
use crate::state::now_ms;

/// Сколько ждать ответа на рукопожатие и проверки.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// Наибольший ответ пира, который читается целиком.
const MAX_ANSWER: usize = 4 * 1024 * 1024;

/// Насколько мы доверяем адресу.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// Адрес одобрил администратор или с ним уже есть пиринг.
    Approved,
    /// Адрес назвал посторонний: он обязан быть публичным.
    Asserted,
}

/// Почему запрос не состоялся.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// Ответа не было за отведённое время.
    Timeout,
    /// Адрес недопустим или соединение не удалось.
    Failed(String),
}

/// Ответ пира.
#[derive(Debug, Clone)]
pub struct Answer {
    /// Код ответа.
    pub status: u16,
    /// Заголовки.
    pub headers: HeaderMap,
    /// Тело.
    pub body: Vec<u8>,
}

impl Answer {
    /// Ответ с кодом 2xx.
    #[must_use]
    pub const fn ok(&self) -> bool {
        self.status >= 200 && self.status < 300
    }

    /// Тело как объект JSON; всё остальное — пустой объект.
    #[must_use]
    pub fn object(&self) -> serde_json::Map<String, Value> {
        match serde_json::from_slice(&self.body) {
            Ok(Value::Object(fields)) => fields,
            _ => serde_json::Map::new(),
        }
    }
}

/// Адрес инстанса в каноническом виде (`validateOrigin`): только `https`,
/// а `http` — лишь для `localhost` и `127.0.0.1`.
#[must_use]
pub fn validate_origin(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    match url.scheme() {
        "https" => {}
        "http" if matches!(url.host_str(), Some("localhost" | "127.0.0.1")) => {}
        _ => return None,
    }
    url.host()?;
    Some(url.origin().ascii_serialization())
}

/// Проверяет адрес пира и возвращает, куда соединяться
/// (`assertPeerOriginAllowed`). Для адреса, названного посторонним, имя
/// разрешается здесь же, и запрос идёт на проверенный адрес.
async fn checked(
    config: &Config,
    origin: &str,
    trust: Trust,
) -> Result<(Url, Option<(String, SocketAddr)>), String> {
    let url = Url::parse(origin).map_err(|_| format!("Invalid peer origin: {origin}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("Invalid peer origin scheme: {}:", url.scheme()));
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err("Invalid peer origin: must carry no path, query or fragment".to_owned());
    }
    if trust == Trust::Approved || config.federation_allow_private_peers {
        return Ok((url, None));
    }
    let shown = url.origin().ascii_serialization();
    let port = url.port_or_known_default().unwrap_or(443);
    let private = || format!("Peer origin is not reachable on the public internet: {shown}");
    let pinned = match url.host() {
        Some(Host::Ipv4(ip)) => {
            if !crate::net::is_public(ip.into()) {
                return Err(private());
            }
            None
        }
        Some(Host::Ipv6(ip)) => {
            if !crate::net::is_public(ip.into()) {
                return Err(private());
            }
            None
        }
        Some(Host::Domain(name)) => {
            let found: Vec<SocketAddr> = tokio::net::lookup_host((name, port))
                .await
                .map(Iterator::collect)
                .unwrap_or_default();
            let Some(first) = found.first().copied() else {
                return Err(format!("Peer origin did not resolve: {shown}"));
            };
            if !crate::net::is_public(first.ip()) {
                return Err(private());
            }
            Some((name.to_owned(), first))
        }
        None => return Err(format!("Invalid peer origin: {origin}")),
    };
    Ok((url, pinned))
}

/// Запрос к пиру (`federationFetch`): без перенаправлений, с пределом времени.
///
/// # Errors
/// Адрес недопустим, соединение не удалось или ответ не пришёл вовремя.
pub async fn request(
    config: &Config,
    origin: &str,
    path: &str,
    body: Option<String>,
    headers: &[(&'static str, String)],
    trust: Trust,
    timeout: Duration,
) -> Result<Answer, FetchError> {
    let method = if body.is_some() {
        reqwest::Method::POST
    } else {
        reqwest::Method::GET
    };
    request_with(config, method, origin, path, body, headers, trust, timeout).await
}

/// Как [`request`], с явным методом.
///
/// # Errors
/// Как у [`request`].
#[allow(clippy::too_many_arguments, reason = "поля запроса")]
pub async fn request_with(
    config: &Config,
    method: reqwest::Method,
    origin: &str,
    path: &str,
    body: Option<String>,
    headers: &[(&'static str, String)],
    trust: Trust,
    timeout: Duration,
) -> Result<Answer, FetchError> {
    let work = async {
        let (base, pinned) = checked(config, origin, trust).await?;
        let target = base.join(path).map_err(|error| error.to_string())?;
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(timeout);
        if let Some((name, address)) = pinned {
            builder = builder.resolve(&name, address);
        }
        if let Some(pem) = &config.extra_ca_certs {
            for certificate in
                reqwest::Certificate::from_pem_bundle(pem).map_err(|error| error.to_string())?
            {
                builder = builder.add_root_certificate(certificate);
            }
        }
        let client = builder.build().map_err(|error| error.to_string())?;
        let mut call = client.request(method, target);
        for (name, value) in headers {
            call = call.header(*name, value);
        }
        if let Some(body) = body {
            call = call.body(body);
        }
        // Node.js на любую сетевую ошибку отвечает одним текстом; подробности
        // остаются в журнале.
        let response = call.send().await.map_err(|error| {
            tracing::debug!(origin, path, ?error, "запрос к пиру не удался");
            "fetch failed".to_owned()
        })?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let (body, _) = crate::net::read_limited(response, MAX_ANSWER, timeout).await;
        Ok::<_, String>(Answer {
            status,
            headers,
            body,
        })
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(text)) => Err(FetchError::Failed(text)),
        Err(_) => Err(FetchError::Timeout),
    }
}

/// Подписанный запрос `POST` с телом JSON.
///
/// # Errors
/// Как у [`request`].
pub async fn signed_post(
    config: &Config,
    origin: &str,
    path: &str,
    body: &Value,
    secret: &str,
    trust: Trust,
) -> Result<Answer, FetchError> {
    let text = crate::jsjson::stringify(body);
    let ours = crate::federation::our_origin(config);
    let headers = federation_auth::headers(&text, secret, &ours, now_ms());
    request(config, origin, path, Some(text), &headers, trust, TIMEOUT).await
}

/// Итог подписанной проверки эпохи пира.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Epoch {
    /// Пир ответил и подписал ответ нашим общим секретом.
    Verified(String),
    /// Пир нас не знает.
    NotPeered,
    /// Секреты не совпадают.
    SecretMismatch,
    /// Ответа нет или он непонятен.
    Unknown,
}

/// Спрашивает у пира его эпоху и проверяет подпись ответа (`probeEpoch`).
pub async fn probe_epoch(config: &Config, origin: &str, secret: &str) -> Epoch {
    let Ok(answer) = signed_post(
        config,
        origin,
        "/api/federation/epoch",
        &serde_json::json!({}),
        secret,
        Trust::Approved,
    )
    .await
    else {
        return Epoch::Unknown;
    };
    match answer.status {
        403 => return Epoch::NotPeered,
        401 => return Epoch::SecretMismatch,
        _ if !answer.ok() => return Epoch::Unknown,
        _ => {}
    }
    let header = |name: &str| {
        answer
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let signature = header("x-federation-signature").unwrap_or_default();
    let signature = signature.strip_prefix("sha256=").unwrap_or(signature);
    let timestamp: Option<i64> =
        header("x-federation-timestamp").and_then(|text| text.parse().ok());
    let (Ok(text), Some(timestamp)) = (std::str::from_utf8(&answer.body), timestamp) else {
        return Epoch::Unknown;
    };
    let nonce = header("x-federation-nonce");
    if !federation_auth::verify(text, signature, secret, timestamp, nonce, now_ms()) {
        return Epoch::Unknown;
    }
    match answer.object().get("instanceId").and_then(Value::as_str) {
        Some(id) if !id.is_empty() => Epoch::Verified(id.to_owned()),
        _ => Epoch::Unknown,
    }
}

/// Отвечает ли пир и какая у него эпоха (`probePeerReachable`).
pub async fn probe_reachable(config: &Config, origin: &str) -> (bool, Option<String>) {
    let answer = request(
        config,
        origin,
        "/api/instance/info",
        None,
        &[],
        Trust::Approved,
        TIMEOUT,
    )
    .await;
    match answer {
        Ok(answer) if answer.ok() => {
            let epoch = answer
                .object()
                .get("instanceId")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned);
            (true, epoch)
        }
        _ => (false, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_is_canonical_and_https_only() {
        assert_eq!(
            validate_origin("https://Chat.Example:443/path?x#y").as_deref(),
            Some("https://chat.example")
        );
        assert_eq!(
            validate_origin("https://chat.example:8443").as_deref(),
            Some("https://chat.example:8443")
        );
        assert_eq!(
            validate_origin("http://localhost:3105/").as_deref(),
            Some("http://localhost:3105")
        );
        assert_eq!(
            validate_origin("http://127.0.0.1:80").as_deref(),
            Some("http://127.0.0.1")
        );
        for bad in [
            "http://chat.example",
            "ftp://chat.example",
            "chat.example",
            "",
            "https://",
        ] {
            assert!(validate_origin(bad).is_none(), "{bad}");
        }
    }
}
