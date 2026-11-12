//! Каталог пространств: документ инстанса, лента общего каталога и
//! оповещение каталога об изменениях (`directory/*.ts`, `routes/directory.ts`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use crate::db::{DbResult, Tx};
use crate::error::ApiError;
use crate::federation::our_origin;
use crate::params;
use crate::state::{AppState, now_ms};
use crate::util::utf16_prefix;

const MAX_SPACES: i64 = 200;
const MAX_NAME: usize = 100;
const MAX_DESCRIPTION: usize = 200;
const TIMEOUT: Duration = Duration::from_secs(10);
const FEED_CACHE: Duration = Duration::from_mins(1);
const FEED_CACHE_ENTRIES: usize = 64;
/// Паузы между повторами оповещения.
const BACKOFF_MS: [i64; 4] = [60_000, 300_000, 900_000, 3_600_000];

/// Номер правки документа: растёт при каждом изменении каталога.
static DOCUMENT_VERSION: AtomicU64 = AtomicU64::new(0);

/// Отмечает, что документ каталога изменился (`markDirectoryDirty`).
///
/// # Errors
/// Ошибка базы.
pub fn mark_dirty(tx: &mut dyn Tx) -> DbResult<()> {
    tx.execute(
        "UPDATE instance_settings SET directory_dirty = 1 WHERE id = 1",
        params![],
    )?;
    DOCUMENT_VERSION.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// Ссылка на картинку в абсолютном виде; чужие адреса не публикуются
/// (`absoluteAssetUrl`).
fn asset_url(value: Option<String>, origin: &str) -> Option<String> {
    let value = value?;
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let same = match (Url::parse(&value), Url::parse(origin)) {
            (Ok(a), Ok(b)) => a.origin() == b.origin(),
            _ => false,
        };
        return same.then_some(value);
    }
    if value.starts_with('/') {
        Some(format!("{origin}{value}"))
    } else {
        Some(format!("{origin}/api/uploads/{value}"))
    }
}

/// Документ инстанса для каталога (`buildDirectoryDocument`).
///
/// # Errors
/// Ошибка базы.
pub fn document(tx: &mut dyn Tx, origin: &str, version: &str) -> DbResult<Value> {
    let settings = tx.query_one(
        "SELECT instance_name, federated_registration_open, discovery_enabled, directory_enabled \
         FROM instance_settings WHERE id = 1",
        params![],
    )?;
    let flag = |name: &str| {
        settings
            .as_ref()
            .is_some_and(|row| row.opt_flag(name).ok().flatten() == Some(true))
    };
    let name = settings
        .as_ref()
        .and_then(|row| row.opt_text("instance_name").ok().flatten());
    let mut spaces = Vec::new();
    if flag("directory_enabled") && flag("discovery_enabled") {
        let rows = tx.query(
            "SELECT s.id, s.name, s.icon, s.banner, s.avatar_color, s.description, s.visibility, s.created_at, \
             (SELECT count(*) FROM space_members sm WHERE sm.space_id = s.id) AS member_count \
             FROM spaces s WHERE s.visibility IN ('public', 'request') AND s.directory_listed = 1 \
             ORDER BY member_count DESC, s.created_at DESC LIMIT ?",
            params![MAX_SPACES],
        )?;
        for row in rows {
            spaces.push(json!({
                "id": row.text("id")?,
                "name": utf16_prefix(&row.text("name")?, MAX_NAME),
                "description": row.opt_text("description")?.map(|text| utf16_prefix(&text, MAX_DESCRIPTION).to_owned()),
                "icon": asset_url(row.opt_text("icon")?, origin),
                "banner": asset_url(row.opt_text("banner")?, origin),
                "avatarColor": row.opt_text("avatar_color")?,
                "visibility": row.text("visibility")?,
                "memberCount": row.int("member_count")?,
                "createdAt": row.int("created_at")?,
            }));
        }
    }
    Ok(json!({
        "schema": 1,
        "origin": origin,
        "instance": {
            "name": name.unwrap_or_else(|| "Backspace".to_owned()),
            "federatedRegistrationOpen": flag("federated_registration_open"),
            "version": version,
        },
        "spaces": spaces,
    }))
}

/// Запрос к ленте каталога.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedQuery {
    q: String,
    limit: i64,
    offset: i64,
}

fn clamp_int(raw: Option<&str>, fallback: i64, min: i64, max: i64) -> i64 {
    let Some(raw) = raw else { return fallback };
    let text = raw.trim_start();
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse::<i64>()
        .map_or(fallback, |value| (sign * value).clamp(min, max))
}

impl FeedQuery {
    /// Разбор параметров (`parseFeedQuery`).
    #[must_use]
    pub fn parse(q: Option<&str>, limit: Option<&str>, offset: Option<&str>) -> Self {
        Self {
            q: utf16_prefix(crate::util::js_trim(q.unwrap_or_default()), 100).to_owned(),
            limit: clamp_int(limit, 50, 1, 100),
            offset: clamp_int(offset, 0, 0, 1000),
        }
    }
}

static FEEDS: LazyLock<Mutex<HashMap<String, (Instant, Value)>>> = LazyLock::new(Mutex::default);

fn client(version: &str) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(format!("backspace-server/{version}"))
        .build()
        .ok()
}

/// Лента общего каталога с кратким запоминанием ответов (`getFeed`).
pub async fn feed(endpoint: &str, version: &str, query: &FeedQuery) -> Option<Value> {
    let key = format!("{}:{}:{}", query.limit, query.offset, query.q);
    {
        let mut feeds = FEEDS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match feeds.get(&key) {
            Some((at, feed)) if at.elapsed() < FEED_CACHE => return Some(feed.clone()),
            Some(_) => {
                feeds.remove(&key);
            }
            None => {}
        }
    }
    let mut url = Url::parse(&format!("{endpoint}/v1/spaces")).ok()?;
    {
        let mut pairs = url.query_pairs_mut();
        if !query.q.is_empty() {
            pairs.append_pair("q", &query.q);
        }
        pairs
            .append_pair("limit", &query.limit.to_string())
            .append_pair("offset", &query.offset.to_string());
    }
    let response = client(version)?
        .get(url)
        .header("accept", "application/json")
        .send()
        .await
        .ok()?;
    if response.status().as_u16() != 200 {
        return None;
    }
    let age = response
        .headers()
        .get("age")
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.trim().parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map_or(Duration::ZERO, |seconds| {
            Duration::from_secs_f64(seconds.min(FEED_CACHE.as_secs_f64()))
        });
    let body: Value = response.json().await.ok()?;
    let valid = body["schema"].as_i64() == Some(1)
        && body["spaces"]
            .as_array()
            .is_some_and(|spaces| spaces.iter().all(Value::is_object));
    if !valid {
        return None;
    }
    let mut feeds = FEEDS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if feeds.len() >= FEED_CACHE_ENTRIES
        && let Some(oldest) = feeds
            .iter()
            .min_by_key(|(_, (at, _))| *at)
            .map(|(key, _)| key.clone())
    {
        feeds.remove(&oldest);
    }
    let stored_at = Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
    feeds.insert(key, (stored_at, body.clone()));
    Some(body)
}

/// Память оповещателя между проходами.
#[derive(Debug, Default)]
struct Pinger {
    failures: usize,
    next_retry_at: Option<i64>,
    cooldown_until: Option<i64>,
    halted_version: Option<u64>,
    retired: bool,
}

#[derive(Debug)]
struct DirectoryState {
    enabled: bool,
    dirty: bool,
    last_ping_at: Option<i64>,
    last_error_at: Option<i64>,
}

async fn read_state(state: &AppState) -> Option<DirectoryState> {
    state
        .run(|tx, _| {
            let row = tx.query_one(
                "SELECT directory_enabled, directory_dirty, directory_last_ping_at, directory_last_error \
                 FROM instance_settings WHERE id = 1",
                params![],
            )?;
            let Some(row) = row else {
                return Ok(None);
            };
            let last_error_at = row
                .opt_text("directory_last_error")?
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|error| error["at"].as_i64());
            Ok(Some(DirectoryState {
                enabled: row.opt_flag("directory_enabled")? == Some(true),
                dirty: row.opt_flag("directory_dirty")? == Some(true),
                last_ping_at: row.opt_int("directory_last_ping_at")?,
                last_error_at,
            }))
        })
        .await
        .ok()
        .flatten()
}

async fn record(state: &AppState, sql: &'static str, values: Vec<crate::db::Value>) {
    let done = state
        .run(move |tx, _| Ok::<_, ApiError>(tx.execute(sql, &values)?))
        .await;
    if done.is_err() {
        tracing::warn!("состояние каталога не записано");
    }
}

async fn record_failure(state: &AppState, pinger: &mut Pinger, at: i64, error: Value, retry: bool) {
    record(
        state,
        "UPDATE instance_settings SET directory_last_error = ? WHERE id = 1",
        vec![error.to_string().into()],
    )
    .await;
    if retry {
        let step = BACKOFF_MS[pinger.failures.min(BACKOFF_MS.len() - 1)];
        pinger.failures += 1;
        pinger.next_retry_at = Some(at + step);
    }
}

fn retry_after_ms(header: Option<&str>, now: i64) -> i64 {
    let Some(value) = header.map(str::trim) else {
        return 10_000;
    };
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return value
            .parse::<i64>()
            .map_or(10_000, |seconds| seconds.saturating_mul(1000));
    }
    httpdate::parse_http_date(value)
        .ok()
        .and_then(|date| date.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(10_000, |at| {
            (i64::try_from(at.as_millis()).unwrap_or(i64::MAX) - now).max(0)
        })
}

/// Одно оповещение каталога (`performPing`).
async fn ping(state: &AppState, pinger: &mut Pinger) {
    let sent_version = DOCUMENT_VERSION.load(Ordering::SeqCst);
    let endpoint = state.config.directory_endpoint.trim_end_matches('/');
    let origin = our_origin(&state.config);
    let Some(client) = client(&state.config.version) else {
        return;
    };
    let sent = client
        .post(format!("{endpoint}/v1/ping"))
        .json(&json!({"schema": 1, "origin": origin}))
        .send()
        .await;
    let at = now_ms();
    let response = match sent {
        Ok(response) => response,
        Err(error) => {
            let kind = if error.is_timeout() {
                "timeout"
            } else {
                "network"
            };
            record_failure(state, pinger, at, json!({"at": at, "status": kind}), true).await;
            return;
        }
    };
    let status = response.status().as_u16();
    match status {
        200..=299 => {
            let unchanged = sent_version == DOCUMENT_VERSION.load(Ordering::SeqCst);
            let sql = if unchanged {
                "UPDATE instance_settings SET directory_last_ping_at = ?, directory_last_error = NULL, directory_dirty = 0 WHERE id = 1"
            } else {
                "UPDATE instance_settings SET directory_last_ping_at = ?, directory_last_error = NULL WHERE id = 1"
            };
            record(state, sql, vec![at.into()]).await;
            pinger.failures = 0;
            pinger.next_retry_at = None;
            pinger.cooldown_until = None;
        }
        429 => {
            let header = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            pinger.cooldown_until = Some(at + retry_after_ms(header.as_deref(), at));
        }
        400 => {
            record_failure(
                state,
                pinger,
                at,
                json!({"at": at, "status": "origin"}),
                false,
            )
            .await;
            pinger.halted_version = Some(sent_version);
            tracing::info!(%origin, "каталог отверг адрес этого инстанса; повторов не будет, пока не изменятся настройки");
        }
        410 => {
            record(
                state,
                "UPDATE instance_settings SET directory_dirty = 0 WHERE id = 1",
                Vec::new(),
            )
            .await;
            record_failure(
                state,
                pinger,
                at,
                json!({"at": at, "status": status}),
                false,
            )
            .await;
            pinger.retired = true;
            tracing::info!("каталог сообщил, что закрыт; оповещения остановлены до перезапуска");
        }
        _ => {
            let reason = if status == 502 {
                response
                    .json::<Value>()
                    .await
                    .ok()
                    .and_then(|body| body["reason"].as_str().map(str::to_owned))
                    .filter(|reason| {
                        matches!(
                            reason.as_str(),
                            "unreachable" | "status" | "invalid" | "origin-mismatch"
                        )
                    })
            } else {
                None
            };
            let error = match reason {
                Some(reason) => json!({"at": at, "status": "fetch", "reason": reason}),
                None => json!({"at": at, "status": status}),
            };
            record_failure(state, pinger, at, error, true).await;
        }
    }
}

/// Минута суток (UTC), в которую инстанс шлёт ежедневное оповещение (`slotMinute`).
fn slot_minute(instance_id: &str) -> i64 {
    let digest = Sha256::digest(instance_id.as_bytes());
    i64::from(u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1440)
}

async fn tick(state: &AppState, pinger: &mut Pinger, boot: bool) {
    if state.config.directory_endpoint.is_empty() || pinger.retired {
        return;
    }
    let Some(directory) = read_state(state).await else {
        return;
    };
    let now = now_ms();
    let due = pinger.next_retry_at.is_none_or(|at| now >= at)
        && pinger.cooldown_until.is_none_or(|at| now >= at);
    if boot {
        if directory.enabled || directory.dirty {
            ping(state, pinger).await;
        }
        return;
    }
    if directory.dirty
        && pinger.halted_version != Some(DOCUMENT_VERSION.load(Ordering::SeqCst))
        && due
    {
        ping(state, pinger).await;
        return;
    }
    if directory.enabled {
        let day_start = now.div_euclid(86_400_000) * 86_400_000;
        let slot_instant = day_start + slot_minute(&state.instance_id) * 60_000;
        let not_yet = directory.last_ping_at.is_none_or(|at| at < slot_instant);
        let not_attempted = directory.last_error_at.is_none_or(|at| at < day_start);
        if now >= slot_instant && not_yet && not_attempted && due {
            ping(state, pinger).await;
        }
    }
}

/// Фоновая работа оповещателя: при запуске и далее раз в минуту.
pub async fn run_pinger(state: AppState) {
    let mut pinger = Pinger::default();
    tick(&state, &mut pinger, true).await;
    let mut timer = tokio::time::interval(Duration::from_mins(1));
    timer.tick().await;
    loop {
        timer.tick().await;
        tick(&state, &mut pinger, false).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_own_pictures_are_published() {
        let origin = "https://chat.example";
        assert_eq!(
            asset_url(Some("a.png".into()), origin).as_deref(),
            Some("https://chat.example/api/uploads/a.png")
        );
        assert_eq!(
            asset_url(Some("/api/uploads/a.png".into()), origin).as_deref(),
            Some("https://chat.example/api/uploads/a.png")
        );
        assert_eq!(
            asset_url(Some("HTTPS://chat.example/x.png".into()), origin).as_deref(),
            Some("HTTPS://chat.example/x.png")
        );
        assert_eq!(
            asset_url(Some("https://evil.example/x.png".into()), origin),
            None
        );
        assert_eq!(asset_url(None, origin), None);
    }

    #[test]
    fn feed_query_is_clamped() {
        let query = FeedQuery::parse(
            Some(&format!("  {}  ", "к".repeat(150))),
            Some("500"),
            Some("-3"),
        );
        assert_eq!(
            (query.q.chars().count(), query.limit, query.offset),
            (100, 100, 0)
        );
        let query = FeedQuery::parse(None, Some("x"), Some("5000"));
        assert_eq!(
            (query.q.as_str(), query.limit, query.offset),
            ("", 50, 1000)
        );
    }

    #[test]
    fn pauses() {
        assert_eq!(retry_after_ms(Some("30"), 0), 30_000);
        assert_eq!(retry_after_ms(None, 0), 10_000);
        assert_eq!(
            retry_after_ms(Some("Thu, 01 Jan 1970 00:01:00 GMT"), 20_000),
            40_000
        );
        assert_eq!(retry_after_ms(Some("soon"), 0), 10_000);
        assert!((0..1440).contains(&slot_minute("abc")));
    }
}
