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

/// Читает тело запроса как JSON, не больше `limit` байт.
///
/// # Errors
/// Ответ, которым Fastify отвергает слишком большое, пустое или некорректное тело.
pub async fn read_json(request: Request, limit: usize) -> Result<Value, Response> {
    // Запрос без тела и без типа содержимого Fastify передаёт дальше как
    // «тела нет»: отказ даёт уже проверка по схеме.
    let untyped = !request
        .headers()
        .contains_key(axum::http::header::CONTENT_TYPE);
    let bytes = axum::body::to_bytes(request.into_body(), limit)
        .await
        .map_err(|_| {
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
        })?;
    if bytes.is_empty() && untyped {
        return Ok(Value::Null);
    }
    if bytes.is_empty() {
        return Err(bad_request(
            "FST_ERR_CTP_EMPTY_JSON_BODY",
            "Body cannot be empty when content-type is set to 'application/json'",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| bad_request("FST_ERR_CTP_INVALID_JSON_BODY", "Body is not valid JSON"))
}

/// Отказ проверки по схеме маршрута (`FST_ERR_VALIDATION`).
#[must_use]
pub fn schema_error(message: &str) -> Response {
    bad_request("FST_ERR_VALIDATION", message)
}

impl FromRequest<AppState> for JsonBody {
    type Rejection = Response;

    async fn from_request(request: Request, _state: &AppState) -> Result<Self, Self::Rejection> {
        read_json(request, BODY_LIMIT).await.map(Self)
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
