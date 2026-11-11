//! Сервер Backspace на Rust.
//!
//! Совместим с оригинальным сервером по трём границам: HTTP и WebSocket для
//! клиентов, схема базы (тот же файл SQLite открывается обоими) и федерация
//! между инстансами. Сейчас готово ядро входа: конфигурация, база с
//! миграциями оригинала, регистрация, вход и профиль; остальные маршруты
//! добавляются по тому же образцу и проверяются контрактным стендом.

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
#[allow(
    clippy::redundant_pub_crate,
    reason = "таблица только для этого крейта"
)]
mod error_messages;
pub mod extract;
pub mod ratelimit;
pub mod routes;
pub mod snowflake;
pub mod state;

use axum::Json;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::json;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::extract::ClientIp;
use crate::ratelimit::Limit;
use crate::state::AppState;

/// Общий предел частоты на адрес — на все маршруты.
async fn global_limit(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    request: Request,
    next: Next,
) -> Response {
    match state.limiter.check(Limit::GLOBAL, ip) {
        Ok(()) => next.run(request).await,
        Err(err) => err.into_response(),
    }
}

/// Ответ на неизвестный маршрут — как у Fastify по умолчанию.
async fn not_found(method: Method, uri: axum::http::Uri) -> Response {
    let path = uri
        .path_and_query()
        .map_or_else(|| uri.path(), axum::http::uri::PathAndQuery::as_str);
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "message": format!("Route {method}:{path} not found"),
            "error": "Not Found",
            "statusCode": 404,
        })),
    )
        .into_response()
}

fn cors() -> CorsLayer {
    let name = |text: &'static str| HeaderName::from_static(text);
    CorsLayer::new()
        // Как `origin: true` у оригинала: отражаем источник запроса. Сервер
        // авторизует по заголовку Authorization, а не по cookie.
        .allow_origin(AllowOrigin::mirror_request())
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
            Method::HEAD,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            name("tus-resumable"),
            name("upload-length"),
            name("upload-offset"),
            name("upload-metadata"),
            name("upload-defer-length"),
            name("upload-concat"),
            name("upload-checksum"),
            name("x-http-method-override"),
        ])
        .expose_headers([
            name("x-backspace-paging"),
            header::LOCATION,
            name("tus-resumable"),
            name("tus-version"),
            name("tus-extension"),
            name("tus-max-size"),
            name("tus-checksum-algorithm"),
            name("upload-offset"),
            name("upload-length"),
            name("upload-metadata"),
            name("upload-expires"),
        ])
}

/// Собирает приложение.
pub fn app(state: AppState) -> Router {
    let fixed = |name: HeaderName, value: &'static str| {
        SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
    };
    Router::new()
        .route("/api/health", get(routes::instance::health))
        .route("/api/instance/info", get(routes::instance::info))
        .route("/api/auth/register", post(routes::auth::register))
        .route("/api/auth/login", post(routes::auth::login))
        .route("/api/auth/check-username", get(routes::auth::check_username_route))
        .route("/api/auth/check-invite", get(routes::auth::check_invite))
        .route("/api/users/@me", get(routes::users::me))
        .fallback(not_found)
        .layer(axum::middleware::from_fn_with_state(state.clone(), global_limit))
        // Заголовки безопасности, как helmet у оригинала.
        .layer(fixed(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(fixed(header::X_FRAME_OPTIONS, "DENY"))
        .layer(fixed(header::REFERRER_POLICY, "strict-origin-when-cross-origin"))
        .layer(fixed(
            HeaderName::from_static("cross-origin-opener-policy"),
            "same-origin",
        ))
        .layer(fixed(
            HeaderName::from_static("cross-origin-resource-policy"),
            "cross-origin",
        ))
        .layer(cors())
        .with_state(state)
}
