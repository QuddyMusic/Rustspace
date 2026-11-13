//! Участники и свойства групп (`routes/federation/events/membership.ts`).

use std::collections::HashMap;

use serde_json::{Value, json};

use super::messages::{Member, may_relay_into, member_with_identity, members};
use super::{
    Ctx, Step, accept, actor, any_channel, hints_of, non_empty, present, raw_domain, reject, text,
    timestamp, user_row,
};
use crate::db::{DbResult, Row};
use crate::dm::system_content;
use crate::federation::identity::Resolved;
use crate::federation::our_origin;
use crate::federation::relay::{claim_subject, member_subject, record_subject, subject_stale};
use crate::federation_auth::{canonical_home_instance, normalize_origin};
use crate::messages::system_name;
use crate::params;
use crate::routes::users::sanitize_user;
use crate::state::now_ms;
use crate::util::{js_trim, utf16_len};

const NAME_MIN: usize = 1;
const NAME_MAX: usize = 50;

fn already_applied(cx: &mut Ctx<'_>, id: &str) -> DbResult<bool> {
    Ok(cx
        .tx
        .query_one(
            "SELECT id FROM dm_messages WHERE source_instance = ? AND source_message_id = ?",
            params![cx.source, id],
        )?
        .is_some())
}

fn display_of(user: &Row) -> DbResult<String> {
    Ok(system_name(
        user.opt_text("display_name")?.as_deref(),
        &user.text("username")?,
    ))
}

#[allow(clippy::too_many_arguments, reason = "поля системного сообщения")]
fn system_message(
    cx: &mut Ctx<'_>,
    channel: &str,
    author: &str,
    content: &str,
    created_at: i64,
    source_message_id: &str,
    user: Option<&Row>,
) -> DbResult<Value> {
    let id = cx.app.ids.next();
    cx.tx.execute(
        "INSERT INTO dm_messages (id, dm_channel_id, user_id, content, type, created_at, source_instance, \
         source_message_id) VALUES (?, ?, ?, ?, 'system', ?, ?, ?)",
        params![&id, channel, author, content, created_at, cx.source, source_message_id],
    )?;
    let mut payload = json!({
        "id": id,
        "dmChannelId": channel,
        "userId": author,
        "content": content,
        "type": "system",
        "createdAt": created_at,
        "sourceInstance": cx.source,
        "sourceMessageId": source_message_id,
        "editedAt": null,
        "replyToId": null,
        "attachments": [],
        "embeds": [],
        "reactions": [],
    });
    if let Some(user) = user {
        payload["user"] = sanitize_user(user, false)?;
    }
    Ok(payload)
}

fn subject_of(cx: &Ctx<'_>, federated: &str, member: &Value) -> Option<String> {
    member_subject(&cx.app.config, Some(federated), member)
}

/// Участник добавлен в группу (`processMemberAddEvent`).
#[allow(clippy::too_many_lines, reason = "порядок проверок оригинала")]
pub fn add(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let id = text(event, "messageId").unwrap_or_default().to_owned();
    let federated = non_empty(event, "federatedId");
    let membership = present(event, "membership");
    let added = membership.and_then(|m| present(m, "user"));
    let (Some(federated), Some(membership), Some(added)) = (federated, membership, added) else {
        return reject("missing_membership_payload");
    };
    if already_applied(cx, &id)? {
        return accept();
    }
    let added_by = present(membership, "addedBy");
    if let Some(added_by) = added_by
        && let Some(refusal) = cx.refusal(Some(added_by))?
    {
        tracing::warn!(refusal, "member_add отклонён");
        return reject(refusal);
    }
    let at = timestamp(event);
    let subject = subject_of(cx, federated, added);
    if let Some(subject) = &subject
        && subject_stale(cx.tx, subject, at)?
    {
        return accept();
    }
    let mut channel = any_channel(cx.tx, federated)?;
    let mut bootstrapped = false;
    if channel.is_none()
        && let Some(group) = present(event, "group")
    {
        let Some(owner) = present(group, "owner") else {
            return reject("invalid_target");
        };
        if let Some(refusal) = cx.refusal(Some(owner))? {
            tracing::warn!(refusal, "member_add: владелец группы отклонён");
            return reject(refusal);
        }
        let owner_local = match actor(owner) {
            Some(who) => cx.replica(&who, &hints_of(owner))?,
            None => None,
        };
        let mut roster: Vec<Row> = Vec::new();
        let mut roster_subjects: HashMap<String, Option<String>> = HashMap::new();
        for member in group
            .get("members")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(who) = actor(member) else {
                continue;
            };
            let Some(replica) = cx.replica(&who, &hints_of(member))? else {
                continue;
            };
            let user_id = replica.user.text("id")?;
            if roster
                .iter()
                .any(|row| row.text("id").is_ok_and(|id| id == user_id))
            {
                continue;
            }
            roster_subjects.insert(user_id, subject_of(cx, federated, member));
            roster.push(replica.user);
        }
        let ours = our_origin(&cx.app.config);
        let roster_members: Vec<Member> = roster.iter().map(Member::of).collect::<DbResult<_>>()?;
        let owner_local = match owner_local {
            Some(owner)
                if may_relay_into(&roster_members, &ours, &owner.user.text("id")?, cx.source) =>
            {
                owner.user
            }
            _ => {
                tracing::warn!(federated, "member_add: группа без владельца в составе");
                return reject("invalid_target");
            }
        };
        let channel_id = cx.app.ids.next();
        let name = group.get("name").and_then(Value::as_str).map(str::to_owned);
        let icon_url = group.get("icon").and_then(Value::as_str).map(str::to_owned);
        let metadata_at = group
            .get("metadataUpdatedAt")
            .filter(|v| !v.is_null())
            .map_or(0, |v| super::millis(Some(v)));
        let icon = match &icon_url {
            Some(url) => {
                let source = cx.source.to_owned();
                match cx.asset(url, &source)? {
                    Some(file) => {
                        cx.keep(&file);
                        Some(file)
                    }
                    None => Some(url.clone()),
                }
            }
            None => None,
        };
        let owner_home = text(owner, "homeInstance").unwrap_or_default();
        cx.tx.execute(
            "INSERT INTO dm_channels (id, federated_id, owner_id, owner_home_user_id, owner_home_instance, \
             created_at, name, icon, metadata_updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                &channel_id,
                federated,
                owner_local.text("id")?,
                text(owner, "homeUserId"),
                canonical_home_instance(Some(owner_home)),
                now_ms(),
                name,
                icon,
                metadata_at
            ],
        )?;
        for user in &roster {
            let user_id = user.text("id")?;
            if let Some(Some(subject)) = roster_subjects.get(&user_id)
                && subject_stale(cx.tx, subject, at)?
            {
                continue;
            }
            crate::dm::insert_member(cx.tx, &channel_id, &user_id, false, now_ms())?;
        }
        channel = cx.tx.query_one(
            "SELECT * FROM dm_channels WHERE id = ?",
            params![&channel_id],
        )?;
        tracing::info!(
            channel = channel_id,
            federated,
            "группа создана по событию пира"
        );
        bootstrapped = true;
    }
    let Some(channel) = channel else {
        return reject("channel_not_found");
    };
    let channel_id = channel.text("id")?;
    let ours = our_origin(&cx.app.config);
    if !bootstrapped {
        if channel.opt_text("owner_id")?.is_none_or(|o| o.is_empty()) {
            return reject("invalid_target");
        }
        let list = members(cx.tx, &channel_id)?;
        let adder = added_by
            .and_then(actor)
            .and_then(|who| member_with_identity(&list, &ours, &who).cloned());
        if !adder.is_some_and(|adder| may_relay_into(&list, &ours, &adder.id, cx.source)) {
            tracing::warn!(channel = channel_id, "member_add не от участника отклонён");
            return reject("unauthorized_source");
        }
    }
    if channel.opt_int("deleted_at")?.is_some() {
        cx.tx.execute(
            "UPDATE dm_channels SET deleted_at = NULL WHERE id = ?",
            params![&channel_id],
        )?;
    }
    let Some(local) = (match actor(added) {
        Some(who) => cx.replica(&who, &hints_of(added))?,
        None => None,
    }) else {
        return reject("participant_not_found");
    };
    let local = local.user;
    let local_id = local.text("id")?;
    let count = cx
        .tx
        .query(
            "SELECT user_id FROM dm_members WHERE dm_channel_id = ?",
            params![&channel_id],
        )?
        .len();
    if count >= crate::dm::GROUP_MAX_MEMBERS {
        return reject("max_members_exceeded");
    }
    if let Some(subject) = &subject
        && !claim_subject(cx.tx, subject, at)?
    {
        return accept();
    }
    if !crate::dm::is_member(cx.tx, &channel_id, &local_id)? {
        crate::dm::insert_member(cx.tx, &channel_id, &local_id, false, now_ms())?;
    }
    let actor_user = match added_by.and_then(actor) {
        Some(who) => cx
            .replica(&who, &hints_of(added_by.unwrap_or(&Value::Null)))?
            .map(|replica| replica.user),
        None => None,
    };
    let actor_id = match &actor_user {
        Some(user) => user.text("id")?,
        None => local_id.clone(),
    };
    let content = system_content(&[
        ("event", json!("member_added")),
        ("targetUserId", json!(local_id)),
        ("targetDisplayName", json!(display_of(&local)?)),
    ]);
    let shown = actor_user.as_ref().unwrap_or(&local).clone();
    let payload = system_message(
        cx,
        &channel_id,
        &actor_id,
        &content,
        now_ms(),
        &id,
        Some(&shown),
    )?;
    if bootstrapped {
        if let Some(mut wire) = crate::dm::channel_wire(cx.tx, &channel_id)? {
            wire["lastMessage"] = payload;
            let event = json!({"type": "dm_channel_created", "dmChannel": wire});
            for member in wire["members"].as_array().into_iter().flatten() {
                let home = member["homeInstance"].as_str().filter(|h| !h.is_empty());
                let home = home.map_or_else(
                    || ours.clone(),
                    |home| {
                        if home.starts_with("http") {
                            home.to_owned()
                        } else {
                            format!("https://{home}")
                        }
                    },
                );
                if home != ours {
                    continue;
                }
                if let Some(member_id) = member["id"].as_str() {
                    cx.to_user(member_id, &event)?;
                }
            }
        }
    } else {
        cx.to_dm_members(
            &channel_id,
            &json!({"type": "dm_message_created", "message": payload}),
        )?;
        cx.to_dm_members(
            &channel_id,
            &json!({"type": "dm_member_added", "dmChannelId": channel_id, "user": sanitize_user(&local, false)?}),
        )?;
    }
    accept()
}

/// Участник покинул группу или исключён (`processMemberRemoveEvent`).
pub fn remove(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let id = text(event, "messageId").unwrap_or_default().to_owned();
    let federated = non_empty(event, "federatedId");
    let membership = present(event, "membership");
    let removed = membership.and_then(|m| present(m, "user"));
    let (Some(federated), Some(membership), Some(removed)) = (federated, membership, removed)
    else {
        return reject("missing_membership_payload");
    };
    let leave = text(membership, "reason") == Some("leave");
    if leave && let Some(refusal) = cx.refusal(Some(removed))? {
        tracing::warn!(refusal, "member_remove отклонён");
        return reject(refusal);
    }
    if already_applied(cx, &id)? {
        return accept();
    }
    let channel = any_channel(cx.tx, federated)?;
    let at = timestamp(event);
    let subject = subject_of(cx, federated, removed);
    let Some(channel) = channel else {
        if leave && let Some(subject) = &subject {
            record_subject(cx.tx, subject, at)?;
        }
        return accept();
    };
    if !leave && channel.opt_text("owner_id")?.is_none_or(|o| o.is_empty()) {
        return reject("invalid_target");
    }
    if !leave
        && let Some(owner_home) = channel
            .opt_text("owner_home_instance")?
            .filter(|h| !h.is_empty())
        && normalize_origin(Some(cx.source)) != normalize_origin(Some(&owner_home))
    {
        return reject("unauthorized_source");
    }
    let resolved = cx.resolve(removed)?;
    if leave && matches!(resolved, Resolved::Mismatch) {
        return reject("attribution_mismatch");
    }
    if let Some(subject) = &subject
        && !claim_subject(cx.tx, subject, at)?
    {
        return accept();
    }
    let Resolved::Found(user) = resolved else {
        return accept();
    };
    let user_id = user.text("id")?;
    let channel_id = channel.text("id")?;
    let reason = if text(membership, "reason") == Some("kick") {
        "kick"
    } else {
        "leave"
    };
    let content = system_content(&[
        ("event", json!("member_removed")),
        ("targetUserId", json!(user_id)),
        ("targetDisplayName", json!(display_of(&user)?)),
        ("reason", json!(reason)),
    ]);
    let payload = system_message(
        cx,
        &channel_id,
        &user_id,
        &content,
        now_ms(),
        &id,
        Some(&user),
    )?;
    cx.to_dm_members(
        &channel_id,
        &json!({"type": "dm_message_created", "message": payload}),
    )?;
    cx.tx.execute(
        "DELETE FROM dm_members WHERE dm_channel_id = ? AND user_id = ?",
        params![&channel_id, &user_id],
    )?;
    cx.tx.execute(
        "DELETE FROM read_states WHERE user_id = ? AND channel_id = ?",
        params![&user_id, &channel_id],
    )?;
    cx.to_dm_members(
        &channel_id,
        &json!({"type": "dm_member_removed", "dmChannelId": channel_id, "userId": user_id}),
    )?;
    let remaining = cx
        .tx
        .query(
            "SELECT user_id FROM dm_members WHERE dm_channel_id = ?",
            params![&channel_id],
        )?
        .len();
    if remaining == 0 {
        cx.tx.execute(
            "UPDATE dm_channels SET deleted_at = ? WHERE id = ?",
            params![now_ms(), &channel_id],
        )?;
    }
    accept()
}

/// Владелец группы сменился (`processOwnershipTransferEvent`).
pub fn ownership(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let id = text(event, "messageId").unwrap_or_default().to_owned();
    let (Some(federated), Some(ownership)) =
        (non_empty(event, "federatedId"), present(event, "ownership"))
    else {
        return reject("missing_ownership_payload");
    };
    let previous = present(ownership, "previousOwner");
    if let Some(previous) = previous
        && let Some(refusal) = cx.refusal(Some(previous))?
    {
        return reject(refusal);
    }
    if already_applied(cx, &id)? {
        return accept();
    }
    let Some(channel) = any_channel(cx.tx, federated)? else {
        return accept();
    };
    let Some(owner_id) = channel.opt_text("owner_id")?.filter(|o| !o.is_empty()) else {
        return reject("invalid_target");
    };
    if let Some(owner_home) = channel
        .opt_text("owner_home_instance")?
        .filter(|h| !h.is_empty())
        && normalize_origin(Some(cx.source)) != normalize_origin(Some(&owner_home))
    {
        return reject("unauthorized_source");
    }
    let previous_local = match previous {
        Some(previous) => match cx.resolve(previous)? {
            Resolved::Mismatch => return reject("attribution_mismatch"),
            Resolved::Found(user) => Some(*user),
            Resolved::Unknown => None,
        },
        None => None,
    };
    let new_owner = ownership.get("newOwner").unwrap_or(&Value::Null);
    let Some(new_local) = (match actor(new_owner) {
        Some(who) => cx.replica(&who, &hints_of(new_owner))?,
        None => None,
    }) else {
        return reject("participant_not_found");
    };
    let new_local = new_local.user;
    let new_home = text(new_owner, "homeInstance").unwrap_or_default();
    let canonical = canonical_home_instance(Some(new_home)).unwrap_or_else(|| new_home.to_owned());
    let channel_id = channel.text("id")?;
    let new_id = new_local.text("id")?;
    cx.tx.execute(
        "UPDATE dm_channels SET owner_id = ?, owner_home_user_id = ?, owner_home_instance = ? WHERE id = ?",
        params![&new_id, text(new_owner, "homeUserId"), &canonical, &channel_id],
    )?;
    cx.to_dm_members(
        &channel_id,
        &json!({
            "type": "dm_owner_updated",
            "dmChannelId": channel_id,
            "newOwnerId": new_id,
            "newOwnerHomeUserId": new_owner.get("homeUserId"),
            "newOwnerHomeInstance": canonical,
        }),
    )?;
    let previous_id = match &previous_local {
        Some(user) => user.text("id")?,
        None => owner_id,
    };
    let content = system_content(&[
        ("event", json!("owner_changed")),
        ("newOwnerId", json!(new_id)),
        ("newOwnerDisplayName", json!(display_of(&new_local)?)),
    ]);
    let payload = system_message(
        cx,
        &channel_id,
        &previous_id,
        &content,
        now_ms(),
        &id,
        previous_local.as_ref(),
    )?;
    cx.to_dm_members(
        &channel_id,
        &json!({"type": "dm_message_created", "message": payload}),
    )?;
    accept()
}

/// Название или значок группы изменились (`processGroupMetadataUpdateEvent`).
#[allow(clippy::too_many_lines, reason = "порядок проверок оригинала")]
pub fn metadata(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let id = text(event, "messageId").unwrap_or_default().to_owned();
    let Some(federated) = non_empty(event, "federatedId") else {
        return reject("missing_federated_id");
    };
    let Some(channel) = any_channel(cx.tx, federated)? else {
        return accept();
    };
    let owner_home = channel.opt_text("owner_home_instance")?.unwrap_or_default();
    if raw_domain(cx.source) != raw_domain(&owner_home) {
        return reject("attribution_mismatch");
    }
    let Some(metadata) = present(event, "metadata") else {
        return reject("missing_metadata_payload");
    };
    let name = metadata.get("name").and_then(Value::as_str);
    if let Some(name) = name {
        let length = utf16_len(js_trim(name));
        if !(NAME_MIN..=NAME_MAX).contains(&length) {
            return reject("invalid_payload");
        }
    }
    let icon = metadata.get("icon").and_then(Value::as_str);
    if let Some(icon) = icon
        && !(icon.starts_with("http://") || icon.starts_with("https://"))
    {
        return reject("invalid_payload");
    }
    let updated_at = super::millis(metadata.get("metadataUpdatedAt"));
    if updated_at <= channel.opt_int("metadata_updated_at")?.unwrap_or(0) {
        return accept();
    }
    let old_name = channel.opt_text("name")?;
    let old_icon = channel.opt_text("icon")?;
    let name_changed = name != old_name.as_deref();
    let icon_changed = icon != old_icon.as_deref();
    if !name_changed && !icon_changed {
        return accept();
    }
    let name_message = format!("{id}:name");
    let icon_message = format!("{id}:icon");
    let name_row = name_changed && already_applied(cx, &name_message)?;
    let icon_row = icon_changed && already_applied(cx, &icon_message)?;
    if (!name_changed || name_row) && (!icon_changed || icon_row) {
        return accept();
    }
    let mut resolved_icon = icon.map(str::to_owned);
    if icon_changed && let Some(url) = icon {
        let source = cx.source.to_owned();
        if let Some(file) = cx.asset(url, &source)? {
            cx.keep(&file);
            resolved_icon = Some(file);
        }
    }
    let actor_participant = present(metadata, "actor");
    let mut actor_id = match actor_participant.and_then(|p| actor(p).map(|who| (p, who))) {
        Some((participant, who)) => cx
            .replica(&who, &hints_of(participant))?
            .map(|replica| replica.user.text("id"))
            .transpose()?,
        None => None,
    };
    if actor_id.is_none() {
        actor_id = channel.opt_text("owner_id")?.filter(|o| !o.is_empty());
    }
    let Some(actor_id) = actor_id else {
        return reject("actor_not_found");
    };
    let channel_id = channel.text("id")?;
    cx.tx.execute(
        "UPDATE dm_channels SET name = ?, icon = ?, metadata_updated_at = ? WHERE id = ?",
        params![name, &resolved_icon, updated_at, &channel_id],
    )?;
    let mut rows = Vec::new();
    if name_changed && !name_row {
        let content = system_content(&[
            ("event", json!("name_changed")),
            ("oldName", json!(old_name)),
            ("newName", json!(name)),
        ]);
        rows.push((content, name_message));
    }
    if icon_changed && !icon_row {
        rows.push((
            system_content(&[("event", json!("icon_changed"))]),
            icon_message,
        ));
    }
    let actor_row = user_row(cx.tx, &actor_id)?;
    let mut payloads = Vec::new();
    for (content, source_id) in &rows {
        payloads.push(system_message(
            cx,
            &channel_id,
            &actor_id,
            content,
            updated_at,
            source_id,
            actor_row.as_ref(),
        )?);
    }
    cx.to_dm_members(
        &channel_id,
        &json!({"type": "dm_channel_updated", "dmChannelId": channel_id, "name": name, "icon": resolved_icon}),
    )?;
    for payload in payloads {
        cx.to_dm_members(
            &channel_id,
            &json!({"type": "dm_message_created", "message": payload}),
        )?;
    }
    if icon_changed
        && let Some(old) = old_icon
        && !old.starts_with("http://")
        && !old.starts_with("https://")
        && Some(&old) != resolved_icon.as_ref()
    {
        cx.discard(&old);
    }
    accept()
}
