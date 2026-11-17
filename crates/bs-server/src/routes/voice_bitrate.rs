//! Битрейт голоса в звонках (Rustspace; у оригинала его нет).
//!
//! Оригинал публикует микрофон с битрейтом LiveKit по умолчанию — для
//! голоса этого мало. Здесь администратор инстанса задаёт битрейт, а
//! управляющие голосовым каналом и участники личной переписки могут его
//! понизить, если связь у собеседников не тянет.
//!
//! - `GET /api/rustspace/admin/voice-bitrate` → `{kbps}`;
//! - `PUT /api/rustspace/admin/voice-bitrate` `{kbps}` — только администратор;
//! - `GET /api/rustspace/voice-bitrate/:target` — канал или переписка:
//!   `{instanceKbps, overrideKbps, effectiveKbps, canEdit}`;
//! - `PUT /api/rustspace/voice-bitrate/:target` `{kbps | null}` — понизить или
//!   вернуть битрейт инстанса.

use axum::Json;
use axum::extract::{Path, State};
use bs_permissions::Permissions;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::JsonBody;
use crate::messages::channel_space;
use crate::routes::invites::is_admin;
use crate::state::{AppState, now_ms};
use crate::{params, perms};

/// Битрейт инстанса по умолчанию, кбит/с: Opus на нём звучит чисто.
pub const DEFAULT_KBPS: i64 = 96;
/// Пределы для инстанса (Opus: 6–510 кбит/с).
const INSTANCE_RANGE: (i64, i64) = (16, 510);
/// Ниже этого голос разваливается.
const MIN_OVERRIDE: i64 = 8;

/// Битрейт инстанса.
///
/// # Errors
/// Ошибка базы.
pub fn instance_kbps(tx: &mut dyn Tx) -> DbResult<i64> {
    Ok(tx
        .query_one(
            "SELECT value FROM bs_settings WHERE key = 'voice_bitrate_kbps'",
            params![],
        )?
        .and_then(|row| row.text("value").ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_KBPS))
}

/// Может ли пользователь видеть и менять звонок `target` (канал или ЛС).
fn access(tx: &mut dyn Tx, user: &str, target: &str) -> ApiResult<bool> {
    if let Some(space) = channel_space(tx, target)? {
        if !perms::has_channel_permission(tx, user, &space, target, Permissions::VIEW_CHANNEL)? {
            return Err(ApiError::not_found("channel_not_found"));
        }
        return Ok(perms::has_channel_permission(
            tx,
            user,
            &space,
            target,
            Permissions::MANAGE_CHANNELS,
        )?);
    }
    if crate::dm::is_member(tx, target, user)? {
        return Ok(true);
    }
    Err(ApiError::not_found("channel_not_found"))
}

fn override_kbps(tx: &mut dyn Tx, target: &str) -> DbResult<Option<i64>> {
    tx.query_one(
        "SELECT kbps FROM bs_voice_bitrate WHERE target_id = ?",
        params![target],
    )?
    .map(|row| row.int("kbps"))
    .transpose()
}

fn describe(tx: &mut dyn Tx, target: &str, can_edit: bool) -> DbResult<Value> {
    let instance = instance_kbps(tx)?;
    let own = override_kbps(tx, target)?;
    Ok(json!({
        "instanceKbps": instance,
        "overrideKbps": own,
        "effectiveKbps": own.map_or(instance, |own| own.min(instance)),
        "canEdit": can_edit,
        "minKbps": MIN_OVERRIDE,
    }))
}

/// `GET /api/rustspace/voice-bitrate/:target`.
pub async fn get(
    State(state): State<AppState>,
    user: AuthUser,
    Path(target): Path<String>,
) -> ApiResult<Json<Value>> {
    let body = state
        .run(move |tx, _| {
            let can_edit = access(tx, &user.id, &target)?;
            Ok(describe(tx, &target, can_edit)?)
        })
        .await?;
    Ok(Json(body))
}

/// `PUT /api/rustspace/voice-bitrate/:target`.
pub async fn put(
    State(state): State<AppState>,
    user: AuthUser,
    Path(target): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let wanted = match body.get("kbps") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_i64()
                .ok_or_else(|| ApiError::bad_request("voice_bitrate_invalid"))?,
        ),
    };
    let body = state
        .run(move |tx, _| {
            if !access(tx, &user.id, &target)? {
                return Err(perms::missing("MANAGE_CHANNELS"));
            }
            let instance = instance_kbps(tx)?;
            match wanted {
                // Не выше инстанса: понижать можно, повышать — нет.
                Some(kbps) if !(MIN_OVERRIDE..=instance).contains(&kbps) => {
                    return Err(ApiError::bad_request("voice_bitrate_out_of_range")
                        .detail("min", MIN_OVERRIDE)
                        .detail("max", instance));
                }
                Some(kbps) => {
                    tx.execute("DELETE FROM bs_voice_bitrate WHERE target_id = ?", params![&target])?;
                    tx.execute(
                        "INSERT INTO bs_voice_bitrate (target_id, kbps, set_by, updated_at) VALUES (?, ?, ?, ?)",
                        params![&target, kbps, &user.id, now_ms()],
                    )?;
                }
                None => {
                    tx.execute("DELETE FROM bs_voice_bitrate WHERE target_id = ?", params![&target])?;
                }
            }
            Ok(describe(tx, &target, true)?)
        })
        .await?;
    Ok(Json(body))
}

/// `GET /api/rustspace/admin/voice-bitrate`.
pub async fn instance_get(
    State(state): State<AppState>,
    _user: AuthUser,
) -> ApiResult<Json<Value>> {
    let kbps = state.run(|tx, _| Ok(instance_kbps(tx)?)).await?;
    Ok(Json(
        json!({"kbps": kbps, "min": INSTANCE_RANGE.0, "max": INSTANCE_RANGE.1}),
    ))
}

/// `PUT /api/rustspace/admin/voice-bitrate`.
pub async fn instance_put(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let kbps = body
        .get("kbps")
        .and_then(Value::as_i64)
        .filter(|kbps| (INSTANCE_RANGE.0..=INSTANCE_RANGE.1).contains(kbps))
        .ok_or_else(|| {
            ApiError::bad_request("voice_bitrate_out_of_range")
                .detail("min", INSTANCE_RANGE.0)
                .detail("max", INSTANCE_RANGE.1)
        })?;
    state
        .run(move |tx, _| {
            if !is_admin(tx, &user.id)? {
                return Err(ApiError::forbidden("admin_required"));
            }
            tx.execute(
                "DELETE FROM bs_settings WHERE key = 'voice_bitrate_kbps'",
                params![],
            )?;
            tx.execute(
                "INSERT INTO bs_settings (key, value) VALUES ('voice_bitrate_kbps', ?)",
                params![kbps.to_string()],
            )?;
            Ok(())
        })
        .await?;
    Ok(Json(
        json!({"kbps": kbps, "min": INSTANCE_RANGE.0, "max": INSTANCE_RANGE.1}),
    ))
}
