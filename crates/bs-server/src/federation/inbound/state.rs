//! Состояние переписок и присутствие (`routes/federation/events/dmState.ts`).

use std::collections::HashSet;

use serde_json::{Value, json};

use super::{
    Ctx, Step, accept, actor, domain, live_channel, present, raw_domain, reject, text, timestamp,
};
use crate::db::{DbResult, Row};
use crate::federation::identity::{Actor, Resolved};
use crate::federation::our_origin;
use crate::params;

fn affected_users(cx: &mut Ctx<'_>, event: &Value) -> DbResult<Vec<Row>> {
    let source_domain = domain(cx.source);
    let identities: Vec<Actor> = match event.get("affectedUsers").and_then(Value::as_array) {
        Some(list) => list
            .iter()
            .filter_map(actor)
            .filter(|who| domain(&who.home_instance) == source_domain)
            .collect(),
        None => event
            .get("affectedUserIds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|id| Actor {
                home_user_id: id.to_owned(),
                home_instance: cx.source.to_owned(),
            })
            .collect(),
    };
    let mut users = Vec::new();
    let mut seen = HashSet::new();
    for who in identities {
        if let Resolved::Found(user) =
            crate::federation::identity::resolve_actor(cx.tx, &cx.app.config, &who)?
        {
            let id = user.text("id")?;
            if seen.insert(id) {
                users.push(*user);
            }
        }
    }
    Ok(users)
}

/// Получатели не смогли скачать вложение (`processFileRejectedEvent`).
pub fn file_rejected(cx: &mut Ctx<'_>, event: &Value) -> Step {
    if present(event, "attachmentId").is_none() || present(event, "rejectionReason").is_none() {
        return reject("missing_file_rejected_payload");
    }
    let id = text(event, "messageId").unwrap_or_default();
    let Some(message) = cx.tx.query_one(
        &format!(
            "SELECT {} FROM dm_messages WHERE id = ?",
            crate::dm::DM_MESSAGE_COLUMNS
        ),
        params![id],
    )?
    else {
        return reject("message_not_found");
    };
    let message_id = message.text("id")?;
    let attachments = cx.tx.query(
        "SELECT id, filename, federation_meta FROM attachments WHERE dm_message_id = ?",
        params![&message_id],
    )?;
    let mut matched = match present(event, "sourceFilename").and_then(Value::as_str) {
        Some(name) => attachments
            .iter()
            .find(|row| row.text("filename").is_ok_and(|f| f == name))
            .cloned(),
        None => None,
    };
    if matched.is_none() && attachments.len() == 1 {
        matched = attachments.first().cloned();
    }
    let Some(matched) = matched else {
        return reject("attachment_not_found");
    };
    let limit = event
        .get("rejectionLimit")
        .filter(|v| !v.is_null())
        .cloned()
        .unwrap_or_else(|| json!(0));
    let mut affected = Vec::new();
    for user in affected_users(cx, event)? {
        let name = user
            .opt_text("display_name")?
            .filter(|n| !n.is_empty())
            .map_or_else(|| user.text("username"), Ok)?;
        affected.push(json!({"userId": user.text("id")?, "username": name, "limit": limit}));
    }
    if affected.is_empty() {
        return accept();
    }
    let mut existing: Vec<Value> = matched
        .opt_text("federation_meta")?
        .and_then(|meta| serde_json::from_str::<Value>(&meta).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let known: HashSet<String> = existing
        .iter()
        .filter_map(|entry| {
            entry
                .get("userId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    let fresh: Vec<Value> = affected
        .iter()
        .filter(|user| !known.contains(user["userId"].as_str().unwrap_or_default()))
        .cloned()
        .collect();
    if fresh.is_empty() {
        return accept();
    }
    existing.extend(fresh);
    let attachment_id = matched.text("id")?;
    cx.tx.execute(
        "UPDATE attachments SET federation_status = 'remote_partial', federation_meta = ? WHERE id = ?",
        params![Value::Array(existing).to_string(), &attachment_id],
    )?;
    if let Some(row) = crate::dm::message_row(cx.tx, &message_id)?
        && let Some(full) = crate::dm::message_json(cx.tx, &row)?
    {
        let channel = message.text("dm_channel_id")?;
        cx.to_dm_members(
            &channel,
            &json!({"type": "dm_message_updated", "message": full}),
        )?;
        cx.to_user(
            &message.text("user_id")?,
            &json!({
                "type": "federation_file_rejected",
                "messageId": message_id,
                "dmChannelId": channel,
                "attachmentId": attachment_id,
                "affectedUsers": affected,
            }),
        )?;
    }
    accept()
}

/// Присутствие пользователя другого инстанса (`processPresenceUpdateEvent`).
pub fn presence(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let Some(payload) = present(event, "presenceUpdate") else {
        return reject("missing_presence_update_payload");
    };
    let home = text(payload, "homeInstance").unwrap_or_default();
    if raw_domain(home) != raw_domain(cx.source) {
        tracing::warn!(home, source = cx.source, "presence_update не от дома");
        return reject("attribution_mismatch");
    }
    let Some(status) =
        text(payload, "status").filter(|s| crate::federation::presence::is_status(s))
    else {
        return reject("invalid_status");
    };
    let user = match cx.resolve(payload)? {
        Resolved::Found(user) => *user,
        _ => return accept(),
    };
    if user.opt_text("home_instance")?.is_none_or(|h| h.is_empty())
        || user.opt_flag("federation_home_orphaned")? == Some(true)
    {
        return accept();
    }
    let id = user.text("id")?;
    let Some(shown) = crate::federation::presence::project(
        cx.tx,
        &cx.app.hub,
        &cx.app.federation.projections,
        &id,
        status,
    )?
    else {
        return accept();
    };
    let activities = if status == "offline" {
        Some(Vec::new())
    } else if payload.get("activities").is_some() {
        let valid = crate::events::validate_activities(payload.get("activities"));
        if valid.is_none() {
            tracing::warn!(user = id, "занятия из presence_update вне пределов");
        }
        valid
    } else {
        None
    };
    if let Some(list) = &activities {
        cx.app.hub.set_activities(&id, list.clone());
    }
    let wire = crate::ws::presence_update(cx.tx, &id, &shown, activities.as_deref())?;
    for target in crate::ws::profile_targets(cx.tx, &id)? {
        cx.to_user(&target, &wire)?;
    }
    accept()
}

/// Позиция чтения (`processReadStateUpdateEvent`).
pub fn read_state(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let (Some(federated), Some(state)) = (
        super::non_empty(event, "federatedId"),
        present(event, "readState"),
    ) else {
        return reject("missing_read_state_payload");
    };
    let reader = state.get("user");
    if let Some(refusal) = cx.refusal(reader)? {
        tracing::warn!(refusal, source = cx.source, "read_state_update отклонён");
        return reject(refusal);
    }
    let Some(channel) = live_channel(cx.tx, federated)? else {
        return reject("channel_not_found");
    };
    let user = match cx.resolve(reader.unwrap_or(&Value::Null))? {
        Resolved::Found(user) => *user,
        Resolved::Mismatch => return reject("attribution_mismatch"),
        Resolved::Unknown => return reject("user_not_found"),
    };
    let reference = state.get("messageRef").unwrap_or(&Value::Null);
    let ref_source = text(reference, "sourceInstance").unwrap_or_default();
    let ref_id = text(reference, "sourceMessageId").unwrap_or_default();
    let ours = our_origin(&cx.app.config);
    let local_message = if raw_domain(ref_source) == raw_domain(&ours) {
        ref_id.to_owned()
    } else {
        let Some(row) = cx.tx.query_one(
            "SELECT id FROM dm_messages WHERE source_instance = ? AND source_message_id = ?",
            params![ref_source, ref_id],
        )?
        else {
            return accept();
        };
        row.text("id")?
    };
    let user_id = user.text("id")?;
    let channel_id = channel.text("id")?;
    let at = timestamp(event);
    let existing = cx.tx.query_one(
        "SELECT updated_at FROM read_states WHERE user_id = ? AND channel_id = ?",
        params![&user_id, &channel_id],
    )?;
    match existing {
        Some(row) => {
            if at > row.opt_int("updated_at")?.unwrap_or(0) {
                cx.tx.execute(
                    "UPDATE read_states SET last_read_message_id = ?, updated_at = ? WHERE user_id = ? AND channel_id = ?",
                    params![&local_message, at, &user_id, &channel_id],
                )?;
            }
        }
        None => {
            cx.tx.execute(
                "INSERT INTO read_states (user_id, channel_id, last_read_message_id, updated_at) VALUES (?, ?, ?, ?)",
                params![&user_id, &channel_id, &local_message, at],
            )?;
        }
    }
    cx.to_user(
        &user_id,
        &json!({"type": "channel_ack", "channelId": channel_id, "messageId": local_message}),
    )?;
    accept()
}

/// Переписку закрыли или открыли на другом инстансе (`processDmCloseEvent`,
/// `processDmReopenEvent`).
pub fn close(cx: &mut Ctx<'_>, event: &Value, closed: bool) -> Step {
    let federated = super::non_empty(event, "federatedId");
    let payload = present(event, "dmCloseReopen");
    let (Some(federated), Some(payload)) = (federated, payload) else {
        return reject(if closed {
            "missing_dm_close_payload"
        } else {
            "missing_dm_reopen_payload"
        });
    };
    if let Some(refusal) = cx.refusal(Some(payload))? {
        tracing::warn!(refusal, source = cx.source, "dm_close/dm_reopen отклонён");
        return reject(refusal);
    }
    let Some(channel) = live_channel(cx.tx, federated)? else {
        return accept();
    };
    let user = match cx.resolve(payload)? {
        Resolved::Found(user) => *user,
        Resolved::Mismatch => return reject("attribution_mismatch"),
        Resolved::Unknown => return accept(),
    };
    let user_id = user.text("id")?;
    let channel_id = channel.text("id")?;
    let at = timestamp(event);
    let Some(row) = cx.tx.query_one(
        "SELECT closed, closed_changed_at FROM dm_members WHERE dm_channel_id = ? AND user_id = ?",
        params![&channel_id, &user_id],
    )?
    else {
        return accept();
    };
    if at <= row.opt_int("closed_changed_at")?.unwrap_or(0) {
        return accept();
    }
    cx.tx.execute(
        "UPDATE dm_members SET closed = ?, closed_changed_at = ? WHERE dm_channel_id = ? AND user_id = ?",
        params![closed, at, &channel_id, &user_id],
    )?;
    let was_closed = row.opt_flag("closed")? == Some(true);
    if was_closed != closed {
        if closed {
            cx.to_user(
                &user_id,
                &json!({"type": "dm_channel_closed", "dmChannelId": channel_id}),
            )?;
        } else if let Some(wire) = crate::dm::channel_wire(cx.tx, &channel_id)? {
            cx.to_user(
                &user_id,
                &json!({"type": "dm_channel_created", "dmChannel": wire}),
            )?;
        }
    }
    accept()
}
