//! Рассылки с учётом прав: кому из подключённых положено событие.
//!
//! Событие, меняющее счётчик непрочитанных получателя, сопровождается его
//! новым `channel_unread_count` (`unreadCountFollowUp`).

use bs_permissions::Permissions;
use serde_json::{Value, json};

use crate::db::{DbResult, Tx};
use crate::hub::{Hub, Outbox};
use crate::params;
use crate::perms;
use crate::ready::{count_channel_unread, count_dm_unread, read_cursor};

fn space_of_channel(tx: &mut dyn Tx, channel_id: &str) -> DbResult<Option<String>> {
    tx.query_one(
        "SELECT space_id FROM channels WHERE id = ?",
        params![channel_id],
    )?
    .map(|row| row.text("space_id"))
    .transpose()
}

/// Счётчик канала пространства: `None` — канала нет; без права читать — 0.
fn space_channel_count(
    tx: &mut dyn Tx,
    user_id: &str,
    channel_id: &str,
    known: Option<Permissions>,
) -> DbResult<Option<i64>> {
    let resolved = match known {
        Some(known) => known,
        None => {
            let Some(space) = space_of_channel(tx, channel_id)? else {
                return Ok(None);
            };
            perms::compute(tx, user_id, &space, Some(channel_id))?
        }
    };
    if !perms::can_count_unread(resolved) {
        return Ok(Some(0));
    }
    let cursor = read_cursor(tx, user_id, channel_id)?;
    count_channel_unread(tx, user_id, channel_id, &cursor).map(Some)
}

/// Счётчик переписки: только открытому участнику живой переписки.
fn dm_count(tx: &mut dyn Tx, user_id: &str, dm_channel_id: &str) -> DbResult<Option<i64>> {
    let open = tx
        .query_one(
            "SELECT 1 AS one FROM dm_members m JOIN dm_channels c ON c.id = m.dm_channel_id \
             WHERE m.dm_channel_id = ? AND m.user_id = ? AND m.closed = 0 AND c.deleted_at IS NULL",
            params![dm_channel_id, user_id],
        )?
        .is_some();
    if !open {
        return Ok(None);
    }
    let cursor = read_cursor(tx, user_id, dm_channel_id)?;
    count_dm_unread(tx, user_id, dm_channel_id, &cursor).map(Some)
}

/// Событие `channel_unread_count`, следующее за `event`, если оно меняет
/// непрочитанное пользователя.
fn follow_up(
    tx: &mut dyn Tx,
    user_id: &str,
    event: &Value,
    channel_perms: Option<Permissions>,
) -> DbResult<Option<Value>> {
    let text = |pointer: &str| event.pointer(pointer).and_then(Value::as_str);
    let (channel, count) = match event["type"].as_str().unwrap_or_default() {
        "message_created" => match text("/message/channelId") {
            Some(id) => (id, space_channel_count(tx, user_id, id, channel_perms)?),
            None => return Ok(None),
        },
        "message_deleted" => match text("/channelId") {
            Some(id) => (id, space_channel_count(tx, user_id, id, channel_perms)?),
            None => return Ok(None),
        },
        "channel_updated" => match text("/channel/id") {
            Some(id) => (id, space_channel_count(tx, user_id, id, channel_perms)?),
            None => return Ok(None),
        },
        "dm_message_created" => match text("/message/dmChannelId") {
            Some(id) => (id, dm_count(tx, user_id, id)?),
            None => return Ok(None),
        },
        "dm_message_deleted" => match text("/dmChannelId") {
            Some(id) => (id, dm_count(tx, user_id, id)?),
            None => return Ok(None),
        },
        "dm_channel_created" => match text("/dmChannel/id") {
            Some(id) => (id, dm_count(tx, user_id, id)?),
            None => return Ok(None),
        },
        "channel_ack" | "mark_unread" => match text("/channelId") {
            Some(id) => {
                let count = if space_of_channel(tx, id)?.is_some() {
                    space_channel_count(tx, user_id, id, None)?
                } else {
                    dm_count(tx, user_id, id)?
                };
                (id, count)
            }
            None => return Ok(None),
        },
        _ => return Ok(None),
    };
    Ok(count.map(|count| json!({"type": "channel_unread_count", "counts": {channel: count}})))
}

/// Всем соединениям пользователя, со счётчиком вслед, если он изменился.
///
/// # Errors
/// Ошибка базы.
pub fn to_user(tx: &mut dyn Tx, out: &mut Outbox, user_id: &str, event: &Value) -> DbResult<()> {
    deliver(tx, out, user_id, event, None)
}

/// То же, когда права пользователя в канале события уже посчитаны.
///
/// # Errors
/// Ошибка базы.
pub fn to_user_with_perms(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    user_id: &str,
    event: &Value,
    channel_perms: Permissions,
) -> DbResult<()> {
    deliver(tx, out, user_id, event, Some(channel_perms))
}

fn deliver(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    user_id: &str,
    event: &Value,
    channel_perms: Option<Permissions>,
) -> DbResult<()> {
    out.to_user(user_id, event);
    if let Some(count) = follow_up(tx, user_id, event, channel_perms)? {
        out.to_user(user_id, &count);
    }
    Ok(())
}

/// Подключённым участникам пространства, которые видят канал.
///
/// # Errors
/// Ошибка базы.
pub fn to_channel(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    space_id: &str,
    channel_id: &str,
    event: &Value,
    except: Option<&str>,
) -> DbResult<()> {
    for user in hub.online_in_space(space_id) {
        if except == Some(user.as_str()) {
            continue;
        }
        let channel_perms = perms::compute(tx, &user, space_id, Some(channel_id))?;
        if channel_perms.contains(Permissions::VIEW_CHANNEL) {
            deliver(tx, out, &user, event, Some(channel_perms))?;
        }
    }
    Ok(())
}

/// Участникам личной переписки.
///
/// # Errors
/// Ошибка базы.
pub fn to_dm_members(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    dm_channel_id: &str,
    event: &Value,
    except: Option<&str>,
) -> DbResult<()> {
    let members = tx.query(
        "SELECT user_id FROM dm_members WHERE dm_channel_id = ?",
        params![dm_channel_id],
    )?;
    for row in members {
        let user = row.text("user_id")?;
        if except != Some(user.as_str()) {
            to_user(tx, out, &user, event)?;
        }
    }
    Ok(())
}

/// Роли или права в пространстве изменились: каждый участник перечитывает
/// его (`space_access_changed`).
pub fn access_changed(out: &mut Outbox, space_id: &str) {
    out.to_space(
        space_id,
        &json!({"type": "space_access_changed", "spaceId": space_id}),
    );
}
