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
    clippy::result_large_err,
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
pub mod bots;
pub mod bridges;
pub mod broadcast;
pub mod config;
pub mod db;
pub mod directory;
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
pub mod federation_auth;
pub mod files;
pub mod hub;
pub mod jsjson;
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
pub mod stickers;
pub mod storage;
pub mod twofactor;
pub mod util;
pub mod voice;
pub mod web;
pub mod webhooks;
pub mod ws;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, patch, post, put};
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

async fn content_policy(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    if !response
        .headers()
        .contains_key(header::CONTENT_SECURITY_POLICY)
        && let Ok(policy) = HeaderValue::from_str(&state.csp)
    {
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_SECURITY_POLICY, policy);
        headers.insert(
            HeaderName::from_static("reporting-endpoints"),
            HeaderValue::from_static("csp=\"/api/csp-report\""),
        );
    }
    response
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
        .route("/api/users/@me/reattach", post(routes::federated::reattach))
        .route(
            "/api/users/@me/federation-credential",
            post(routes::federated::federation_credential),
        )
        .route(
            "/api/users/@me/federation-identity/delete",
            post(routes::federated::federation_identity_delete),
        )
        .route("/api/auth/attach-proof", post(routes::federated::attach_proof))
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
        .route("/api/files/", any(routes::files::tus))
        .route("/api/files/{*rest}", any(routes::files::tus))
        .route("/api/uploads/{filename}", get(routes::uploads::get))
        .route("/api/utils/metadata", get(routes::utils::metadata))
        .route("/api/stickers", get(routes::stickers::list).post(routes::stickers::create))
        .route("/api/stickers/assets/{filename}", get(routes::stickers::asset))
        .route("/api/game-icons", post(routes::game_icons::upload))
        .route("/api/game-icons/assets/{filename}", get(routes::game_icons::asset))
        .route("/api/stickers/{id}/collect", post(routes::stickers::collect_existing))
        .route(
            "/api/rustspace/bots",
            get(routes::bots::list).post(routes::bots::create),
        )
        .route(
            "/api/rustspace/bots/{id}",
            patch(routes::bots::update).delete(routes::bots::delete),
        )
        .route(
            "/api/rustspace/bots/{id}/token",
            post(routes::bots::regenerate),
        )
        .route(
            "/api/rustspace/spaces/{space}/bots/{bot}",
            put(routes::bots::add_to_space),
        )
        .route(
            "/api/rustspace/personal-bridges",
            get(routes::personal_bridges::list),
        )
        .route(
            "/api/rustspace/personal-bridges/matrix",
            post(routes::personal_bridges::add_matrix),
        )
        .route(
            "/api/rustspace/personal-bridges/telegram/login",
            post(routes::personal_bridges::telegram_login),
        )
        .route(
            "/api/rustspace/personal-bridges/telegram/code",
            post(routes::personal_bridges::telegram_code),
        )
        .route(
            "/api/rustspace/personal-bridges/telegram/password",
            post(routes::personal_bridges::telegram_password),
        )
        .route(
            "/api/rustspace/personal-bridges/{id}",
            delete(routes::personal_bridges::remove),
        )
        .route(
            "/api/rustspace/personal-bridges/{id}/remote-chats",
            get(routes::personal_bridges::remote_chats),
        )
        .route(
            "/api/rustspace/personal-bridges/{id}/chats",
            put(routes::personal_bridges::link_chat),
        )
        .route(
            "/api/rustspace/personal-bridges/{id}/chats/{chat}",
            delete(routes::personal_bridges::unlink_chat),
        )
        .route(
            "/api/rustspace/sticker-packs",
            get(routes::sticker_packs::list).post(routes::sticker_packs::create),
        )
        .route(
            "/api/rustspace/sticker-packs/with/{sticker}",
            get(routes::sticker_packs::containing),
        )
        .route(
            "/api/rustspace/sticker-packs/{id}",
            get(routes::sticker_packs::get_pack)
                .patch(routes::sticker_packs::update)
                .delete(routes::sticker_packs::remove),
        )
        .route(
            "/api/rustspace/sticker-packs/{id}/stickers/{sticker}",
            put(routes::sticker_packs::put_sticker).delete(routes::sticker_packs::remove_sticker),
        )
        .route("/api/rustspace/sticker-packs/{id}/order", put(routes::sticker_packs::reorder))
        .route(
            "/api/rustspace/sticker-packs/{id}/added",
            put(routes::sticker_packs::add).delete(routes::sticker_packs::unadd),
        )
        .route("/api/stickers/{id}", delete(routes::stickers::remove).patch(routes::stickers::rename))
        .route("/api/gif/enabled", get(routes::gif::enabled))
        .route("/api/gif/trending", get(routes::gif::trending))
        .route("/api/gif/search", get(routes::gif::search))
        .route("/api/spaces/explore", get(routes::explore::explore))
        .route("/api/spaces/{id}/public-join", post(routes::explore::public_join))
        .route("/api/spaces/{id}/request-join", post(routes::explore::request_join))
        .route("/api/spaces/{id}/join-requests", get(routes::explore::list))
        .route("/api/spaces/{id}/join-requests/{requestId}", patch(routes::explore::decide))
        .route("/api/users/@me/join-requests", get(routes::explore::mine))
        .route("/api/directory/spaces", get(routes::directory::document))
        .route("/api/directory", get(routes::directory::feed))
        .route("/api/csp-report", post(routes::directory::csp_report))
        .route("/api/admin/storage/stats", get(routes::admin::storage_stats))
        .route("/api/admin/storage/orphans", get(routes::admin::storage_orphans))
        .route("/api/admin/storage/cleanup", post(routes::admin::storage_cleanup))
        .route("/api/admin/storage/cleanup-media", post(routes::admin::storage_cleanup_media))
        .route("/api/admin/storage/cleanup-tus", post(routes::admin::storage_cleanup_uploads))
        .route("/api/admin/users", get(routes::admin::users))
        .route("/api/admin/users/instances", get(routes::admin::user_instances))
        .route("/api/admin/users/{id}/role", patch(routes::admin::set_role))
        .route("/api/admin/users/{id}/reset-password", post(routes::admin::reset_password))
        .route("/api/admin/users/{id}", delete(routes::admin::delete_user))
        .route("/api/federation/peer/initiate", post(routes::federation::initiate))
        .route("/api/federation/peer/accept", post(routes::federation::accept))
        .route("/api/federation/peer/ensure", post(routes::federation::ensure))
        .route("/api/federation/peer/rotate", post(routes::federation::rotate_inbound))
        .route("/api/federation/peer/denied", post(routes::federation::denied))
        .route("/api/federation/epoch", post(routes::federation::epoch))
        .route("/api/federation/relay", post(routes::federation::relay))
        .route("/api/federation/sync", post(routes::federation::sync))
        .route("/api/admin/test/seed-peer", post(routes::federation::test_seed_peer))
        .route(
            "/api/admin/test/federation/resync",
            post(routes::federation::test_resync),
        )
        .route("/api/federation/users/lookup", post(routes::federation::users_lookup))
        .route("/api/federation/users/by-home-id", post(routes::federation::users_by_home_id))
        .route("/api/federation/verify-attach-proof", post(routes::federation::verify_attach_proof))
        .route("/api/federation/identity", delete(routes::federation::delete_identity))
        .route("/api/federation/peers", get(routes::federation::list_peers))
        .route(
            "/api/federation/peers/{id}",
            delete(routes::federation::revoke_peer).patch(routes::federation::update_peer),
        )
        .route("/api/federation/peers/{id}/reset", post(routes::federation::reset_peer))
        .route("/api/federation/peers/{id}/recheck", post(routes::federation::recheck_peer))
        .route("/api/federation/peers/{id}/rotate", post(routes::federation::rotate_peer))
        .route("/api/federation/peers/{id}/permanent", delete(routes::federation::delete_peer))
        .route("/api/federation/reset-events", get(routes::federation::reset_events))
        .route(
            "/api/federation/reset-events/acknowledge",
            post(routes::federation::acknowledge_reset),
        )
        .route("/api/federation/approval-requests", get(routes::federation::approval_requests))
        .route(
            "/api/federation/approval-requests/{id}/approve",
            post(routes::federation::approve_request),
        )
        .route(
            "/api/federation/approval-requests/{id}/deny",
            post(routes::federation::deny_request),
        )
        .route("/api/federation/peering-subscriptions", get(routes::federation::subscriptions))
        .route(
            "/api/federation/peering-subscriptions/{id}",
            delete(routes::federation::cancel_subscription),
        )
        .route("/api/federation/peering-notifications", get(routes::federation::notifications))
        .route(
            "/api/federation/peering-notifications/read-all",
            post(routes::federation::read_all_notifications),
        )
        .route(
            "/api/federation/peering-notifications/{id}/read",
            post(routes::federation::read_notification),
        )
        .route(
            "/api/admin/telemetry",
            get(routes::admin_status::telemetry).put(routes::admin_status::set_telemetry),
        )
        .route("/api/admin/telemetry/preview", get(routes::admin_status::telemetry_preview))
        .route("/api/admin/instance/update-status", get(routes::admin_status::update_status))
        .route("/api/rustspace/admin/update", post(routes::admin_status::request_update))
        .route("/api/auth/2fa", get(routes::twofactor::status))
        .route("/api/auth/2fa/setup", post(routes::twofactor::setup))
        .route("/api/auth/2fa/enable", post(routes::twofactor::enable))
        .route("/api/auth/2fa/disable", post(routes::twofactor::disable))
        .route("/api/auth/2fa/recovery-codes", post(routes::twofactor::recovery_codes))
        .route(
            "/api/e2ee/key",
            get(routes::e2ee::mine).put(routes::e2ee::publish).delete(routes::e2ee::withdraw),
        )
        .route("/api/e2ee/keys/{userId}", get(routes::e2ee::of_user))
        .route(
            "/api/rustspace/channels/{id}/bridges",
            get(routes::bridges::list).post(routes::bridges::create),
        )
        .route("/api/rustspace/bridges/{linkId}", delete(routes::bridges::remove))
        .route(
            "/api/rustspace/channels/{id}/webhooks",
            get(routes::webhooks::list).post(routes::webhooks::create),
        )
        .route("/api/rustspace/webhooks/{hookId}", delete(routes::webhooks::remove))
        .route("/api/rustspace/channels/{id}/cards", post(routes::webhooks::send_cards))
        .route("/api/webhooks/{hookId}/{token}", post(routes::webhooks::execute))
        .route("/api/rustspace/channels/{id}/nsfw", put(routes::channels::set_nsfw))
        .route("/api/rustspace/saved", get(routes::saved::saved))
        .route("/api/rustspace/pins/{kind}/{channelId}", get(routes::saved::pins))
        .route(
            "/api/rustspace/pins/{kind}/{channelId}/{messageId}",
            put(routes::saved::pin).delete(routes::saved::unpin),
        )
        .route("/api/bookmarks", get(routes::saved::bookmarks))
        .route("/api/bookmarks/keys", get(routes::saved::bookmark_keys))
        .route(
            "/api/bookmarks/{kind}/{messageId}",
            put(routes::saved::bookmark_add).delete(routes::saved::bookmark_remove),
        )
        .route(
            "/api/rustspace/admin/voice-bitrate",
            get(routes::voice_bitrate::instance_get).put(routes::voice_bitrate::instance_put),
        )
        .route(
            "/api/rustspace/voice-bitrate/{target}",
            get(routes::voice_bitrate::get).put(routes::voice_bitrate::put),
        )
        .route("/api/livekit/token", post(routes::livekit::token))
        .route("/ws", get(ws::upgrade))
        // Тело разбирается до обработчика (и до проверки входа), как у Fastify.
        .route_layer(axum::middleware::from_fn(extract::body_stage))
        .fallback(web::fallback)
        .method_not_allowed_fallback(web::fallback)
        .layer(axum::middleware::from_fn_with_state(state.clone(), global_limit))
        // Политика содержимого: маршрут, задавший свою (раздача загрузок), сохраняет её.
        .layer(axum::middleware::from_fn_with_state(state.clone(), content_policy))
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
        .layer(fixed(HeaderName::from_static("origin-agent-cluster"), "?1"))
        .layer(fixed(HeaderName::from_static("x-dns-prefetch-control"), "off"))
        .layer(fixed(HeaderName::from_static("x-download-options"), "noopen"))
        .layer(fixed(HeaderName::from_static("x-permitted-cross-domain-policies"), "none"))
        .layer(fixed(HeaderName::from_static("x-xss-protection"), "0"))
        .layer(cors())
        .with_state(state)
}

/// Запускает фоновые работы сервера: уборку незавершённых загрузок и хранилища, оповещение каталога.
pub fn spawn_background(state: &AppState) {
    tokio::spawn(directory::run_pinger(state.clone()));
    federation::worker::spawn(state);
    federation::calls::spawn(state);
    bridges::spawn(state);
    bridges::personal::spawn(state);
    bots::load(state);
    tokio::spawn(routes::livekit::run_evictions(state.clone()));
    let uploads = state.clone();
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_hours(1));
        loop {
            timer.tick().await;
            routes::files::sweep_expired(&uploads).await;
        }
    });
    let janitor = state.clone();
    tokio::spawn(async move {
        // Первая уборка — через час после запуска, дальше раз в сутки.
        let start = tokio::time::Instant::now() + std::time::Duration::from_hours(1);
        let mut timer = tokio::time::interval_at(start, std::time::Duration::from_hours(24));
        loop {
            timer.tick().await;
            let config = janitor.config.clone();
            match janitor
                .run(move |tx, _| Ok(storage::cleanup(tx, &config, false)?))
                .await
            {
                Ok(done)
                    if done.deleted_files + done.deleted_records > 0 || !done.errors.is_empty() =>
                {
                    tracing::info!(
                        files = done.deleted_files,
                        records = done.deleted_records,
                        bytes = done.freed_bytes,
                        errors = done.errors.len(),
                        "уборка хранилища"
                    )
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(?error, "уборка хранилища не удалась"),
            }
        }
    });
}
