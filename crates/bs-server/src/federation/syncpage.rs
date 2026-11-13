//! Ответ на `/api/federation/sync`: события журнала мутаций, которые пир
//! вправе видеть (`routes/federation/handlers/syncPage.ts`).
//!
//! Отбор оригинал делает одним запросом SQLite с `json_extract`; здесь
//! дешёвая часть условия остаётся в SQL, а проверки по содержимому
//! выполняются в коде над строками в том же порядке, так что страница
//! та же на SQLite и PostgreSQL.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};
use url::Url;

use super::our_origin;
use crate::config::Config;
use crate::db::{DbResult, Row, Tx};
use crate::federation_auth::normalize_origin;
use crate::params;

/// Позиция в журнале (`SyncPosition`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    /// Время мутации.
    pub ts: i64,
    /// Идентификатор строки при равном времени.
    pub id: Option<String>,
}

/// Что спрашивает пир (`SyncRequestScope`).
#[derive(Debug, Clone)]
pub struct Scope {
    /// `dm`, `friend` или `profile`.
    pub context: String,
    /// Одна переписка по нашему идентификатору.
    pub dm_channel: Option<String>,
    /// Одна переписка по общему идентификатору.
    pub federated: Option<String>,
}

/// Узел из адреса или имени (`identityHost`).
#[must_use]
pub fn identity_host(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let text = if lower.starts_with("http://") || lower.starts_with("https://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    Url::parse(&text).ok()?.host_str().map(str::to_lowercase)
}

#[derive(Debug, Clone)]
struct LogRow {
    id: String,
    entity_id: String,
    context_id: String,
    context_type: Option<String>,
    mutation_type: String,
    mutated_at: i64,
    payload: Option<String>,
}

impl LogRow {
    fn of(row: &Row) -> DbResult<Self> {
        Ok(Self {
            id: row.text("id")?,
            entity_id: row.text("entity_id")?,
            context_id: row.text("context_id")?,
            context_type: row.opt_text("context_type")?,
            mutation_type: row.text("mutation_type")?,
            mutated_at: row.int("mutated_at")?,
            payload: row.opt_text("payload")?,
        })
    }

    fn json(&self) -> Option<Value> {
        serde_json::from_str(self.payload.as_deref()?).ok()
    }
}

/// Читает журнал по порядку `(mutated_at, id)` после позиции, пока не наберёт
/// `limit` строк, прошедших `keep` (`readLogPage`).
fn read_page(
    tx: &mut dyn Tx,
    context: &str,
    channel: Option<&str>,
    after: &Position,
    limit: usize,
    keep: &mut dyn FnMut(&mut dyn Tx, &LogRow) -> DbResult<bool>,
) -> DbResult<Vec<LogRow>> {
    const BATCH: i64 = 500;
    let mut found = Vec::new();
    let mut position = after.clone();
    loop {
        let mut sql =
            "SELECT id, entity_id, context_id, context_type, mutation_type, mutated_at, payload \
                       FROM federation_mutation_log WHERE context_type = ?"
                .to_owned();
        let mut values: Vec<crate::db::Value> = vec![context.into()];
        if let Some(channel) = channel {
            sql.push_str(" AND context_id = ?");
            values.push(channel.into());
        }
        match &position.id {
            None => {
                sql.push_str(" AND mutated_at > ?");
                values.push(position.ts.into());
            }
            Some(id) => {
                sql.push_str(" AND (mutated_at > ? OR (mutated_at = ? AND id > ?))");
                values.push(position.ts.into());
                values.push(position.ts.into());
                values.push(id.clone().into());
            }
        }
        sql.push_str(" ORDER BY mutated_at ASC, id ASC LIMIT ?");
        values.push(BATCH.into());
        let rows = tx.query(&sql, &values)?;
        let exhausted = rows.len() < usize::try_from(BATCH).unwrap_or(usize::MAX);
        for row in &rows {
            let row = LogRow::of(row)?;
            position = Position {
                ts: row.mutated_at,
                id: Some(row.id.clone()),
            };
            if keep(tx, &row)? {
                found.push(row);
                if found.len() >= limit {
                    return Ok(found);
                }
            }
        }
        if exhausted {
            return Ok(found);
        }
    }
}

fn shared_channels(tx: &mut dyn Tx, peer_host: Option<&str>) -> DbResult<HashMap<String, String>> {
    let mut shared = HashMap::new();
    let Some(peer_host) = peer_host else {
        return Ok(shared);
    };
    for row in tx.query(
        "SELECT c.id AS channel_id, c.federated_id, u.home_instance FROM dm_channels c \
         JOIN dm_members m ON m.dm_channel_id = c.id JOIN users u ON u.id = m.user_id \
         WHERE c.federated_id IS NOT NULL AND c.deleted_at IS NULL AND u.is_deleted = 0 \
         AND u.federation_home_orphaned = 0 AND u.home_instance IS NOT NULL",
        params![],
    )? {
        if identity_host(row.opt_text("home_instance")?.as_deref()).as_deref() == Some(peer_host) {
            shared.insert(row.text("channel_id")?, row.text("federated_id")?);
        }
    }
    Ok(shared)
}

fn requester_home_ids(tx: &mut dyn Tx, peer_host: Option<&str>) -> DbResult<HashSet<String>> {
    let mut ids = HashSet::new();
    let Some(peer_host) = peer_host else {
        return Ok(ids);
    };
    for row in tx.query(
        "SELECT home_user_id, home_instance FROM users WHERE home_user_id IS NOT NULL \
         AND home_instance IS NOT NULL AND is_deleted = 0 AND federation_home_orphaned = 0",
        params![],
    )? {
        if identity_host(row.opt_text("home_instance")?.as_deref()).as_deref() == Some(peer_host) {
            ids.insert(row.text("home_user_id")?);
        }
    }
    Ok(ids)
}

/// Страница: прочитанные строки и те, что отдаются пиру (`SyncPage`).
struct Page {
    read: Vec<LogRow>,
    served: Vec<LogRow>,
}

fn dm_page(
    tx: &mut dyn Tx,
    peer_origin: &str,
    scope: &Scope,
    shared: &HashMap<String, String>,
    after: &Position,
    limit: usize,
) -> DbResult<Page> {
    let peer_host = identity_host(Some(peer_origin));
    let own = requester_home_ids(tx, peer_host.as_deref())?;
    let mut channel = scope.dm_channel.clone();
    if channel.is_none()
        && let Some(federated) = &scope.federated
    {
        match tx.query_one(
            "SELECT id FROM dm_channels WHERE federated_id = ? AND deleted_at IS NULL",
            params![federated],
        )? {
            Some(row) => channel = Some(row.text("id")?),
            None => {
                return Ok(Page {
                    read: Vec::new(),
                    served: Vec::new(),
                });
            }
        }
    }
    if shared.is_empty() && own.is_empty() {
        return Ok(Page {
            read: Vec::new(),
            served: Vec::new(),
        });
    }
    let mut keep = |tx: &mut dyn Tx, row: &LogRow| -> DbResult<bool> {
        let kind = row.mutation_type.as_str();
        let payload = row.json();
        let in_scope = shared.contains_key(&row.context_id)
            || (matches!(kind, "member_add" | "member_remove")
                && payload
                    .as_ref()
                    .and_then(|p| p.pointer("/membership/user/homeUserId"))
                    .and_then(Value::as_str)
                    .is_some_and(|id| own.contains(id))
                && tx
                    .query_one(
                        "SELECT id FROM dm_channels WHERE id = ? AND federated_id IS NOT NULL AND deleted_at IS NULL",
                        params![&row.context_id],
                    )?
                    .is_some());
        if !in_scope {
            return Ok(false);
        }
        if matches!(
            kind,
            "create" | "update" | "reaction_add" | "reaction_remove"
        ) && tx
            .query_one(
                "SELECT id FROM dm_messages WHERE id = ?",
                params![&row.entity_id],
            )?
            .is_none()
        {
            return Ok(false);
        }
        if matches!(kind, "reaction_add" | "reaction_remove") {
            let Some(payload) = payload else {
                return Ok(false);
            };
            let user = payload.get("userId").and_then(Value::as_str);
            let emoji = payload.get("emoji").and_then(Value::as_str);
            let held = match (user, emoji) {
                (Some(user), Some(emoji)) => tx
                    .query_one(
                        "SELECT id FROM dm_reactions WHERE dm_message_id = ? AND user_id = ? AND emoji = ?",
                        params![&row.entity_id, user, emoji],
                    )?
                    .is_some(),
                _ => false,
            };
            return Ok(held == (kind == "reaction_add"));
        }
        Ok(true)
    };
    let read = read_page(tx, "dm", channel.as_deref(), after, limit, &mut keep)?;
    let served = read
        .iter()
        .filter(|row| {
            shared.contains_key(&row.context_id)
                || (matches!(row.mutation_type.as_str(), "member_add" | "member_remove")
                    && identity_host(
                        row.json()
                            .as_ref()
                            .and_then(|p| p.pointer("/membership/user/homeInstance"))
                            .and_then(Value::as_str),
                    )
                    .is_some_and(|host| Some(&host) == peer_host.as_ref()))
        })
        .cloned()
        .collect();
    Ok(Page { read, served })
}

fn friend_page(
    tx: &mut dyn Tx,
    peer_origin: &str,
    after: &Position,
    limit: usize,
) -> DbResult<Page> {
    let peer_host = identity_host(Some(peer_origin));
    let read = read_page(tx, "friend", None, after, limit, &mut |_, _| Ok(true))?;
    let mut served = Vec::new();
    for row in &read {
        let Some(friendship) = row.json().and_then(|p| p.get("friendship").cloned()) else {
            continue;
        };
        let mut qualifies = false;
        for side in [friendship.get("from"), friendship.get("to")] {
            if side_qualifies(tx, peer_host.as_deref(), side)? {
                qualifies = true;
                break;
            }
        }
        if qualifies {
            served.push(row.clone());
        }
    }
    Ok(Page { read, served })
}

fn side_qualifies(
    tx: &mut dyn Tx,
    peer_host: Option<&str>,
    side: Option<&Value>,
) -> DbResult<bool> {
    let (Some(peer_host), Some(side)) = (peer_host, side) else {
        return Ok(false);
    };
    let (Some(id), Some(home)) = (
        side.get("homeUserId")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty()),
        side.get("homeInstance")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty()),
    ) else {
        return Ok(false);
    };
    if identity_host(Some(home)).as_deref() != Some(peer_host) {
        return Ok(false);
    }
    let mut rows = tx.query(
        "SELECT home_instance, is_deleted, federation_home_orphaned FROM users WHERE home_user_id = ?",
        params![id],
    )?;
    for row in tx.query(
        "SELECT detached_home_instance AS home_instance, is_deleted, 1 AS federation_home_orphaned \
         FROM users WHERE detached_home_user_id = ?",
        params![id],
    )? {
        rows.push(row);
    }
    for row in rows {
        if identity_host(row.opt_text("home_instance")?.as_deref()).as_deref() == Some(peer_host) {
            return Ok(row.opt_flag("is_deleted")? != Some(true)
                && row.opt_flag("federation_home_orphaned")? != Some(true));
        }
    }
    Ok(true)
}

fn profile_page(tx: &mut dyn Tx, after: &Position, limit: usize) -> DbResult<Page> {
    let read = read_page(tx, "profile", None, after, limit, &mut |_, _| Ok(true))?;
    Ok(Page {
        served: read.clone(),
        read,
    })
}

const EVENT_PAYLOAD_TYPES: [&str; 8] = [
    "member_add",
    "member_remove",
    "ownership_transfer",
    "friend_request_create",
    "friend_request_update",
    "friend_request_cancel",
    "friend_add",
    "friend_remove",
];

fn base(row: &LogRow, kind: &str, channel: bool) -> Map<String, Value> {
    let mut event = Map::new();
    event.insert("eventType".into(), json!(kind));
    if channel {
        event.insert("dmChannelId".into(), json!(row.context_id));
    }
    event.insert("messageId".into(), json!(row.entity_id));
    event.insert("encryptionVersion".into(), json!(0));
    event.insert("timestamp".into(), json!(row.mutated_at));
    event
}

fn truthy_object(value: Option<Value>) -> Option<Value> {
    value.filter(|v| !v.is_null() && v != &Value::Bool(false))
}

/// Событие из строки журнала (`serializeSyncRow`).
fn serialize(
    tx: &mut dyn Tx,
    config: &Config,
    row: &LogRow,
    peer_origin: &str,
    shared: &HashMap<String, String>,
) -> DbResult<Option<Value>> {
    let kind = row.mutation_type.as_str();
    if EVENT_PAYLOAD_TYPES.contains(&kind) {
        let dm = row.context_type.as_deref().is_none_or(|c| c == "dm");
        let mut event = Map::new();
        event.insert("eventType".into(), json!(kind));
        event.insert(
            "contextType".into(),
            json!(row.context_type.as_deref().unwrap_or("dm")),
        );
        if dm {
            event.insert("dmChannelId".into(), json!(row.context_id));
        }
        event.insert("messageId".into(), json!(row.entity_id));
        event.insert("encryptionVersion".into(), json!(0));
        event.insert("timestamp".into(), json!(row.mutated_at));
        if let Some(Value::Object(payload)) = row.json() {
            for (key, value) in payload {
                event.insert(key, value);
            }
        }
        return Ok(Some(Value::Object(event)));
    }
    let channel_federated = || shared.get(&row.context_id).cloned();
    let event = match kind {
        "delete" => {
            let mut event = base(row, "delete", true);
            if let Some(target) = truthy_object(row.json().and_then(|p| p.get("target").cloned())) {
                event.insert("target".into(), target);
            }
            Some(event)
        }
        "reaction_add" | "reaction_remove" => {
            let Some(Value::Object(reaction)) = row.json() else {
                return Ok(None);
            };
            if tx
                .query_one(
                    "SELECT id FROM dm_messages WHERE id = ?",
                    params![&row.entity_id],
                )?
                .is_none()
            {
                return Ok(None);
            }
            let target = super::message_ref(tx, config, &row.entity_id)?;
            let mut wire = Map::new();
            wire.insert("messageId".into(), target["messageId"].clone());
            wire.insert(
                "messageHomeInstance".into(),
                target["messageHomeInstance"].clone(),
            );
            for key in ["userId", "homeUserId"] {
                if let Some(value) = reaction.get(key) {
                    wire.insert(key.into(), value.clone());
                }
            }
            let home = reaction
                .get("homeInstance")
                .filter(|v| super::inbound::truthy(v))
                .cloned()
                .unwrap_or_else(|| json!(our_origin(config)));
            wire.insert("homeInstance".into(), home);
            if let Some(value) = reaction.get("emoji") {
                wire.insert("emoji".into(), value.clone());
            }
            let created = reaction
                .get("createdAt")
                .filter(|v| !v.is_null())
                .cloned()
                .unwrap_or_else(|| json!(row.mutated_at));
            wire.insert("createdAt".into(), created);
            let mut event = base(row, kind, true);
            event.insert("reaction".into(), Value::Object(wire));
            Some(event)
        }
        "dm_close" | "dm_reopen" => {
            let (Some(payload), Some(federated)) = (truthy_object(row.json()), channel_federated())
            else {
                return Ok(None);
            };
            let mut event = base(row, kind, true);
            event.insert("federatedId".into(), json!(federated));
            event.insert("dmCloseReopen".into(), payload);
            Some(event)
        }
        "read_state_update" | "group_metadata_update" => {
            let (Some(payload), Some(federated)) = (truthy_object(row.json()), channel_federated())
            else {
                return Ok(None);
            };
            let mut event = base(row, kind, true);
            event.insert("federatedId".into(), json!(federated));
            let key = if kind == "read_state_update" {
                "readState"
            } else {
                "metadata"
            };
            event.insert(key.into(), payload);
            Some(event)
        }
        "file_rejected" => {
            let Some(Value::Object(rejection)) = row.json() else {
                return Ok(None);
            };
            let copy = tx.query_one(
                "SELECT source_instance FROM dm_messages WHERE source_message_id = ? AND dm_channel_id = ?",
                params![&row.entity_id, &row.context_id],
            )?;
            let source = copy
                .map(|copy| copy.opt_text("source_instance"))
                .transpose()?
                .flatten()
                .filter(|s| !s.is_empty());
            if source.is_none()
                || normalize_origin(source.as_deref()) != normalize_origin(Some(peer_origin))
            {
                return Ok(None);
            }
            let mut event = base(row, kind, true);
            for key in [
                "attachmentId",
                "sourceFilename",
                "rejectionReason",
                "rejectionLimit",
                "affectedUserIds",
            ] {
                if let Some(value) = rejection.get(key) {
                    event.insert(key.into(), value.clone());
                }
            }
            if let Some(users) = rejection
                .get("affectedUsers")
                .filter(|v| super::inbound::truthy(v))
            {
                event.insert("affectedUsers".into(), users.clone());
            }
            Some(event)
        }
        "profile_update" => {
            let Some(update) =
                truthy_object(row.json().and_then(|p| p.get("profileUpdate").cloned()))
            else {
                return Ok(None);
            };
            let mut event = Map::new();
            event.insert("eventType".into(), json!("profile_update"));
            event.insert("contextType".into(), json!("profile"));
            event.insert("messageId".into(), json!(row.entity_id));
            event.insert("encryptionVersion".into(), json!(0));
            event.insert("timestamp".into(), json!(row.mutated_at));
            event.insert("profileUpdate".into(), update);
            Some(event)
        }
        "create" | "update" => {
            return super::relay::sync_message(
                tx,
                config,
                &row.context_id,
                &row.entity_id,
                kind == "update",
                row.mutated_at,
            );
        }
        _ => None,
    };
    Ok(event.map(Value::Object))
}

/// Ответ на запрос пира (`buildSyncResponse`).
///
/// # Errors
/// Ошибка базы.
pub fn build(
    tx: &mut dyn Tx,
    config: &Config,
    peer_origin: &str,
    scope: &Scope,
    after: &Position,
    limit: usize,
) -> DbResult<Value> {
    let peer_host = identity_host(Some(peer_origin));
    let shared = if scope.context == "dm" {
        shared_channels(tx, peer_host.as_deref())?
    } else {
        HashMap::new()
    };
    let mut events = Vec::new();
    let mut position = after.clone();
    let mut last: Option<LogRow> = None;
    loop {
        let page = match scope.context.as_str() {
            "friend" => friend_page(tx, peer_origin, &position, limit)?,
            "profile" => profile_page(tx, &position, limit)?,
            _ => dm_page(tx, peer_origin, scope, &shared, &position, limit)?,
        };
        for row in &page.served {
            if let Some(event) = serialize(tx, config, row, peer_origin, &shared)? {
                events.push(event);
            }
        }
        if let Some(page_last) = page.read.last() {
            position = Position {
                ts: page_last.mutated_at,
                id: Some(page_last.id.clone()),
            };
            last = Some(page_last.clone());
        }
        let full = page.read.len() >= limit;
        if !events.is_empty() || !full {
            let mut response = json!({
                "events": events,
                "hasMore": full,
                "checkpoint": last.as_ref().map_or(after.ts, |row| row.mutated_at),
            });
            if let Some(row) = &last {
                response["checkpointId"] = json!(row.id);
            }
            return Ok(response);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        assert_eq!(identity_host(Some(" Y.test ")).as_deref(), Some("y.test"));
        assert_eq!(
            identity_host(Some("https://y.test:8443")).as_deref(),
            Some("y.test")
        );
        assert_eq!(identity_host(Some("")), None);
    }
}
