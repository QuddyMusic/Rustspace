//! Настройки инстанса: лимиты трансляций и админские параметры
//! (`routes/settings.ts`).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::auth::AuthUser;
use crate::config::Config;
use crate::db::{DbResult, Row, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::JsonBody;
use crate::params;
use crate::routes::invites::as_admin;
use crate::state::{AppState, now_ms};
use crate::util::{field, js_trim, truthy, utf16_len};

const RESOLUTIONS: [i64; 5] = [540, 720, 1080, 1440, 2160];
const FRAMERATES: [i64; 6] = [30, 45, 60, 75, 90, 120];
/// `Number.MAX_SAFE_INTEGER / 2^20`, округлённое вниз.
const MAX_UPLOAD_MB: i64 = 8_589_934_591;

fn settings_row(tx: &mut dyn Tx) -> ApiResult<Row> {
    tx.query_one("SELECT * FROM instance_settings WHERE id = 1", params![])?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "instance_settings_missing",
            )
        })
}

fn flag(row: &Row, column: &str) -> DbResult<bool> {
    Ok(row.opt_flag(column)? == Some(true))
}

fn limits_json(config: &Config, row: &Row) -> DbResult<Value> {
    let resolutions: Vec<Value> = row
        .opt_text("allowed_resolutions")?
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter_map(|part| {
            if part == "native" {
                Some(json!("native"))
            } else {
                part.parse::<i64>()
                    .ok()
                    .filter(|n| RESOLUTIONS.contains(n))
                    .map(|n| json!(n))
            }
        })
        .collect();
    let framerates: Vec<i64> = row
        .opt_text("allowed_framerates")?
        .unwrap_or_default()
        .split(',')
        .filter_map(|part| part.trim().parse::<i64>().ok())
        .filter(|n| FRAMERATES.contains(n))
        .collect();
    let overrides = row
        .opt_text("bitrate_matrix_overrides")?
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter(|value| value.as_object().is_some_and(|map| !map.is_empty()))
        .unwrap_or(Value::Null);
    Ok(json!({
        "maxBitrateKbps": row.opt_number("max_bitrate_kbps")?,
        "minBitrateKbps": row.opt_number("min_bitrate_kbps")?,
        "bitrateStepKbps": row.opt_number("bitrate_step_kbps")?,
        "allowedResolutions": resolutions,
        "allowedFramerates": framerates,
        "maxResolution": row.opt_number("max_resolution")?,
        "maxFramerate": row.opt_number("max_framerate")?,
        "discoveryEnabled": flag(row, "discovery_enabled")?,
        "directoryEnabled": flag(row, "directory_enabled")?,
        "directoryConfigured": !config.directory_endpoint.is_empty(),
        "bitrateMatrixOverrides": overrides,
        "allowCustomBitrate": flag(row, "allow_custom_bitrate")?,
    }))
}

/// Последняя ошибка обращения к каталогу в проверенном виде.
fn directory_error(raw: Option<String>) -> Value {
    let Some(parsed) = raw.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()) else {
        return Value::Null;
    };
    let (Some(at), Some(status)) = (
        parsed.get("at").filter(|at| at.is_number()),
        parsed.get("status"),
    ) else {
        return Value::Null;
    };
    let known = status.is_number()
        || status
            .as_str()
            .is_some_and(|s| matches!(s, "network" | "timeout" | "invalid" | "refused"));
    if !known {
        return Value::Null;
    }
    let mut out = json!({"at": at, "status": status});
    if let Some(reason) = parsed.get("reason").filter(|reason| reason.is_string()) {
        out["reason"] = reason.clone();
    }
    out
}

fn admin_json(tx: &mut dyn Tx, config: &Config, row: &Row) -> DbResult<Value> {
    let gif_key = row.opt_text("gif_api_key")?.filter(|key| !key.is_empty());
    let upload_bytes = row
        .opt_int("max_upload_size_bytes")?
        .unwrap_or_else(|| i64::try_from(config.max_upload_size).unwrap_or(i64::MAX));
    let listed = tx
        .query_one(
            "SELECT count(*) AS n FROM spaces s WHERE s.directory_listed = 1 AND s.visibility <> 'private'",
            params![],
        )?
        .map_or(Ok(0), |row| row.int("n"))?;
    #[allow(
        clippy::cast_precision_loss,
        reason = "размеры загрузок далеки от 2^53"
    )]
    #[allow(clippy::cast_possible_truncation, reason = "округлённые мегабайты")]
    let upload_mb = (upload_bytes as f64 / 1_048_576.0).round() as i64;
    let mut settings = json!({
        "instanceName": row.opt_text("instance_name")?.unwrap_or_else(|| "Rustspace".to_owned()),
        "registrationOpen": row.opt_flag("registration_open")?.unwrap_or(config.registration_open),
        "federatedRegistrationOpen": flag(row, "federated_registration_open")?,
        "discoveryEnabled": flag(row, "discovery_enabled")?,
        "gifEnabled": gif_key.is_some(),
        "maxUploadSizeMb": upload_mb,
        "federationRelayEnabled": flag(row, "federation_relay_enabled")?,
        "federationRelayTtlDays": row.opt_number("federation_relay_ttl_days")?,
        "defaultAutoRotateIntervalDays": row.opt_number("default_auto_rotate_interval_days")?,
        "autoAcceptPeering": flag(row, "auto_accept_peering")?,
        "directoryEnabled": flag(row, "directory_enabled")?,
        "directoryBrowseEnabled": flag(row, "directory_browse_enabled")?,
        "directoryLastPingAt": row.opt_int("directory_last_ping_at")?,
        "directoryLastError": directory_error(row.opt_text("directory_last_error")?),
        "directoryListedSpaceCount": listed,
        "supportCardEnabled": flag(row, "support_card_enabled")?,
    });
    if let Some(key) = gif_key {
        let tail: String = key
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        settings["gifApiKey"] = json!(format!("****{tail}"));
    }
    Ok(settings)
}

type Sets = Vec<(&'static str, crate::db::Value)>;

fn pending(sets: &Sets, column: &str) -> Option<crate::db::Value> {
    sets.iter()
        .find(|(name, _)| *name == column)
        .map(|(_, value)| value.clone())
}

/// Каталог работает только при включённом обнаружении
/// (`applyDiscoveryAndDirectory`).
fn discovery_and_directory(body: &Value, sets: &mut Sets, current: &Row) -> ApiResult<()> {
    let discovery_on = match pending(sets, "discovery_enabled") {
        Some(crate::db::Value::Int(value)) => value == 1,
        _ => flag(current, "discovery_enabled")?,
    };
    if let Some(value) = field(body, "directoryEnabled") {
        let wanted = value.as_bool().ok_or_else(|| {
            ApiError::bad_request("field_not_boolean").detail("field", "directoryEnabled")
        })?;
        if wanted && !discovery_on {
            return Err(ApiError::bad_request("directory_requires_discovery"));
        }
        sets.push(("directory_enabled", wanted.into()));
    }
    if !discovery_on {
        sets.retain(|(name, _)| *name != "directory_enabled");
        sets.push(("directory_enabled", false.into()));
    }
    Ok(())
}

fn write(tx: &mut dyn Tx, mut sets: Sets, current: &Row) -> DbResult<()> {
    let mut dirty = false;
    for column in [
        "directory_enabled",
        "discovery_enabled",
        "federated_registration_open",
    ] {
        if let Some(crate::db::Value::Int(value)) = pending(&sets, column)
            && (value == 1) != flag(current, column)?
        {
            dirty = true;
        }
    }
    if let Some(crate::db::Value::Text(name)) = pending(&sets, "instance_name")
        && name
            != current
                .opt_text("instance_name")?
                .unwrap_or_else(|| "Rustspace".to_owned())
    {
        dirty = true;
    }
    sets.push(("updated_at", now_ms().into()));
    let assignments: Vec<String> = sets
        .iter()
        .map(|(column, _)| format!("{column} = ?"))
        .collect();
    let values: Vec<crate::db::Value> = sets.into_iter().map(|(_, value)| value).collect();
    tx.execute(
        &format!(
            "UPDATE instance_settings SET {} WHERE id = 1",
            assignments.join(", ")
        ),
        &values,
    )?;
    if dirty {
        crate::routes::spaces::mark_directory_dirty(tx)?;
    }
    Ok(())
}

/// `GET /api/settings/streaming`.
pub async fn streaming(State(state): State<AppState>, _user: AuthUser) -> ApiResult<Json<Value>> {
    let config = state.config.clone();
    state
        .run(move |tx, _| {
            let row = settings_row(tx)?;
            Ok(Json(limits_json(&config, &row)?))
        })
        .await
}

fn bounded(value: &Value, min: f64, max: f64) -> Option<f64> {
    value.as_f64().filter(|n| *n >= min && *n <= max)
}

fn out_of_range(code: &'static str, min: i64, max: i64) -> ApiError {
    ApiError::bad_request(code)
        .detail("min", min)
        .detail("max", max)
}

fn join_values(values: &[Value]) -> String {
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn list_of(values: &[i64]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn respond(outcome: ApiResult<Result<Json<Value>, Response>>) -> Response {
    match outcome {
        Ok(Ok(value)) => value.into_response(),
        Ok(Err(refused)) => refused,
        Err(error) => error.into_response(),
    }
}

/// `PATCH /api/settings/streaming`.
pub async fn update_streaming(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> Response {
    let config = state.config.clone();
    respond(
        as_admin(&state, &user, move |tx, _| {
            let mut sets: Sets = Vec::new();
            for (name, column, min, max, code) in [
                (
                    "maxBitrateKbps",
                    "max_bitrate_kbps",
                    500,
                    1_000_000,
                    "streaming_max_bitrate_out_of_range",
                ),
                (
                    "minBitrateKbps",
                    "min_bitrate_kbps",
                    100,
                    1_000_000,
                    "streaming_min_bitrate_out_of_range",
                ),
                (
                    "bitrateStepKbps",
                    "bitrate_step_kbps",
                    50,
                    5000,
                    "streaming_bitrate_step_out_of_range",
                ),
            ] {
                if let Some(value) = field(&body, name) {
                    #[allow(clippy::cast_precision_loss, reason = "малые пределы")]
                    let number = bounded(value, min as f64, max as f64)
                        .ok_or_else(|| out_of_range(code, min, max))?;
                    sets.push((column, number.into()));
                }
            }
            if let Some(value) = field(&body, "allowedResolutions") {
                let list = value
                    .as_array()
                    .filter(|list| !list.is_empty())
                    .ok_or_else(|| ApiError::bad_request("streaming_resolutions_required"))?;
                let invalid: Vec<Value> = list
                    .iter()
                    .filter(|r| {
                        *r != "native" && !r.as_i64().is_some_and(|n| RESOLUTIONS.contains(&n))
                    })
                    .cloned()
                    .collect();
                if !invalid.is_empty() {
                    return Err(ApiError::bad_request("streaming_resolutions_invalid")
                        .detail("invalid", join_values(&invalid))
                        .detail("allowed", format!("{}, native", list_of(&RESOLUTIONS))));
                }
                let mut numbers: Vec<i64> = list.iter().filter_map(Value::as_i64).collect();
                numbers.sort_unstable();
                let mut parts: Vec<String> = numbers.iter().map(ToString::to_string).collect();
                if list.iter().any(|r| r == "native") {
                    parts.push("native".to_owned());
                }
                sets.push(("allowed_resolutions", parts.join(",").into()));
            }
            if let Some(value) = field(&body, "allowedFramerates") {
                let list = value
                    .as_array()
                    .filter(|list| !list.is_empty())
                    .ok_or_else(|| ApiError::bad_request("streaming_framerates_required"))?;
                let invalid: Vec<Value> = list
                    .iter()
                    .filter(|f| !f.as_i64().is_some_and(|n| FRAMERATES.contains(&n)))
                    .cloned()
                    .collect();
                if !invalid.is_empty() {
                    return Err(ApiError::bad_request("streaming_framerates_invalid")
                        .detail("invalid", join_values(&invalid))
                        .detail("allowed", list_of(&FRAMERATES)));
                }
                let mut numbers: Vec<i64> = list.iter().filter_map(Value::as_i64).collect();
                numbers.sort_unstable();
                sets.push((
                    "allowed_framerates",
                    list_of(&numbers).replace(", ", ",").into(),
                ));
            }
            if let Some(value) = field(&body, "maxResolution") {
                let number = value
                    .as_i64()
                    .filter(|n| RESOLUTIONS.contains(n))
                    .ok_or_else(|| {
                        ApiError::bad_request("streaming_max_resolution_invalid")
                            .detail("allowed", list_of(&RESOLUTIONS))
                    })?;
                sets.push(("max_resolution", number.into()));
            }
            if let Some(value) = field(&body, "maxFramerate") {
                let number = value
                    .as_i64()
                    .filter(|n| FRAMERATES.contains(n))
                    .ok_or_else(|| {
                        ApiError::bad_request("streaming_max_framerate_invalid")
                            .detail("allowed", list_of(&FRAMERATES))
                    })?;
                sets.push(("max_framerate", number.into()));
            }
            if let Some(value) = field(&body, "discoveryEnabled") {
                sets.push(("discovery_enabled", truthy(Some(value)).into()));
            }
            if let Some(value) = field(&body, "allowCustomBitrate") {
                sets.push(("allow_custom_bitrate", truthy(Some(value)).into()));
            }
            if let Some(value) = field(&body, "bitrateMatrixOverrides") {
                match value {
                    Value::Null => sets.push(("bitrate_matrix_overrides", crate::db::Value::Null)),
                    Value::Object(map) => {
                        check_matrix(map)?;
                        sets.push(("bitrate_matrix_overrides", value.to_string().into()));
                    }
                    _ => return Err(ApiError::bad_request("streaming_bitrate_matrix_invalid")),
                }
            }
            let current = settings_row(tx)?;
            let effective = |column: &str| -> DbResult<f64> {
                Ok(match pending(&sets, column) {
                    Some(crate::db::Value::Int(value)) => {
                        #[allow(clippy::cast_precision_loss, reason = "малые значения")]
                        let value = value as f64;
                        value
                    }
                    Some(crate::db::Value::Real(value)) => value,
                    _ => current
                        .opt_number(column)?
                        .and_then(|n| n.as_f64())
                        .unwrap_or(0.0),
                })
            };
            if effective("min_bitrate_kbps")? >= effective("max_bitrate_kbps")? {
                return Err(ApiError::bad_request("streaming_min_bitrate_not_below_max"));
            }
            discovery_and_directory(&Value::Null, &mut sets, &current)?;
            write(tx, sets, &current)?;
            let updated = settings_row(tx)?;
            Ok::<_, ApiError>(Json(limits_json(&config, &updated)?))
        })
        .await,
    )
}

fn check_matrix(map: &Map<String, Value>) -> ApiResult<()> {
    for (key, value) in map {
        let known = key.split_once('_').is_some_and(|(res, fps)| {
            res.parse::<i64>()
                .is_ok_and(|n| RESOLUTIONS.contains(&n) && n.to_string() == res)
                && fps
                    .parse::<i64>()
                    .is_ok_and(|n| FRAMERATES.contains(&n) && n.to_string() == fps)
        });
        if !known {
            return Err(
                ApiError::bad_request("streaming_bitrate_matrix_key_invalid")
                    .detail("key", key.as_str()),
            );
        }
        if !value.as_f64().is_some_and(|n| n > 0.0 && n <= 1_000_000.0) {
            return Err(
                ApiError::bad_request("streaming_bitrate_matrix_value_invalid")
                    .detail("key", key.as_str())
                    .detail("max", 1_000_000),
            );
        }
    }
    Ok(())
}

/// `GET /api/settings/instance`.
pub async fn instance(State(state): State<AppState>, user: AuthUser) -> Response {
    let config = state.config.clone();
    respond(
        as_admin(&state, &user, move |tx, _| {
            let row = settings_row(tx)?;
            Ok::<_, ApiError>(Json(admin_json(tx, &config, &row)?))
        })
        .await,
    )
}

/// `Number(value)` для целого в пределах.
fn whole_in(value: &Value, min: i64, max: i64) -> Option<i64> {
    let number = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => js_trim(text).parse::<f64>().ok()?,
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        _ => return None,
    };
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "проверено пределами"
    )]
    let whole = (number.fract() == 0.0 && number >= min as f64 && number <= max as f64)
        .then_some(number as i64);
    whole
}

/// `PATCH /api/settings/instance`.
pub async fn update_instance(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> Response {
    let config = state.config.clone();
    respond(
        as_admin(&state, &user, move |tx, _| {
            let mut sets: Sets = Vec::new();
            let boolean = |name: &'static str| -> ApiResult<Option<bool>> {
                match field(&body, name) {
                    None => Ok(None),
                    Some(value) => value.as_bool().map(Some).ok_or_else(|| {
                        ApiError::bad_request("field_not_boolean").detail("field", name)
                    }),
                }
            };
            if let Some(value) = field(&body, "instanceName") {
                let name = value
                    .as_str()
                    .map(js_trim)
                    .filter(|name| !name.is_empty() && utf16_len(name) <= 32)
                    .ok_or_else(|| out_of_range("instance_name_length", 1, 32))?;
                sets.push(("instance_name", name.into()));
            }
            for (name, column) in [
                ("registrationOpen", "registration_open"),
                ("discoveryEnabled", "discovery_enabled"),
                ("federationRelayEnabled", "federation_relay_enabled"),
                ("autoAcceptPeering", "auto_accept_peering"),
            ] {
                if let Some(value) = field(&body, name) {
                    sets.push((column, truthy(Some(value)).into()));
                }
            }
            for (name, column) in [
                ("federatedRegistrationOpen", "federated_registration_open"),
                ("directoryBrowseEnabled", "directory_browse_enabled"),
                ("supportCardEnabled", "support_card_enabled"),
            ] {
                if let Some(flag) = boolean(name)? {
                    sets.push((column, flag.into()));
                }
            }
            if let Some(value) = field(&body, "gifApiKey") {
                let masked = value.as_str().is_some_and(|key| key.starts_with("****"));
                if !masked {
                    let key = value.as_str().map(js_trim).filter(|key| !key.is_empty());
                    sets.push(("gif_api_key", key.into()));
                }
            }
            if let Some(value) = field(&body, "maxUploadSizeMb") {
                let mb = whole_in(value, 1, MAX_UPLOAD_MB)
                    .ok_or_else(|| out_of_range("upload_limit_out_of_range", 1, MAX_UPLOAD_MB))?;
                sets.push(("max_upload_size_bytes", (mb * 1_048_576).into()));
            }
            for (name, column, code) in [
                (
                    "federationRelayTtlDays",
                    "federation_relay_ttl_days",
                    "relay_ttl_out_of_range",
                ),
                (
                    "defaultAutoRotateIntervalDays",
                    "default_auto_rotate_interval_days",
                    "rotation_interval_out_of_range",
                ),
            ] {
                if let Some(value) = field(&body, name) {
                    let days = whole_in(value, 1, 365).ok_or_else(|| out_of_range(code, 1, 365))?;
                    sets.push((column, days.into()));
                }
            }
            let current = settings_row(tx)?;
            discovery_and_directory(&body, &mut sets, &current)?;
            write(tx, sets, &current)?;
            let updated = settings_row(tx)?;
            Ok::<_, ApiError>(Json(admin_json(tx, &config, &updated)?))
        })
        .await,
    )
}
