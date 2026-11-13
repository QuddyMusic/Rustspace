//! События личных переписок по WebSocket.

use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::db::{DbResult, Tx};
use crate::error::error_text;
use crate::events::Context;
use crate::hub::Outbox;
use crate::routes::users::{sanitize_user, user_row};
use crate::state::{AppState, now_ms};
use crate::{broadcast, dm, federation, params};

fn refuse(out: &mut Outbox, cx: &Context, code: &'static str, details: Option<Value>) {
    let mut event = json!({
        "type": "error",
        "message": error_text(code, details.as_ref().and_then(Value::as_object)),
        "code": code,
    });
    if let Some(details) = details {
        event["details"] = details;
    }
    out.to_connection(cx.connection, &event);
}

/// Обрабатывает событие переписки. `false` — тип не из этой группы.
/// В `embeds` возвращается сообщение, для которого надо посчитать встраивания.
///
/// # Errors
/// Ошибка базы.
pub fn handle(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    cx: &Context,
    kind: &str,
    event: &Map<String, Value>,
    stale_files: &mut Vec<String>,
    embeds: &mut Option<(String, Option<String>, crate::embeds::Place)>,
) -> DbResult<bool> {
    let text = |name: &str| {
        event
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    match kind {
        "dm_message_create" => {
            let limited = !app.config.disable_rate_limits
                && !app
                    .hub
                    .allow(&format!("dm-send:{}", cx.ip), 5, Duration::from_secs(5));
            if limited {
                refuse(out, cx, "rate_limited", None);
                return Ok(true);
            }
            let Some(channel) = text("dmChannelId") else {
                refuse(out, cx, "validation_failed", None);
                return Ok(true);
            };
            let fields = Value::Object(event.clone());
            match dm::check_create(tx, channel, &cx.user_id, &fields)? {
                Err(refusal) => refuse(out, cx, refusal.code, refusal.details),
                Ok(new) => {
                    match dm::create_message(
                        tx,
                        out,
                        &app.config,
                        &app.ids,
                        channel,
                        &cx.user_id,
                        &new,
                    )? {
                        Some(message) => {
                            *embeds = Some((
                                message["id"].as_str().unwrap_or_default().to_owned(),
                                new.content,
                                crate::embeds::Place::Dm {
                                    dm_channel_id: channel.to_owned(),
                                },
                            ));
                        }
                        None => refuse(out, cx, "message_create_failed", None),
                    }
                }
            }
        }
        "dm_message_edit" => {
            let Some(message_id) = text("messageId") else {
                refuse(out, cx, "validation_failed", None);
                return Ok(true);
            };
            match dm::check_edit(tx, message_id, &cx.user_id, event.get("content"))? {
                Err(refusal) => refuse(out, cx, refusal.code, refusal.details),
                Ok((row, content)) => {
                    let channel = row.text("dm_channel_id")?;
                    match dm::edit_message(tx, out, &app.config, &app.ids, &row, &content)? {
                        Some(_) => {
                            *embeds = Some((
                                message_id.to_owned(),
                                Some(content),
                                crate::embeds::Place::Dm {
                                    dm_channel_id: channel,
                                },
                            ));
                        }
                        None => refuse(out, cx, "message_update_failed", None),
                    }
                }
            }
        }
        "dm_message_delete" => {
            let Some(message_id) = text("messageId") else {
                refuse(out, cx, "validation_failed", None);
                return Ok(true);
            };
            match dm::check_delete(tx, message_id, &cx.user_id)? {
                Err(refusal) => refuse(out, cx, refusal.code, refusal.details),
                Ok(row) => {
                    *stale_files =
                        dm::delete_message(tx, out, &app.config, &app.ids, &row, &cx.user_id)?;
                }
            }
        }
        "dm_typing_start" => {
            let Some(channel) = text("dmChannelId") else {
                return Ok(true);
            };
            if !dm::is_member(tx, channel, &cx.user_id)? {
                return Ok(true);
            }
            broadcast::to_dm_members(
                tx,
                out,
                channel,
                &json!({
                    "type": "dm_typing",
                    "dmChannelId": channel,
                    "userId": cx.user_id,
                    "username": cx.username,
                }),
                Some(&cx.user_id),
            )?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Реакция на личное сообщение: поставить (`add`) или снять.
///
/// # Errors
/// Ошибка базы.
pub fn reaction(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    cx: &Context,
    message_id: &str,
    emoji: &str,
    add: bool,
) -> DbResult<()> {
    let Some(message) = dm::message_row(tx, message_id)? else {
        return Ok(());
    };
    let channel = message.text("dm_channel_id")?;
    if !dm::is_member(tx, &channel, &cx.user_id)?
        || dm::is_dead_one_on_one(tx, &channel, &cx.user_id)?
    {
        return Ok(());
    }
    if add {
        let reaction_id = app.ids.next();
        let now = now_ms();
        let inserted = tx.execute(
            "INSERT INTO dm_reactions (id, dm_message_id, user_id, emoji, created_at) \
             VALUES (?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
            params![&reaction_id, message_id, &cx.user_id, emoji, now],
        )?;
        if inserted == 0 {
            return Ok(());
        }
        let mut reaction = json!({
            "id": reaction_id,
            "messageId": message_id,
            "userId": cx.user_id,
            "emoji": emoji,
            "createdAt": now,
        });
        if let Some(user) = user_row(tx, &cx.user_id)? {
            reaction["user"] = sanitize_user(&user, false)?;
        }
        broadcast::to_dm_members(
            tx,
            out,
            &channel,
            &json!({"type": "reaction_added", "messageId": message_id, "reaction": reaction}),
            None,
        )?;
        federation::relay::reaction(
            tx,
            federation::relay::Relay {
                config: &app.config,
                ids: &app.ids,
            },
            &channel,
            message_id,
            &cx.user_id,
            emoji,
            Some((&reaction_id, now)),
        )?;
        return Ok(());
    }
    let removed = tx.execute(
        "DELETE FROM dm_reactions WHERE dm_message_id = ? AND user_id = ? AND emoji = ?",
        params![message_id, &cx.user_id, emoji],
    )?;
    if removed == 0 {
        return Ok(());
    }
    broadcast::to_dm_members(
        tx,
        out,
        &channel,
        &json!({
            "type": "reaction_removed",
            "messageId": message_id,
            "userId": cx.user_id,
            "emoji": emoji,
        }),
        None,
    )?;
    federation::relay::reaction(
        tx,
        federation::relay::Relay {
            config: &app.config,
            ids: &app.ids,
        },
        &channel,
        message_id,
        &cx.user_id,
        emoji,
        None,
    )
}
