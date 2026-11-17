//! Телеметрия и сведения об обновлениях для администратора
//! (`routes/adminTelemetry.ts`, `routes/adminUpdates.ts`).
//!
//! Этот сервер телеметрию никуда не отправляет: выбор администратора
//! хранится (клиенту нужен ответ), а предпросмотр показывает, что отправил
//! бы оригинал. О новых выпусках сервер спрашивает GitHub только по запросу
//! администратора и только если это разрешено настройкой.

use std::sync::Mutex;
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Tx};
use crate::error::ApiError;
use crate::params;
use crate::routes::invites::as_admin;
use crate::state::{AppState, now_ms};
use crate::util::field;

type Outcome = Result<Response, ApiError>;

/// Выпуски Rustspace: версия сервера — своя, обновления — оттуда же.
const RELEASES_API: &str = "https://api.github.com/repos/QuddyMusic/Rustspace/releases/latest";
const UPSTREAM_SOURCE: &str = crate::config::SOURCE_URL;
const CACHE_TTL_MS: i64 = 6 * 3_600_000;
const MAX_RESPONSE_BYTES: usize = 512 * 1024;

/// `[major, minor, patch]` из начала строки версии.
fn parse_version(value: &str) -> Option<[u64; 3]> {
    let mut parts = value.trim().splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch: String = parts
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    Some([major, minor, patch.parse().ok()?])
}

/// Пора ли снова спросить администратора о телеметрии (`isAskDue`).
fn ask_due(enabled: Option<bool>, declined: Option<&str>, running: &str) -> bool {
    match (enabled, declined) {
        (None, _) | (Some(false), None) => true,
        (Some(true), _) => false,
        (Some(false), Some(declined)) => match (parse_version(declined), parse_version(running)) {
            (Some(declined), Some(running)) => {
                (running[0], running[1]) > (declined[0], declined[1])
            }
            _ => false,
        },
    }
}

fn telemetry_state(tx: &mut dyn Tx, version: &str) -> DbResult<Value> {
    let row = tx.query_one(
        "SELECT telemetry_enabled, telemetry_id, telemetry_last_day, telemetry_last_error, telemetry_declined_version \
         FROM instance_settings WHERE id = 1",
        params![],
    )?;
    let Some(row) = row else {
        return Ok(
            json!({"enabled": null, "id": null, "lastDay": null, "lastError": null, "askDue": true}),
        );
    };
    let enabled = row.opt_flag("telemetry_enabled")?;
    let last_error = row
        .opt_text("telemetry_last_error")?
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(|error| error["day"].is_string() && error["status"].is_number())
        .map(|error| json!({"day": error["day"], "status": error["status"]}));
    Ok(json!({
        "enabled": enabled,
        "id": row.opt_text("telemetry_id")?,
        "lastDay": row.opt_text("telemetry_last_day")?,
        "lastError": last_error,
        "askDue": ask_due(enabled, row.opt_text("telemetry_declined_version")?.as_deref(), version),
    }))
}

/// `GET /api/admin/telemetry`.
pub async fn telemetry(State(state): State<AppState>, user: AuthUser) -> Outcome {
    let version = state.config.version.clone();
    let done = as_admin(&state, &user, move |tx, _| {
        Ok::<_, ApiError>(telemetry_state(tx, &version)?)
    })
    .await?;
    Ok(done.map_or_else(|refusal| refusal, |value| Json(value).into_response()))
}

/// `PUT /api/admin/telemetry`.
pub async fn set_telemetry(
    State(state): State<AppState>,
    user: AuthUser,
    body: axum::body::Bytes,
) -> Outcome {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let version = state.config.version.clone();
    let done = as_admin(&state, &user, move |tx, _| {
        let Some(enabled) = field(&body, "enabled").and_then(Value::as_bool) else {
            return Err(ApiError::bad_request("validation_failed"));
        };
        let current = telemetry_state(tx, &version)?;
        if enabled && current["enabled"] == json!(true) {
            return Ok(current);
        }
        if enabled {
            tx.execute(
                "UPDATE instance_settings SET telemetry_enabled = 1, telemetry_id = COALESCE(telemetry_id, ?), \
                 telemetry_last_error = NULL, updated_at = ? WHERE id = 1",
                params![uuid::Uuid::new_v4().to_string(), now_ms()],
            )?;
        } else {
            tx.execute(
                "UPDATE instance_settings SET telemetry_enabled = 0, telemetry_declined_version = ?, \
                 telemetry_last_error = NULL, updated_at = ? WHERE id = 1",
                params![&version, now_ms()],
            )?;
        }
        Ok(telemetry_state(tx, &version)?)
    })
    .await?;
    Ok(done.map_or_else(|refusal| refusal, |value| Json(value).into_response()))
}

/// Округление до двух значащих цифр (`roundTwoSignificant`).
fn round_two(value: i64) -> i64 {
    if value < 100 {
        return value.max(0);
    }
    let mut magnitude = 1;
    while value / magnitude >= 100 {
        magnitude *= 10;
    }
    (value + magnitude / 2) / magnitude * magnitude
}

/// День UTC вида `2026-11-12` со сдвигом в днях.
#[must_use]
pub fn utc_day(ms: i64, shift_days: i64) -> String {
    let days = ms.div_euclid(86_400_000) + shift_days;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn count(tx: &mut dyn Tx, sql: &str, values: &[crate::db::Value]) -> DbResult<i64> {
    Ok(tx
        .query_one(sql, values)?
        .map(|row| row.opt_int("n"))
        .transpose()?
        .flatten()
        .unwrap_or(0))
}

/// `GET /api/admin/telemetry/preview`: что отправил бы оригинал.
pub async fn telemetry_preview(State(state): State<AppState>, user: AuthUser) -> Outcome {
    let config = state.config.clone();
    let done = as_admin(&state, &user, move |tx, _| {
        let now = now_ms();
        let today = utc_day(now, 0);
        let current = telemetry_state(tx, &config.version)?;
        let instance = match (current["enabled"].as_bool(), current["id"].as_str()) {
            (Some(true), Some(id)) => id.to_owned(),
            _ => "preview".to_owned(),
        };
        let local = "home_instance IS NULL AND (is_deleted IS NULL OR is_deleted = 0)";
        let active = |tx: &mut dyn Tx, shift: i64| {
            count(
                tx,
                &format!("SELECT count(*) AS n FROM users WHERE {local} AND last_active_day >= ?"),
                params![utc_day(now, shift)],
            )
        };
        let registered = count(tx, &format!("SELECT count(*) AS n FROM users WHERE {local}"), params![])?;
        let active_1d = count(
            tx,
            &format!("SELECT count(*) AS n FROM users WHERE {local} AND last_active_day = ?"),
            params![&today],
        )?;
        let (active_7d, active_30d) = (active(tx, -6)?, active(tx, -29)?);
        let mut clients = [0i64; 3];
        for row in tx.query(
            &format!(
                "SELECT COALESCE(last_client, 'web') AS client, count(*) AS n FROM users \
                 WHERE {local} AND last_active_day >= ? GROUP BY COALESCE(last_client, 'web')"
            ),
            params![utc_day(now, -6)],
        )? {
            let slot = match row.text("client")?.as_str() {
                "desktop" => 1,
                "mobile" => 2,
                _ => 0,
            };
            clients[slot] += row.int("n")?;
        }
        let week_ago = now.div_euclid(86_400_000) * 86_400_000 - 6 * 86_400_000;
        let settings = tx.query_one(
            "SELECT registration_open, federation_relay_enabled, installed_at FROM instance_settings WHERE id = 1",
            params![],
        )?;
        let flag = |name: &str| settings.as_ref().and_then(|row| row.opt_flag(name).ok().flatten());
        let installed = settings.as_ref().and_then(|row| row.opt_int("installed_at").ok().flatten()).unwrap_or(now);
        let storage = count(tx, "SELECT CAST(COALESCE(sum(size), 0) AS BIGINT) AS n FROM attachments WHERE source_url IS NULL", params![])?;
        let voice = config.livekit.url.is_some() && config.livekit.api_key.is_some() && config.livekit.api_secret.is_some();
        let domain_set = config.domain.as_deref().is_some_and(|domain| !domain.is_empty());
        Ok::<_, ApiError>(json!({
            "schema": 1,
            "instance": instance,
            "day": today,
            "build": {
                "version": config.version,
                "commit": config.commit,
                "modified": config.source_code_url != UPSTREAM_SOURCE,
            },
            "users": {
                "registered": round_two(registered),
                "active1d": round_two(active_1d),
                "active7d": round_two(active_7d),
                "active30d": round_two(active_30d),
            },
            "clients": {"web": round_two(clients[0]), "desktop": round_two(clients[1]), "mobile": round_two(clients[2])},
            "content": {
                "spaces": round_two(count(tx, "SELECT count(*) AS n FROM spaces", params![])?),
                "channels": round_two(count(tx, "SELECT count(*) AS n FROM channels", params![])?),
                "messages": round_two(count(tx, "SELECT count(*) AS n FROM messages", params![])?),
                "messages7d": round_two(count(tx, "SELECT count(*) AS n FROM messages WHERE created_at >= ?", params![week_ago])?),
                "storageMiB": round_two(storage / (1024 * 1024)),
            },
            "features": {
                "voice": voice,
                "federation": domain_set && flag("federation_relay_enabled") == Some(true),
                "peers": round_two(count(tx, "SELECT count(*) AS n FROM federation_peers WHERE status = 'active'", params![])?),
                "registrationOpen": flag("registration_open").unwrap_or(config.registration_open),
            },
            "runtime": {
                "install": config.install_channel.as_deref().filter(|channel| matches!(*channel, "prebuilt" | "source")),
                "os": std::env::consts::OS,
                "arch": match std::env::consts::ARCH { "x86_64" => "x64", "aarch64" => "arm64", other => other },
                "node": 0,
            },
            "installedAt": utc_day(installed, 0)[..7].to_owned(),
        }))
    })
    .await?;
    Ok(done.map_or_else(|refusal| refusal, |value| Json(value).into_response()))
}

/// Последний выпуск из ответа GitHub (`parseLatestRelease`).
fn parse_release(payload: &Value) -> Option<Value> {
    if !payload.is_object()
        || payload["draft"] == json!(true)
        || payload["prerelease"] == json!(true)
    {
        return None;
    }
    let tag = payload["tag_name"]
        .as_str()
        .map(str::trim)
        .unwrap_or_default();
    let version = tag.strip_prefix('v').unwrap_or(tag);
    parse_version(version)?;
    let url = payload["html_url"]
        .as_str()
        .filter(|url| url.starts_with("https://github.com/"))
        .map_or_else(
            || format!("{UPSTREAM_SOURCE}/releases/tag/{tag}"),
            str::to_owned,
        );
    Some(
        json!({"version": version, "url": url, "publishedAt": payload["published_at"].as_str().unwrap_or_default()}),
    )
}

/// Итог проверки: выпуск, причина неудачи, время.
type Check = (Option<Value>, Option<&'static str>, i64);

static RELEASE_CACHE: Mutex<Option<(Check, i64)>> = Mutex::new(None);

async fn lookup() -> Check {
    let at = now_ms();
    let failed = |reason| (None, Some(reason), at);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("rustspace-server/", env!("CARGO_PKG_VERSION")))
        .build();
    let Ok(client) = client else {
        return failed("unreachable");
    };
    let Ok(response) = client
        .get(RELEASES_API)
        .header("accept", "application/vnd.github+json")
        .send()
        .await
    else {
        return failed("unreachable");
    };
    match response.status().as_u16() {
        403 | 429 => return failed("rate-limited"),
        200..=299 => {}
        _ => return failed("unreachable"),
    }
    let (body, truncated) =
        crate::net::read_limited(response, MAX_RESPONSE_BYTES, Duration::from_secs(5)).await;
    if truncated {
        return failed("unparseable");
    }
    match serde_json::from_slice::<Value>(&body)
        .ok()
        .as_ref()
        .and_then(parse_release)
    {
        Some(latest) => (Some(latest), None, at),
        None => failed("unparseable"),
    }
}

async fn latest_release(enabled: bool, force: bool) -> Check {
    if !enabled {
        return (None, Some("disabled"), now_ms());
    }
    if !force
        && let Some((check, expires)) = RELEASE_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        && expires > now_ms()
    {
        return check;
    }
    let check = lookup().await;
    let ttl = if check.0.is_some() {
        CACHE_TTL_MS
    } else {
        CACHE_TTL_MS / 10
    };
    *RELEASE_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((check.clone(), now_ms() + ttl));
    check
}

/// Параметры запроса сведений об обновлении.
#[derive(Debug, Default, serde::Deserialize)]
pub struct UpdateQuery {
    refresh: Option<String>,
}

/// `GET /api/admin/instance/update-status`.
pub async fn update_status(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<UpdateQuery>,
) -> Outcome {
    if let Err(refusal) = as_admin(&state, &user, |_, _| Ok::<_, ApiError>(())).await? {
        return Ok(refusal);
    }
    let config = &state.config;
    let updater = updater_status(config);
    let (latest, reason, checked_at) = latest_release(
        config.update_check || updater["available"] == true,
        query.refresh.as_deref() == Some("true"),
    )
    .await;
    let status = latest
        .as_ref()
        .and_then(|latest| {
            Some((
                parse_version(&config.version)?,
                parse_version(latest["version"].as_str()?)?,
            ))
        })
        .map_or("unknown", |(current, newest)| {
            if current < newest {
                "update-available"
            } else {
                "up-to-date"
            }
        });
    let channel = match config.install_channel.as_deref() {
        Some("prebuilt") => "prebuilt",
        Some("source") => "source",
        _ => "unknown",
    };
    Ok(Json(json!({
        "current": {"version": config.version, "commit": config.commit},
        "latest": latest,
        "state": status,
        "checkedAt": if reason == Some("disabled") { Value::Null } else { json!(checked_at) },
        "checkEnabled": config.update_check || updater["available"] == true,
        "reason": reason,
        "channel": channel,
        "selfUpdate": updater,
    }))
    .into_response())
}

/// Помощник обновления: стоит ли и что делает (`status.json` пишет он).
fn updater_status(config: &crate::config::Config) -> Value {
    let Some(dir) = config
        .updater_dir
        .as_deref()
        .filter(|d| d.join("enabled").exists())
    else {
        return json!({"available": false});
    };
    let mut status = std::fs::read(dir.join("status.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({"state": "idle"}));
    if dir.join("request.json").exists() {
        status["state"] = json!("pending");
    }
    status["available"] = json!(true);
    status
}

/// Тело заявки на обновление.
#[derive(Debug, Default, serde::Deserialize)]
pub struct UpdateRequest {
    version: Option<String>,
}

fn plain_version(version: &str) -> bool {
    let mut parts = version.splitn(2, ['-', '+']);
    let core = parts.next().unwrap_or_default();
    let numbers: Vec<&str> = core.split('.').collect();
    numbers.len() == 3
        && numbers
            .iter()
            .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        && parts.next().is_none_or(|rest| {
            !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.')
        })
}

/// `POST /api/rustspace/admin/update` `{version?}` — заявка помощнику:
/// поставить выпуск (по умолчанию последний). Сервер root-прав не получает
/// и сам ничего не качает: выпуск берёт помощник из репозитория, заданного
/// при установке, и сверяет его контрольную сумму.
pub async fn request_update(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<UpdateRequest>,
) -> Outcome {
    if let Err(refusal) = as_admin(&state, &user, |_, _| Ok::<_, ApiError>(())).await? {
        return Ok(refusal);
    }
    let current = updater_status(&state.config);
    let Some(dir) = state
        .config
        .updater_dir
        .clone()
        .filter(|_| current["available"] == true)
    else {
        return Err(ApiError::conflict("updater_unavailable"));
    };
    if matches!(current["state"].as_str(), Some("pending" | "running")) {
        return Err(ApiError::conflict("update_in_progress"));
    }
    let version = body
        .version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("latest");
    let version = version.strip_prefix('v').unwrap_or(version);
    if version != "latest" && !plain_version(version) {
        return Err(ApiError::bad_request("validation_failed"));
    }
    let request = json!({"version": version, "requestedBy": user.id, "at": now_ms()});
    let temporary = dir.join("request.json.tmp");
    let written = tokio::fs::write(&temporary, request.to_string()).await;
    if written.is_err()
        || tokio::fs::rename(&temporary, dir.join("request.json"))
            .await
            .is_err()
    {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(ApiError::internal());
    }
    tracing::info!(version, admin = %user.id, "заявка на обновление сервера");
    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(updater_status(&state.config)),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_rounding() {
        assert_eq!(parse_version("1.12.3-beta"), Some([1, 12, 3]));
        assert_eq!(parse_version("1.2"), None);
        assert!(ask_due(None, None, "1.0.0"));
        assert!(!ask_due(Some(true), None, "1.0.0"));
        assert!(ask_due(Some(false), Some("1.2.0"), "1.3.0"));
        assert!(!ask_due(Some(false), Some("1.2.0"), "1.2.9"));
        assert!(!ask_due(Some(false), Some("2.0.0"), "1.9.0"));
        assert_eq!(
            [
                round_two(-4),
                round_two(99),
                round_two(149),
                round_two(1234),
                round_two(98_765)
            ],
            [0, 99, 150, 1200, 99_000]
        );
        assert_eq!(utc_day(0, 0), "1970-01-01");
        assert_eq!(utc_day(1_794_431_256_779, -6), "2026-11-05");
    }

    #[test]
    fn releases() {
        let release = parse_release(
            &json!({"tag_name": "v1.4.0", "html_url": "https://evil.example/x", "published_at": "2026-01-01"}),
        );
        assert_eq!(
            release,
            Some(
                json!({"version": "1.4.0", "url": "https://github.com/QuddyMusic/Rustspace/releases/tag/v1.4.0", "publishedAt": "2026-01-01"})
            )
        );
        assert_eq!(
            parse_release(&json!({"tag_name": "v1.4.0", "prerelease": true})),
            None
        );
        assert_eq!(parse_release(&json!({"tag_name": "nightly"})), None);
    }
}
