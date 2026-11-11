//! Сервер Backspace на Rust.
//!
//! Совместим с оригинальным сервером по трём границам: HTTP и WebSocket для
//! клиентов, схема базы (тот же файл SQLite открывается обоими) и федерация
//! между инстансами. Сейчас готово ядро входа: конфигурация, база с
//! миграциями оригинала, регистрация, вход и профиль; остальные маршруты
//! добавляются по тому же образцу и проверяются контрактным стендом.

// Обработчики повторяют порядок проверок оригинала один к одному: от него
// зависит, какой код ошибки получит клиент. Поэтому стилистические замечания,
// которые требуют этот порядок перекроить, для сервера отключены.
#![allow(
    clippy::format_push_string,
    clippy::too_many_lines,
    clippy::useless_let_if_seq,
    clippy::option_if_let_else,
    clippy::single_match_else,
    clippy::similar_names,
    clippy::needless_pass_by_value,
    clippy::implicit_hasher,
    clippy::type_complexity,
    clippy::assigning_clones,
    clippy::significant_drop_tightening,
    clippy::too_long_first_doc_paragraph,
    clippy::cognitive_complexity,
    clippy::result_unit_err,
    clippy::unused_async,
    clippy::too_many_arguments,
    clippy::struct_excessive_bools,
    clippy::option_option,
    clippy::semicolon_if_nothing_returned,
    reason = "порядок проверок и форма ответов повторяют оригинальный сервер"
)]

pub mod auth;
pub mod broadcast;
pub mod config;
pub mod db;
pub mod dm;
pub mod dm_events;
pub mod embeds;
pub mod error;
#[allow(
    clippy::redundant_pub_crate,
    reason = "таблица только для этого крейта"
)]
mod error_messages;
pub mod events;
pub mod extract;
pub mod federation;
pub mod files;
pub mod hub;
pub mod media;
pub mod mentions;
pub mod messages;
pub mod net;
pub mod perms;
pub mod ratelimit;
pub mod ready;
pub mod routes;
pub mod snowflake;
pub mod state;
pub mod util;
pub mod voice;
pub mod ws;

use axum::Json;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, patch, post};
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
        .route(
            "/api/users/@me",
            get(routes::users::me)
                .patch(routes::account::update_me)
                .delete(routes::account::delete_me),
        )
        .route("/api/users/@me/verify-password", post(routes::account::verify_password))
        .route("/api/users/@me/change-password", post(routes::account::change_password))
        .route(
            "/api/users/@me/federation-registry",
            get(routes::account::registry).put(routes::account::put_registry),
        )
        .route("/api/users/@me/space-layout", axum::routing::put(routes::account::put_space_layout))
        .route("/api/users/{id}", get(routes::account::get_user))
        .route("/api/users/{id}/mutuals", get(routes::account::mutuals))
        .route("/api/spaces", get(routes::spaces::list).post(routes::spaces::create))
        .route("/api/spaces/join", post(routes::spaces::join_by_code))
        .route("/api/spaces/invite/{code}/preview", get(routes::spaces::invite_preview))
        .route(
            "/api/spaces/{id}",
            get(routes::spaces::detail)
                .patch(routes::spaces::update)
                .delete(routes::spaces::delete),
        )
        .route("/api/spaces/{id}/invite", post(routes::spaces::invite))
        .route("/api/spaces/{id}/join", post(routes::spaces::join))
        .route("/api/spaces/{id}/members", get(routes::spaces::members))
        .route(
            "/api/spaces/{id}/members/{uid}",
            patch(routes::spaces::update_member).delete(routes::spaces::kick),
        )
        .route("/api/spaces/{id}/members/{uid}/roles", post(routes::spaces::add_member_role))
        .route(
            "/api/spaces/{id}/members/{uid}/roles/{role_id}",
            delete(routes::spaces::remove_member_role),
        )
        .route("/api/spaces/{id}/roles", post(routes::spaces::create_role))
        .route(
            "/api/spaces/{id}/roles/{role_id}",
            patch(routes::spaces::update_role).delete(routes::spaces::delete_role),
        )
        .route("/api/spaces/{id}/transfer-ownership", patch(routes::spaces::transfer_ownership))
        .route("/api/spaces/{id}/bans", get(routes::spaces::bans).post(routes::spaces::ban))
        .route("/api/spaces/{id}/bans/{uid}", delete(routes::spaces::unban))
        .route(
            "/api/spaces/{id}/channels",
            get(routes::channels::list).post(routes::channels::create),
        )
        .route("/api/spaces/{id}/categories", post(routes::channels::create_category))
        .route("/api/spaces/{id}/channel-layout", patch(routes::channels::update_layout))
        .route(
            "/api/channels/{id}",
            patch(routes::channels::update).delete(routes::channels::delete),
        )
        .route(
            "/api/channels/{id}/overrides",
            get(routes::channels::channel_overrides).put(routes::channels::put_channel_override),
        )
        .route(
            "/api/channels/{id}/overrides/{target_type}/{target_id}",
            delete(routes::channels::delete_channel_override),
        )
        .route(
            "/api/categories/{id}",
            patch(routes::channels::update_category).delete(routes::channels::delete_category),
        )
        .route(
            "/api/categories/{id}/overrides",
            get(routes::channels::category_overrides).put(routes::channels::put_category_override),
        )
        .route(
            "/api/categories/{id}/overrides/{target_type}/{target_id}",
            delete(routes::channels::delete_category_override),
        )
        .route(
            "/api/channels/{id}/messages",
            get(routes::messages::history).post(routes::messages::create),
        )
        .route(
            "/api/messages/{id}",
            patch(routes::messages::update).delete(routes::messages::delete),
        )
        .route("/api/dm", get(routes::dm::list).post(routes::dm::open))
        .route("/api/dm/group", post(routes::dm::create_group))
        .route("/api/dm/space-invite", post(routes::dm::space_invite))
        .route(
            "/api/dm/messages/{id}",
            patch(routes::dm::update_message).delete(routes::dm::delete_message),
        )
        .route("/api/dm/{id}", patch(routes::dm::update).delete(routes::dm::close))
        .route(
            "/api/dm/{id}/members",
            post(routes::dm::add_member).delete(routes::dm::leave),
        )
        .route("/api/dm/{id}/members/{target}", delete(routes::dm::kick))
        .route("/api/dm/{id}/transfer", post(routes::dm::transfer))
        .route(
            "/api/dm/{id}/messages",
            get(routes::dm::history).post(routes::dm::create_message),
        )
        .route("/api/social/friends", get(routes::social::friends))
        .route("/api/social/friends/{id}", delete(routes::social::remove_friend))
        .route(
            "/api/social/requests",
            get(routes::social::requests).post(routes::social::send_request),
        )
        .route(
            "/api/social/requests/{id}",
            patch(routes::social::answer_request).delete(routes::social::cancel_request),
        )
        .route("/api/social/discover", get(routes::social::discover))
        .route("/api/social/search", get(routes::social::search))
        .route("/api/users/@me/notification-settings", get(routes::notifications::list))
        .route(
            "/api/spaces/{id}/notification-settings",
            patch(routes::notifications::update_space),
        )
        .route(
            "/api/channels/{id}/notification-settings",
            patch(routes::notifications::update_channel),
        )
        .route(
            "/api/admin/invites",
            get(routes::invites::list).post(routes::invites::create),
        )
        .route(
            "/api/admin/invites/{id}",
            patch(routes::invites::update).delete(routes::invites::delete),
        )
        .route("/api/admin/invites/{id}/revoke", post(routes::invites::revoke))
        .route("/api/admin/invites/{id}/reinstate", post(routes::invites::reinstate))
        .route("/api/admin/invites/{id}/redemptions", get(routes::invites::redemptions))
        .route(
            "/api/settings/streaming",
            get(routes::settings::streaming).patch(routes::settings::update_streaming),
        )
        .route(
            "/api/settings/instance",
            get(routes::settings::instance).patch(routes::settings::update_instance),
        )
        .route("/api/channels/{id}/search", get(routes::search::channel))
        .route("/api/channels/{id}/messages/around", get(routes::search::channel_around))
        .route("/api/dm/{id}/search", get(routes::search::dm_channel))
        .route("/api/dm/{id}/messages/around", get(routes::search::dm_around))
        .route("/api/files", any(routes::files::tus))
        .route("/api/files/{*rest}", any(routes::files::tus))
        .route("/api/uploads/{filename}", get(routes::uploads::get))
        .route("/api/utils/metadata", get(routes::utils::metadata))
        .route("/api/livekit/token", post(routes::livekit::token))
        .route("/ws", get(ws::upgrade))
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

/// Запускает фоновые работы сервера: уборку незавершённых загрузок.
pub fn spawn_background(state: &AppState) {
    let uploads = state.clone();
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_hours(1));
        loop {
            timer.tick().await;
            routes::files::sweep_expired(&uploads).await;
        }
    });
}
