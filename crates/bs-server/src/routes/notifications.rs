//! Настройки уведомлений пространств и каналов (`routes/notificationSettings.ts`).

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Row, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::JsonBody;
use crate::hub::Outbox;
use crate::messages::channel_space;
use crate::state::{AppState, now_ms};
use crate::{params, perms};

const LEVELS: [&str; 3] = ["all", "mentions", "nothing"];
const COLUMNS: &str = "space_id, channel_id, level, muted, muted_until, updated_at, suppress_everyone, suppress_roles";

fn setting_json(row: &Row) -> DbResult<Value> {
    let muted = row.opt_flag("muted")? == Some(true);
    Ok(json!({
        "suppressEveryone": row.opt_flag("suppress_everyone")? == Some(true),
        "suppressRoles": row.opt_flag("suppress_roles")? == Some(true),
        "spaceId": row.text("space_id")?,
        "channelId": row.opt_text("channel_id")?,
        "level": row.opt_text("level")?.filter(|level| LEVELS.contains(&level.as_str())),
        "muted": muted,
        "mutedUntil": if muted { row.opt_int("muted_until")? } else { None },
        "updatedAt": row.int("updated_at")?,
    }))
}

/// Проверенное изменение настройки.
#[derive(Debug)]
struct Change {
    level: Option<Option<String>>,
    mute: Option<Option<String>>,
    suppress_everyone: Option<bool>,
    suppress_roles: Option<bool>,
}

fn parse(body: &Value, channel: bool) -> Option<Change> {
    let object = body.as_object()?;
    let mut change = Change {
        level: None,
        mute: None,
        suppress_everyone: None,
        suppress_roles: None,
    };
    let mut any = false;
    if let Some(value) = object.get("level") {
        any = true;
        change.level = Some(match value {
            Value::Null => None,
            Value::String(level) if LEVELS.contains(&level.as_str()) => Some(level.clone()),
            _ => return None,
        });
    }
    if let Some(value) = object.get("mute") {
        any = true;
        change.mute = Some(match value {
            Value::Null => None,
            Value::String(mute) if matches!(mute.as_str(), "1h" | "8h" | "24h" | "indefinite") => {
                Some(mute.clone())
            }
            _ => return None,
        });
    }
    for (name, slot) in [
        ("suppressEveryone", &mut change.suppress_everyone),
        ("suppressRoles", &mut change.suppress_roles),
    ] {
        if let Some(value) = object.get(name) {
            if channel {
                return None;
            }
            any = true;
            *slot = Some(value.as_bool()?);
        }
    }
    any.then_some(change)
}

fn find(tx: &mut dyn Tx, user: &str, space: &str, channel: Option<&str>) -> DbResult<Option<Row>> {
    match channel {
        Some(channel) => tx.query_one(
            &format!(
                "SELECT {COLUMNS} FROM notification_settings WHERE user_id = ? AND channel_id = ?"
            ),
            params![user, channel],
        ),
        None => tx.query_one(
            &format!(
                "SELECT {COLUMNS} FROM notification_settings WHERE user_id = ? AND space_id = ? \
                 AND channel_id IS NULL"
            ),
            params![user, space],
        ),
    }
}

fn store(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    user: &str,
    space: &str,
    channel: Option<&str>,
    change: Change,
) -> DbResult<Value> {
    let existing = find(tx, user, space, channel)?;
    let now = now_ms();
    let stored = existing.as_ref().map(setting_json).transpose()?;
    let stored_text = |name: &str| {
        stored
            .as_ref()
            .and_then(|s| s[name].as_str().map(str::to_owned))
    };
    let stored_flag = |name: &str| stored.as_ref().is_some_and(|s| s[name] == true);
    let level = change.level.unwrap_or_else(|| stored_text("level"));
    let stored_until = stored.as_ref().and_then(|s| s["mutedUntil"].as_i64());
    let mut muted = stored_flag("muted") && stored_until.is_none_or(|until| until > now);
    let mut muted_until = if muted { stored_until } else { None };
    match change.mute.as_ref().map(Option::as_deref) {
        Some(None) => {
            muted = false;
            muted_until = None;
        }
        Some(Some(duration)) => {
            muted = true;
            muted_until = match duration {
                "1h" => Some(now + 3_600_000),
                "8h" => Some(now + 8 * 3_600_000),
                "24h" => Some(now + 24 * 3_600_000),
                _ => None,
            };
        }
        None => {}
    }
    let suppress_everyone = change
        .suppress_everyone
        .unwrap_or_else(|| stored_flag("suppressEveryone"));
    let suppress_roles = change
        .suppress_roles
        .unwrap_or_else(|| stored_flag("suppressRoles"));
    let previous = existing
        .as_ref()
        .map(|row| row.int("updated_at"))
        .transpose()?
        .unwrap_or(0);
    let updated_at = now.max(previous + 1);
    let setting = json!({
        "spaceId": space,
        "channelId": channel,
        "level": level,
        "muted": muted,
        "mutedUntil": muted_until,
        "suppressEveryone": suppress_everyone,
        "suppressRoles": suppress_roles,
        "updatedAt": updated_at,
    });
    let (filter, key): (&str, Vec<crate::db::Value>) = match channel {
        Some(channel) => (
            "user_id = ? AND channel_id = ?",
            vec![user.into(), channel.into()],
        ),
        None => (
            "user_id = ? AND space_id = ? AND channel_id IS NULL",
            vec![user.into(), space.into()],
        ),
    };
    if level.is_none() && !muted && !suppress_everyone && !suppress_roles {
        if existing.is_some() {
            tx.execute(
                &format!("DELETE FROM notification_settings WHERE {filter}"),
                &key,
            )?;
        }
    } else if existing.is_some() {
        let mut values: Vec<crate::db::Value> = vec![
            suppress_everyone.into(),
            suppress_roles.into(),
            level.into(),
            muted.into(),
            muted_until.into(),
            updated_at.into(),
        ];
        values.extend(key);
        tx.execute(
            &format!(
                "UPDATE notification_settings SET suppress_everyone = ?, suppress_roles = ?, level = ?, \
                 muted = ?, muted_until = ?, updated_at = ? WHERE {filter}"
            ),
            &values,
        )?;
    } else {
        tx.execute(
            "INSERT INTO notification_settings (user_id, space_id, channel_id, suppress_everyone, \
             suppress_roles, level, muted, muted_until, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                user, space, channel, suppress_everyone, suppress_roles, level, muted, muted_until,
                updated_at
            ],
        )?;
    }
    out.to_user(
        user,
        &json!({"type": "notification_settings_updated", "setting": setting}),
    );
    Ok(setting)
}

/// `GET /api/users/@me/notification-settings`.
pub async fn list(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let rows = tx.query(
                &format!("SELECT {COLUMNS} FROM notification_settings WHERE user_id = ?"),
                params![&user.id],
            )?;
            let mut settings = Vec::new();
            for row in &rows {
                let space = row.text("space_id")?;
                if !perms::is_member(tx, &space, &user.id)? {
                    continue;
                }
                if let Some(channel) = row.opt_text("channel_id")?
                    && !perms::can_view(perms::compute(tx, &user.id, &space, Some(&channel))?)
                {
                    continue;
                }
                settings.push(setting_json(row)?);
            }
            Ok(Json(json!({"settings": settings})))
        })
        .await
}

/// `PATCH /api/spaces/:spaceId/notification-settings`.
pub async fn update_space(
    State(state): State<AppState>,
    user: AuthUser,
    Path(space): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let change = parse(&body, false).ok_or_else(|| ApiError::bad_request("validation_failed"))?;
    state
        .run(move |tx, out| {
            if crate::routes::spaces::space_row(tx, &space)?.is_none() {
                return Err(ApiError::not_found("space_not_found"));
            }
            if !perms::is_member(tx, &space, &user.id)? {
                return Err(ApiError::forbidden("not_space_member"));
            }
            Ok(Json(store(tx, out, &user.id, &space, None, change)?))
        })
        .await
}

/// `PATCH /api/channels/:channelId/notification-settings`.
pub async fn update_channel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(channel): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let change = parse(&body, true).ok_or_else(|| ApiError::bad_request("validation_failed"))?;
    state
        .run(move |tx, out| {
            let space = channel_space(tx, &channel)?
                .ok_or_else(|| ApiError::not_found("channel_not_found"))?;
            if !perms::is_member(tx, &space, &user.id)? {
                return Err(ApiError::forbidden("not_space_member"));
            }
            if !perms::can_view(perms::compute(tx, &user.id, &space, Some(&channel))?) {
                return Err(perms::missing("VIEW_CHANNEL"));
            }
            Ok(Json(store(
                tx,
                out,
                &user.id,
                &space,
                Some(&channel),
                change,
            )?))
        })
        .await
}
