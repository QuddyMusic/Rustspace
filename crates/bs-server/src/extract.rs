//! Разбор запроса: тело JSON и адрес клиента.

use std::net::{IpAddr, SocketAddr};

use axum::Json;
use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::ratelimit::client_ip;
use crate::state::AppState;

/// Предел тела запроса (как `bodyLimit` Fastify по умолчанию — 1 МиБ).
pub const BODY_LIMIT: usize = 1_048_576;

/// Тело запроса как JSON. Обработчики сами проверяют типы полей — так же,
/// как оригинал, и с теми же кодами ошибок.
#[derive(Debug)]
pub struct JsonBody(pub Value);

fn bad_request(code: &str, message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "statusCode": 400,
            "code": code,
            "error": "Bad Request",
            "message": message,
        })),
    )
        .into_response()
}

/// Тело запроса как JSON. Предел размера — у маршрута (см. `body_limit`).
///
/// # Errors
/// Ответ, которым Fastify отвергает слишком большое, пустое или некорректное тело.
pub async fn read_json(request: Request) -> Result<Value, Response> {
    read_json_with_text(request).await.map(|(value, _)| value)
}

/// Тело запроса, разобранное на входе (этап разбора тела у Fastify).
#[derive(Debug, Clone)]
pub struct ParsedBody {
    /// Значение; `Null` — тела нет.
    pub value: Value,
    /// `JSON.stringify(тела)`; `None` — тело не JSON.
    pub text: Option<String>,
}

fn too_large() -> Response {
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        Json(json!({
            "statusCode": 413,
            "code": "FST_ERR_CTP_BODY_TOO_LARGE",
            "error": "Payload Too Large",
            "message": "Request body is too large",
        })),
    )
        .into_response()
}

fn unsupported(content_type: Option<&str>) -> Response {
    (
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        Json(json!({
            "statusCode": 415,
            "code": "FST_ERR_CTP_INVALID_MEDIA_TYPE",
            "error": "Unsupported Media Type",
            "message": format!("Unsupported Media Type: {}", content_type.unwrap_or("undefined")),
        })),
    )
        .into_response()
}

/// Каким разборщиком Fastify прочтёт тело с этим типом содержимого.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parser {
    Json,
    Text,
}

fn token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || "!#$%&'*+.^_|~-".contains(ch)
}

/// Тип содержимого без параметров, как его разбирает `fast-content-type-parse`;
/// `None` — заголовок не разбирается.
fn media_type(header: &str) -> Option<String> {
    let (kind, mut rest) = match header.find(';') {
        Some(at) => (header[..at].trim(), &header[at..]),
        None => (header.trim(), ""),
    };
    let (main, sub) = kind.split_once('/')?;
    if main.is_empty()
        || sub.is_empty()
        || !main.chars().all(token_char)
        || !sub.chars().all(token_char)
    {
        return None;
    }
    // Параметры: `; *ключ=значение *`, значение — лексема или строка в кавычках.
    while !rest.is_empty() {
        rest = rest.strip_prefix(';')?.trim_start_matches(' ');
        let eq = rest.find('=')?;
        let key = &rest[..eq];
        if key.is_empty() || !key.chars().all(token_char) {
            return None;
        }
        rest = &rest[eq + 1..];
        if let Some(quoted) = rest.strip_prefix('"') {
            let mut chars = quoted.char_indices();
            let mut end = None;
            while let Some((at, ch)) = chars.next() {
                match ch {
                    '\\' => {
                        chars.next()?;
                    }
                    '"' => {
                        end = Some(at);
                        break;
                    }
                    '\u{b}' | ' '..='\u{ff}' => {}
                    _ => return None,
                }
            }
            rest = &quoted[end? + 1..];
        } else {
            let len = rest.find(|ch: char| !token_char(ch)).unwrap_or(rest.len());
            if len == 0 {
                return None;
            }
            rest = &rest[len..];
        }
        rest = rest.trim_start_matches(' ');
    }
    Some(kind.to_ascii_lowercase())
}

fn parser_for(content_type: &str) -> Option<Parser> {
    match content_type {
        "application/json" => return Some(Parser::Json),
        "text/plain" => return Some(Parser::Text),
        _ => {}
    }
    let kind = media_type(content_type)?;
    if kind.contains("text/plain") {
        Some(Parser::Text)
    } else if kind.contains("application/json") {
        Some(Parser::Json)
    } else {
        None
    }
}

/// Наибольшее тело для маршрута (`bodyLimit` маршрута у оригинала).
fn body_limit(path: &str) -> usize {
    match path {
        "/api/stickers" => crate::routes::stickers::UPLOAD_BODY_LIMIT,
        "/api/game-icons" => crate::routes::game_icons::UPLOAD_BODY_LIMIT,
        "/api/federation/epoch"
        | "/api/federation/users/lookup"
        | "/api/federation/users/by-home-id"
        | "/api/federation/verify-attach-proof" => 4 * 1024,
        "/api/federation/relay" => 10 * 1024 * 1024,
        "/api/federation/sync" => 64 * 1024,
        _ => BODY_LIMIT,
    }
}

/// Маршруты со своими разборщиками тела (tus, отчёты CSP).
fn raw_route(path: &str) -> bool {
    path == "/api/csp-report" || path == "/api/files" || path.starts_with("/api/files/")
}

/// Разбор тела, как его делает Fastify до обработчика: выбор разборщика по
/// типу содержимого (415, если подходящего нет), предел размера (413), JSON
/// (400 с текстом V8) или текст. `found` — нашёлся ли маршрут: для
/// ненайденного неподходящий тип не отказ, а просто 404.
///
/// # Errors
/// Ответ Fastify на тело, которое не разбирается.
pub async fn parse_stage(request: Request, found: bool) -> Result<Request, Response> {
    use axum::http::{Method, header};
    let method = request.method().clone();
    let headers = request.headers();
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
    let has_length = headers.contains_key(header::CONTENT_LENGTH);
    let chunked = headers.contains_key(header::TRANSFER_ENCODING);
    let zero = headers
        .get(header::CONTENT_LENGTH)
        .is_some_and(|value| value.as_bytes() == b"0");
    let parse = if method == Method::POST || method == Method::PUT || method == Method::PATCH {
        content_type.is_some() || chunked || (has_length && !zero)
    } else if method == Method::DELETE || method == Method::OPTIONS {
        content_type.is_some() && (chunked || has_length)
    } else {
        false
    };
    let (mut parts, body) = request.into_parts();
    if !parse {
        parts.extensions.insert(ParsedBody {
            value: Value::Null,
            text: None,
        });
        return Ok(Request::from_parts(parts, body));
    }
    let Some(parser) = content_type.as_deref().and_then(parser_for) else {
        if found {
            return Err(unsupported(content_type.as_deref()));
        }
        return Ok(Request::from_parts(parts, body));
    };
    let bytes = axum::body::to_bytes(body, body_limit(parts.uri.path()))
        .await
        .map_err(|_| too_large())?;
    let parsed = match parser {
        Parser::Text => ParsedBody {
            value: Value::String(String::from_utf8_lossy(&bytes).into_owned()),
            text: None,
        },
        Parser::Json if bytes.is_empty() => {
            return Err(bad_request(
                "FST_ERR_CTP_EMPTY_JSON_BODY",
                "Body cannot be empty when content-type is set to 'application/json'",
            ));
        }
        // Ошибка разбора у Fastify — исключение `JSON.parse` или
        // `secure-json-parse`, без кода, с текстом V8.
        Parser::Json => match crate::jsjson::parse_body(&bytes) {
            Ok((value, text)) => ParsedBody {
                value,
                text: Some(text),
            },
            Err(message) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"statusCode": 400, "error": "Bad Request", "message": message})),
                )
                    .into_response());
            }
        },
    };
    parts.extensions.insert(parsed);
    Ok(Request::from_parts(parts, axum::body::Body::from(bytes)))
}

/// Промежуточный слой найденных маршрутов: тело разбирается до обработчика,
/// а значит, до проверки входа — как у Fastify.
pub async fn body_stage(request: Request, next: axum::middleware::Next) -> Response {
    if raw_route(request.uri().path()) {
        return next.run(request).await;
    }
    match parse_stage(request, true).await {
        Ok(request) => next.run(request).await,
        Err(refusal) => refusal,
    }
}

/// Как [`read_json`], но вместе с записью тела, какую дал бы
/// `JSON.stringify(request.body)`: по ней считаются подписи между инстансами.
/// Для запроса без тела записи нет.
///
/// # Errors
/// Ответ, которым Fastify отвергает слишком большое, пустое или некорректное тело.
pub async fn read_json_with_text(request: Request) -> Result<(Value, Option<String>), Response> {
    if let Some(parsed) = request.extensions().get::<ParsedBody>() {
        return Ok((parsed.value.clone(), parsed.text.clone()));
    }
    // Без этапа разбора (запрос собран вручную): тот же разбор здесь.
    let request = parse_stage(request, true).await?;
    let parsed = request
        .extensions()
        .get::<ParsedBody>()
        .cloned()
        .unwrap_or(ParsedBody {
            value: Value::Null,
            text: None,
        });
    Ok((parsed.value, parsed.text))
}

/// Отказ проверки по схеме маршрута (`FST_ERR_VALIDATION`).
#[must_use]
pub fn schema_error(message: &str) -> Response {
    bad_request("FST_ERR_VALIDATION", message)
}

impl FromRequest<AppState> for JsonBody {
    type Rejection = Response;

    async fn from_request(request: Request, _state: &AppState) -> Result<Self, Self::Rejection> {
        read_json(request).await.map(Self)
    }
}

/// Адрес клиента с учётом доверенных прокси.
#[derive(Debug, Clone, Copy)]
pub struct ClientIp(pub IpAddr);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let socket = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map_or_else(|| SocketAddr::from(([127, 0, 0, 1], 0)), |info| info.0);
        Ok(Self(client_ip(
            &parts.headers,
            socket,
            state.config.trusted_proxy_hops,
        )))
    }
}

#[cfg(test)]
mod body_tests {
    use super::*;

    #[test]
    fn content_types_pick_the_fastify_parser() {
        // Сверено с Fastify 4.29 (`fast-content-type-parse` 1.1).
        for (header, parser) in [
            ("application/json", Some(Parser::Json)),
            ("Application/Json", Some(Parser::Json)),
            (" application/json", Some(Parser::Json)),
            ("application/json; charset=utf-8", Some(Parser::Json)),
            ("application/json; charset=\"utf-8\"", Some(Parser::Json)),
            ("application/jsonx", Some(Parser::Json)),
            ("x-application/json", Some(Parser::Json)),
            ("application/json-patch+json", Some(Parser::Json)),
            ("text/plain", Some(Parser::Text)),
            ("text/plainx", Some(Parser::Text)),
            ("text/plain; charset=utf-8", Some(Parser::Text)),
            ("application/json; charset", None),
            ("text/plain;application/json", None),
            ("application/vnd.api+json", None),
            ("application/xml", None),
            ("multipart/form-data", None),
            ("application/x-www-form-urlencoded", None),
            ("json", None),
            ("", None),
        ] {
            assert_eq!(parser_for(header), parser, "{header:?}");
        }
    }
}
