//! Каталог пространств и приём отчётов CSP (`routes/directory.ts`,
//! `routes/cspReport.ts`).

use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::http::header::{CACHE_CONTROL, USER_AGENT};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use crate::auth::AuthUser;
use crate::directory::{self, FeedQuery};
use crate::error::{ApiError, ApiResult};
use crate::extract::ClientIp;
use crate::federation::our_origin;
use crate::params;
use crate::ratelimit::Limit;
use crate::state::AppState;
use crate::util::utf16_prefix;

const FEED_LIMIT: Limit = Limit {
    name: "directory",
    max: 30,
    window: Duration::from_mins(1),
};
/// Сколько байт отчёта CSP читается и сколько символов попадает в журнал.
const MAX_REPORT_BYTES: usize = 16_384;
const MAX_LOGGED_CHARS: usize = 4_096;

/// `GET /api/directory/spaces`: документ этого инстанса для каталога.
pub async fn document(State(state): State<AppState>) -> ApiResult<Response> {
    let origin = our_origin(&state.config);
    let version = state.config.version.clone();
    let document = state
        .run(move |tx, _| Ok(directory::document(tx, &origin, &version)?))
        .await?;
    let mut response = Json(document).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok(response)
}

/// Параметры ленты.
#[derive(Debug, Default, serde::Deserialize)]
pub struct FeedParams {
    q: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/directory`: лента общего каталога.
pub async fn feed(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    _user: AuthUser,
    Query(query): Query<FeedParams>,
) -> ApiResult<Json<Value>> {
    state.limiter.check(FEED_LIMIT, ip)?;
    let endpoint = state.config.directory_endpoint.clone();
    let browse = state
        .run(|tx, _| {
            let row = tx.query_one(
                "SELECT directory_browse_enabled FROM instance_settings WHERE id = 1",
                params![],
            )?;
            Ok(row.is_some_and(|row| {
                row.opt_flag("directory_browse_enabled").ok().flatten() == Some(true)
            }))
        })
        .await?;
    if endpoint.is_empty() || !browse {
        return Err(ApiError::not_found("directory_disabled"));
    }
    let query = FeedQuery::parse(
        query.q.as_deref(),
        query.limit.as_deref(),
        query.offset.as_deref(),
    );
    directory::feed(&endpoint, &state.config.version, &query)
        .await
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_GATEWAY, "directory_unreachable"))
}

/// `POST /api/csp-report`: браузер сообщает о нарушении политики содержимого.
pub async fn csp_report(headers: HeaderMap, body: axum::body::Body) -> StatusCode {
    let bytes = axum::body::to_bytes(body, MAX_REPORT_BYTES)
        .await
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let summary = match serde_json::from_str::<Value>(&text) {
        Ok(parsed) => parsed.to_string(),
        Err(_) => format!("unparseable: {text}"),
    };
    // В журнал идёт строка JSON: переводы строк и управляющие символы в ней
    // экранированы, подделать соседние записи отчётом нельзя.
    let agent = headers
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let csp = Value::from(utf16_prefix(&summary, MAX_LOGGED_CHARS)).to_string();
    let user_agent = Value::from(utf16_prefix(agent, 300)).to_string();
    tracing::warn!(%csp, %user_agent, "браузер сообщил о нарушении CSP");
    StatusCode::NO_CONTENT
}
