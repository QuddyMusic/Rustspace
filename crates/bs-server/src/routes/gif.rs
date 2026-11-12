//! Поиск GIF через Klipy (`routes/gif.ts`). Ключ сервиса задаёт администратор
//! в настройках инстанса; без ключа поиск выключен.

use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::extract::ClientIp;
use crate::params;
use crate::ratelimit::Limit;
use crate::routes::uploads::encode_uri_component;
use crate::state::AppState;
use crate::util::js_trim;

const GIF_LIMIT: Limit = Limit {
    name: "gif",
    max: 30,
    window: Duration::from_mins(1),
};
const KLIPY: &str = "https://api.klipy.com/api/v1";
const TIMEOUT: Duration = Duration::from_secs(10);

/// Параметры запросов GIF.
#[derive(Debug, Default, serde::Deserialize)]
pub struct GifQuery {
    q: Option<String>,
    limit: Option<String>,
    pos: Option<String>,
}

async fn api_key(state: &AppState) -> ApiResult<Option<String>> {
    state
        .run(|tx, _| {
            let row = tx.query_one(
                "SELECT gif_api_key FROM instance_settings WHERE id = 1",
                params![],
            )?;
            Ok(row
                .map(|row| row.opt_text("gif_api_key"))
                .transpose()?
                .flatten()
                .filter(|key| !key.is_empty()))
        })
        .await
}

fn empty() -> Json<Value> {
    Json(json!({"results": [], "next": ""}))
}

/// `Number(text) || fallback`.
fn number_or(text: Option<&str>, fallback: f64) -> f64 {
    let parsed = text
        .map(js_trim)
        .and_then(|text| text.parse::<f64>().ok())
        .unwrap_or(f64::NAN);
    if parsed.is_nan() || parsed == 0.0 {
        fallback
    } else {
        parsed
    }
}

/// Приводит ответ Klipy к виду клиента (`mapKlipyResults`).
fn map_results(gifs: &Value) -> Vec<Value> {
    let file = |tier: &Value, kinds: [&str; 2]| {
        kinds
            .iter()
            .map(|kind| &tier[*kind])
            .find(|file| file.is_object())
            .cloned()
    };
    let tier = |files: &Value, names: [&str; 2]| {
        names
            .iter()
            .map(|name| &files[*name])
            .find(|tier| !tier.is_null())
            .cloned()
    };
    gifs.as_array()
        .into_iter()
        .flatten()
        .filter_map(|gif| {
            let files = &gif["file"];
            let preview = file(&tier(files, ["sm", "xs"])?, ["webp", "gif"])?;
            let full = file(&tier(files, ["hd", "md"])?, ["gif", "webp"])?;
            let id = match gif["slug"].as_str().filter(|slug| !slug.is_empty()) {
                Some(slug) => slug.to_owned(),
                None => gif["id"]
                    .as_i64()
                    .map_or_else(|| gif["id"].to_string(), |id| id.to_string()),
            };
            Some(json!({
                "id": id,
                "title": gif["title"].as_str().unwrap_or_default(),
                "previewUrl": preview["url"],
                "url": full["url"],
                "width": preview["width"],
                "height": preview["height"],
            }))
        })
        .collect()
}

async fn ask(key: &str, kind: &str, query: &GifQuery, q: Option<&str>) -> Option<Value> {
    #[allow(clippy::cast_possible_truncation, reason = "значение зажато в пределы")]
    let per_page = number_or(query.limit.as_deref(), 24.0).clamp(1.0, 50.0) as i64;
    let page = number_or(query.pos.as_deref(), 1.0);
    let page = if page.fract() == 0.0 {
        format!("{page:.0}")
    } else {
        page.to_string()
    };
    let mut url = url::Url::parse(&format!(
        "{KLIPY}/{}/gifs/{kind}",
        encode_uri_component(key)
    ))
    .ok()?;
    {
        let mut pairs = url.query_pairs_mut();
        if let Some(q) = q {
            pairs.append_pair("q", q);
        }
        pairs
            .append_pair("per_page", &per_page.to_string())
            .append_pair("page", &page);
    }
    let client = reqwest::Client::builder().timeout(TIMEOUT).build().ok()?;
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        tracing::warn!(status = response.status().as_u16(), "Klipy ответил ошибкой");
        return None;
    }
    let data: Value = response.json().await.ok()?;
    if data["result"].as_bool() != Some(true) || !data["data"].is_object() {
        return None;
    }
    let next = if data["data"]["has_next"].as_bool() == Some(true) {
        (data["data"]["current_page"].as_i64().unwrap_or(0) + 1).to_string()
    } else {
        String::new()
    };
    Some(json!({"results": map_results(&data["data"]["data"]), "next": next}))
}

/// `GET /api/gif/enabled`.
pub async fn enabled(State(state): State<AppState>, _user: AuthUser) -> ApiResult<Json<Value>> {
    Ok(Json(json!({"enabled": api_key(&state).await?.is_some()})))
}

/// `GET /api/gif/trending`.
pub async fn trending(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    _user: AuthUser,
    Query(query): Query<GifQuery>,
) -> ApiResult<Json<Value>> {
    state.limiter.check(GIF_LIMIT, ip)?;
    let Some(key) = api_key(&state).await? else {
        return Ok(empty());
    };
    Ok(ask(&key, "trending", &query, None)
        .await
        .map_or_else(empty, Json))
}

/// `GET /api/gif/search`.
pub async fn search(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    _user: AuthUser,
    Query(query): Query<GifQuery>,
) -> ApiResult<Json<Value>> {
    state.limiter.check(GIF_LIMIT, ip)?;
    let Some(key) = api_key(&state).await? else {
        return Ok(empty());
    };
    let q = query
        .q
        .as_deref()
        .map(js_trim)
        .filter(|q| !q.is_empty())
        .map(str::to_owned);
    let Some(q) = q else {
        return Err(ApiError::bad_request("validation_failed"));
    };
    Ok(ask(&key, "search", &query, Some(&q))
        .await
        .map_or_else(empty, Json))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn klipy_results_are_mapped() {
        let gifs = json!([
            {"id": 7, "slug": "cat", "title": "Кот", "file": {
                "sm": {"webp": {"url": "p.webp", "width": 100, "height": 50}},
                "hd": {"gif": {"url": "f.gif", "width": 400, "height": 200}}}},
            {"id": 8, "slug": "", "file": {
                "xs": {"gif": {"url": "p.gif", "width": 10, "height": 5}},
                "md": {"webp": {"url": "f.webp", "width": 40, "height": 20}}}},
            {"id": 9, "slug": "no-preview", "file": {"hd": {"gif": {"url": "x"}}}},
            {"id": 10, "slug": "no-file"},
        ]);
        assert_eq!(
            map_results(&gifs),
            vec![
                json!({"id": "cat", "title": "Кот", "previewUrl": "p.webp", "url": "f.gif", "width": 100, "height": 50}),
                json!({"id": "8", "title": "", "previewUrl": "p.gif", "url": "f.webp", "width": 10, "height": 5}),
            ]
        );
    }
}
