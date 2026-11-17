//! Каналы, категории и переопределения прав (`routes/channels.ts`).

use std::collections::HashSet;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use bs_permissions::{Permissions, can_manage_role_at, override_change_refusal, override_version};
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Row, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::JsonBody;
use crate::hub::{Hub, Outbox};
use crate::ready::{CHANNEL_COLUMNS, category_base, channel_base, channel_for_viewer, hidden_ids};
use crate::state::{AppState, now_ms};
use crate::util::{field, js_trim, str_field, truthy, utf16_len};
use crate::{broadcast, files, params, perms, ready, voice};

const NAME_MAX: usize = 100;
const TOPIC_MAX: usize = 1024;

/// Имя канала в том виде, в каком оно хранится (`normalizeChannelName`).
#[must_use]
pub fn normalize_channel_name(name: &str) -> String {
    let lowered = js_trim(name).to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut in_space = false;
    for c in lowered.chars() {
        if c.is_whitespace() || c == '\u{feff}' {
            if !in_space {
                out.push('-');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// Тема канала (`checkChannelTopic`): `Ok(None)` — без темы.
fn check_topic(raw: &Value) -> ApiResult<Option<String>> {
    match raw {
        Value::Null => Ok(None),
        Value::String(text) => {
            let unified = text.replace("\r\n", "\n").replace('\r', "\n");
            let topic = js_trim(&unified);
            if topic.is_empty() {
                return Ok(None);
            }
            if utf16_len(topic) > TOPIC_MAX {
                return Err(ApiError::bad_request("channel_topic_length").detail("max", TOPIC_MAX));
            }
            Ok(Some(topic.to_owned()))
        }
        _ => Err(ApiError::bad_request("channel_topic_invalid")),
    }
}

fn channel_row(tx: &mut dyn Tx, id: &str) -> DbResult<Option<Row>> {
    tx.query_one(
        &format!("SELECT {CHANNEL_COLUMNS} FROM channels WHERE id = ?"),
        params![id],
    )
}

fn category_row(tx: &mut dyn Tx, id: &str) -> DbResult<Option<Row>> {
    tx.query_one(
        "SELECT id, space_id, name, position, created_at FROM channel_categories WHERE id = ?",
        params![id],
    )
}

fn space_exists(tx: &mut dyn Tx, id: &str) -> ApiResult<()> {
    crate::routes::spaces::space_row(tx, id)?
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("space_not_found"))
}

fn position_of(value: &Value) -> ApiResult<f64> {
    value
        .as_f64()
        .filter(|position| *position >= 0.0)
        .ok_or_else(|| ApiError::bad_request("position_invalid"))
}

fn name_length_error(code: &'static str) -> ApiError {
    ApiError::bad_request(code)
        .detail("min", 1)
        .detail("max", NAME_MAX)
}

/// Каждому подключённому участнику — что стало с каналами: видимым
/// `channel_updated` с его правами, невидимым `channel_deleted`
/// (`broadcastChannelVisibility`).
fn broadcast_visibility(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    space_id: &str,
    channel_ids: &[String],
) -> DbResult<()> {
    if channel_ids.is_empty() {
        return Ok(());
    }
    let private = hidden_ids(tx, space_id, false)?;
    let mut channels = Vec::new();
    for id in channel_ids {
        if let Some(row) = channel_row(tx, id)?
            .filter(|row| row.text("space_id").ok().as_deref() == Some(space_id))
        {
            channels.push(row);
        }
    }
    for user in hub.online_in_space(space_id) {
        let mut sees_voice = false;
        for channel in &channels {
            let id = channel.text("id")?;
            let channel_perms = perms::compute(tx, &user, space_id, Some(&id))?;
            if channel_perms.contains(Permissions::VIEW_CHANNEL) {
                sees_voice |= channel.text("type")? == "voice";
                let event = json!({
                    "type": "channel_updated",
                    "channel": channel_for_viewer(channel, private.contains(&id), channel_perms)?,
                    "spaceId": space_id,
                });
                broadcast::to_user_with_perms(tx, out, &user, &event, channel_perms)?;
            } else {
                out.to_user(
                    &user,
                    &json!({"type": "channel_deleted", "channelId": id, "spaceId": space_id}),
                );
            }
        }
        if sees_voice {
            voice::push_space_state(tx, hub, out, &user, space_id)?;
        }
    }
    Ok(())
}

/// Раскладка каналов и категорий каждому подключённому участнику
/// (`broadcastChannelLayout`).
fn broadcast_layout(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    space_id: &str,
    moved: &[String],
) -> DbResult<()> {
    let channels = tx.query(
        &format!("SELECT {CHANNEL_COLUMNS} FROM channels WHERE space_id = ?"),
        params![space_id],
    )?;
    let private_channels = hidden_ids(tx, space_id, false)?;
    let private_categories = hidden_ids(tx, space_id, true)?;
    let mut categories = Vec::new();
    for row in tx.query(
        "SELECT id, space_id, name, position, created_at FROM channel_categories WHERE space_id = ?",
        params![space_id],
    )? {
        let mut category = category_base(&row)?;
        category["isPrivate"] = json!(private_categories.contains(&row.text("id")?));
        categories.push(category);
    }
    for user in hub.online_in_space(space_id) {
        let mut visible = Vec::new();
        let mut sees_moved_voice = false;
        for channel in &channels {
            let id = channel.text("id")?;
            let channel_perms = perms::compute(tx, &user, space_id, Some(&id))?;
            if channel_perms.contains(Permissions::VIEW_CHANNEL) {
                sees_moved_voice |= channel.text("type")? == "voice" && moved.contains(&id);
                visible.push(channel_for_viewer(
                    channel,
                    private_channels.contains(&id),
                    channel_perms,
                )?);
            }
        }
        out.to_user(
            &user,
            &json!({
                "type": "channel_layout_updated",
                "spaceId": space_id,
                "channels": visible,
                "categories": categories,
            }),
        );
        if sees_moved_voice {
            voice::push_space_state(tx, hub, out, &user, space_id)?;
        }
    }
    Ok(())
}

fn broadcast_category_override(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    space_id: &str,
    category_id: &str,
) -> DbResult<()> {
    let channels: Vec<String> = tx
        .query(
            "SELECT id FROM channels WHERE space_id = ? AND category_id = ?",
            params![space_id, category_id],
        )?
        .iter()
        .map(|row| row.text("id"))
        .collect::<DbResult<_>>()?;
    broadcast_visibility(tx, hub, out, space_id, &channels)?;
    if let Some(row) = category_row(tx, category_id)? {
        let mut category = category_base(&row)?;
        category["isPrivate"] = json!(hidden_ids(tx, space_id, true)?.contains(category_id));
        out.to_space(
            space_id,
            &json!({"type": "category_updated", "category": category, "spaceId": space_id}),
        );
    }
    Ok(())
}

/// `GET /api/spaces/:id/channels`.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            space_exists(tx, &id)?;
            if !perms::is_member(tx, &id, &user.id)? {
                return Err(ApiError::forbidden("not_space_member"));
            }
            let rows = tx.query(
                &format!(
                    "SELECT {CHANNEL_COLUMNS} FROM channels WHERE space_id = ? ORDER BY position"
                ),
                params![&id],
            )?;
            let mut channels = Vec::new();
            for row in rows {
                let channel_perms = perms::compute(tx, &user.id, &id, Some(&row.text("id")?))?;
                if perms::can_view(channel_perms) {
                    channels.push(channel_base(&row)?);
                }
            }
            Ok(Json(json!(channels)))
        })
        .await
}

/// `POST /api/spaces/:id/channels`.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let ids = state.ids.clone();
    let hub = state.hub.clone();
    state
        .run(move |tx, out| {
            space_exists(tx, &id)?;
            if !perms::has_permission(tx, &user.id, &id, Permissions::MANAGE_CHANNELS)? {
                return Err(perms::missing("MANAGE_CHANNELS"));
            }
            let name = str_field(&body, "name")
                .filter(|name| !name.is_empty())
                .ok_or_else(|| ApiError::bad_request("channel_name_required"))?;
            let name = normalize_channel_name(name);
            if name.is_empty() || utf16_len(&name) > NAME_MAX {
                return Err(name_length_error("channel_name_length"));
            }
            let kind = str_field(&body, "type")
                .filter(|kind| matches!(*kind, "text" | "voice"))
                .ok_or_else(|| ApiError::bad_request("channel_type_invalid"))?;
            let topic = match field(&body, "topic") {
                Some(raw) => check_topic(raw)?,
                None => None,
            };
            let mut category = None;
            if truthy(field(&body, "categoryId")) {
                let wanted = str_field(&body, "categoryId").unwrap_or_default();
                let found = category_row(tx, wanted)?
                    .filter(|row| row.text("space_id").ok().as_deref() == Some(id.as_str()));
                if found.is_none() {
                    return Err(ApiError::bad_request("category_not_in_space").detail("id", wanted));
                }
                category = Some(wanted.to_owned());
            }
            let max_position = tx
                .query_one("SELECT MAX(position) AS top FROM channels WHERE space_id = ?", params![&id])?
                .map(|row| row.opt_number("top"))
                .transpose()?
                .flatten()
                .and_then(|top| top.as_f64())
                .map_or(-1.0, |top| top.max(-1.0));
            let channel_id = ids.next();
            tx.execute(
                "INSERT INTO channels (id, space_id, name, type, topic, position, category_id, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                params![&channel_id, &id, &name, kind, &topic, max_position + 1.0, &category, now_ms()],
            )?;
            let row = channel_row(tx, &channel_id)?.ok_or_else(ApiError::internal)?;
            for member in hub.online_in_space(&id) {
                let channel_perms = perms::compute(tx, &member, &id, Some(&channel_id))?;
                if channel_perms.contains(Permissions::VIEW_CHANNEL) {
                    out.to_user(
                        &member,
                        &json!({
                            "type": "channel_created",
                            "channel": channel_for_viewer(&row, false, channel_perms)?,
                            "spaceId": id,
                        }),
                    );
                }
            }
            let mine = perms::compute(tx, &user.id, &id, Some(&channel_id))?;
            Ok((StatusCode::CREATED, Json(channel_for_viewer(&row, false, mine)?)))
        })
        .await
}

/// `PATCH /api/channels/:id`.
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let hub = state.hub.clone();
    state
        .run(move |tx, out| {
            let channel =
                channel_row(tx, &id)?.ok_or_else(|| ApiError::not_found("channel_not_found"))?;
            let space_id = channel.text("space_id")?;
            if !perms::has_channel_permission(
                tx,
                &user.id,
                &space_id,
                &id,
                Permissions::MANAGE_CHANNELS,
            )? {
                return Err(perms::missing("MANAGE_CHANNELS"));
            }
            let mut sets: Vec<(&str, crate::db::Value)> = Vec::new();
            if let Some(name) = field(&body, "name") {
                let name = name
                    .as_str()
                    .ok_or_else(|| ApiError::bad_request("channel_name_required"))?;
                let name = normalize_channel_name(name);
                if name.is_empty() || utf16_len(&name) > NAME_MAX {
                    return Err(name_length_error("channel_name_length"));
                }
                sets.push(("name", name.into()));
            }
            if let Some(topic) = field(&body, "topic") {
                sets.push(("topic", check_topic(topic)?.into()));
            }
            if let Some(position) = field(&body, "position") {
                sets.push(("position", position_of(position)?.into()));
            }
            let category = field(&body, "categoryId");
            if let Some(category) = category {
                if category.is_null() {
                    sets.push(("category_id", crate::db::Value::Null));
                } else {
                    let wanted = category.as_str().unwrap_or_default();
                    let found = category_row(tx, wanted)?.filter(|row| {
                        row.text("space_id").ok().as_deref() == Some(space_id.as_str())
                    });
                    if found.is_none() {
                        return Err(ApiError::bad_request("category_not_in_space")
                            .detail("id", category.clone()));
                    }
                    sets.push(("category_id", wanted.into()));
                }
            }
            if sets.is_empty() {
                return Err(ApiError::bad_request("no_fields_to_update"));
            }
            let assignments: Vec<String> = sets
                .iter()
                .map(|(column, _)| format!("{column} = ?"))
                .collect();
            let mut values: Vec<crate::db::Value> =
                sets.into_iter().map(|(_, value)| value).collect();
            values.push(id.as_str().into());
            tx.execute(
                &format!(
                    "UPDATE channels SET {} WHERE id = ?",
                    assignments.join(", ")
                ),
                &values,
            )?;
            let updated = channel_row(tx, &id)?.ok_or_else(ApiError::internal)?;
            let data = channel_base(&updated)?;
            if category.is_some() {
                broadcast_visibility(tx, &hub, out, &space_id, std::slice::from_ref(&id))?;
                if channel.text("type")? == "voice" {
                    voice::check_permissions(tx, &hub, out, &space_id)?;
                }
            } else {
                broadcast::to_channel(
                    tx,
                    &hub,
                    out,
                    &space_id,
                    &id,
                    &json!({"type": "channel_updated", "channel": data, "spaceId": space_id}),
                    None,
                )?;
            }
            Ok(Json(data))
        })
        .await
}

/// `DELETE /api/channels/:id`.
pub async fn delete(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let hub = state.hub.clone();
    let config = state.config.clone();
    let stale = state
        .run(move |tx, out| {
            let channel =
                channel_row(tx, &id)?.ok_or_else(|| ApiError::not_found("channel_not_found"))?;
            let space_id = channel.text("space_id")?;
            if !perms::has_channel_permission(
                tx,
                &user.id,
                &space_id,
                &id,
                Permissions::MANAGE_CHANNELS,
            )? {
                return Err(perms::missing("MANAGE_CHANNELS"));
            }
            voice::channel_deleted(tx, &hub, out, &space_id, &id)?;
            let mut viewers = Vec::new();
            for member in hub.online_in_space(&space_id) {
                if perms::compute(tx, &member, &space_id, Some(&id))?
                    .contains(Permissions::VIEW_CHANNEL)
                {
                    viewers.push(member);
                }
            }
            let stale: Vec<String> = tx
                .query(
                    "SELECT a.filename FROM attachments a JOIN messages m ON m.id = a.message_id \
                     WHERE m.channel_id = ?",
                    params![&id],
                )?
                .iter()
                .map(|row| row.text("filename"))
                .collect::<DbResult<_>>()?;
            tx.execute("DELETE FROM read_states WHERE channel_id = ?", params![&id])?;
            tx.execute("DELETE FROM messages WHERE channel_id = ?", params![&id])?;
            tx.execute("DELETE FROM channels WHERE id = ?", params![&id])?;
            let event = json!({"type": "channel_deleted", "channelId": id, "spaceId": space_id});
            for viewer in viewers {
                out.to_user(&viewer, &event);
            }
            Ok(stale)
        })
        .await?;
    for file in &stale {
        files::delete_upload_file(&config, file).await;
    }
    Ok(Json(json!({"success": true})))
}

/// Чьё переопределение: канала или категории.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Channel,
    Category,
}

impl Scope {
    const fn table(self) -> &'static str {
        match self {
            Self::Channel => "channel_overrides",
            Self::Category => "category_overrides",
        }
    }
    const fn column(self) -> &'static str {
        match self {
            Self::Channel => "channel_id",
            Self::Category => "category_id",
        }
    }
    const fn key(self) -> &'static str {
        match self {
            Self::Channel => "channelId",
            Self::Category => "categoryId",
        }
    }
    const fn missing(self) -> &'static str {
        match self {
            Self::Channel => "channel_not_found",
            Self::Category => "category_not_found",
        }
    }

    /// Пространство, которому принадлежит канал или категория.
    fn space_of(self, tx: &mut dyn Tx, id: &str) -> ApiResult<String> {
        let row = match self {
            Self::Channel => channel_row(tx, id)?,
            Self::Category => category_row(tx, id)?,
        };
        Ok(row
            .ok_or_else(|| ApiError::not_found(self.missing()))?
            .text("space_id")?)
    }
}

fn stored_override(
    tx: &mut dyn Tx,
    scope: Scope,
    id: &str,
    target_type: &str,
    target_id: &str,
) -> DbResult<Option<(Permissions, Permissions)>> {
    tx.query_one(
        &format!(
            "SELECT allow, deny FROM {} WHERE {} = ? AND target_type = ? AND target_id = ?",
            scope.table(),
            scope.column()
        ),
        params![id, target_type, target_id],
    )?
    .map(|row| {
        Ok((
            perms::from_db(row.opt_text("allow")?.as_deref()),
            perms::from_db(row.opt_text("deny")?.as_deref()),
        ))
    })
    .transpose()
}

fn require_permission_reader(
    tx: &mut dyn Tx,
    user_id: &str,
    space_id: &str,
) -> ApiResult<Permissions> {
    let held = perms::compute(tx, user_id, space_id, None)?;
    if !ready::reads_permission_data(held) {
        return Err(perms::missing("MANAGE_ROLES"));
    }
    Ok(held)
}

/// Можно ли действующему трогать переопределение этой цели (`overrideTargetRefusal`).
fn override_target_refusal(
    tx: &mut dyn Tx,
    actor: &str,
    space_id: &str,
    target_type: &str,
    target_id: &str,
    writing: bool,
) -> ApiResult<()> {
    if target_type == "role" {
        let role = tx.query_one(
            "SELECT position FROM roles WHERE id = ? AND space_id = ?",
            params![target_id, space_id],
        )?;
        let Some(role) = role else {
            if writing {
                return Err(ApiError::bad_request("role_not_in_space").detail("roleId", target_id));
            }
            return Ok(());
        };
        if !can_manage_role_at(
            perms::standing(tx, space_id, actor)?,
            role.opt_int("position")?.unwrap_or(0),
        ) {
            return Err(ApiError::forbidden("role_hierarchy"));
        }
    } else if target_type == "member"
        && target_id != actor
        && !perms::can_act_on(tx, space_id, actor, target_id)?
    {
        return Err(ApiError::forbidden("role_hierarchy"));
    }
    Ok(())
}

fn expected_version(value: Option<&Value>) -> ApiResult<Option<String>> {
    match value {
        None => Ok(None),
        Some(Value::String(text)) if !text.is_empty() => Ok(Some(text.clone())),
        Some(_) => Err(ApiError::bad_request("validation_failed")),
    }
}

fn override_bits(value: Option<&Value>) -> Option<Permissions> {
    match value {
        None => Some(Permissions::empty()),
        Some(value) => crate::routes::spaces::parse_bits(value).ok(),
    }
}

async fn list_overrides(
    state: AppState,
    user: AuthUser,
    scope: Scope,
    id: String,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let space_id = scope.space_of(tx, &id)?;
            if scope == Scope::Category && !perms::is_member(tx, &space_id, &user.id)? {
                return Err(ApiError::forbidden("not_space_member"));
            }
            require_permission_reader(tx, &user.id, &space_id)?;
            let rows = tx.query(
                &format!(
                    "SELECT target_type, target_id, allow, deny FROM {} WHERE {} = ?",
                    scope.table(),
                    scope.column()
                ),
                params![&id],
            )?;
            let mut list = Vec::with_capacity(rows.len());
            for row in rows {
                let allow = row.text("allow")?;
                let deny = row.text("deny")?;
                let version = override_version(Some((
                    perms::from_db(Some(&allow)),
                    perms::from_db(Some(&deny)),
                )));
                let mut item = json!({
                    "targetType": row.text("target_type")?,
                    "targetId": row.text("target_id")?,
                    "allow": allow,
                    "deny": deny,
                    "version": version,
                });
                item[scope.key()] = json!(id);
                list.push(item);
            }
            Ok(Json(json!(list)))
        })
        .await
}

async fn put_override(
    state: AppState,
    user: AuthUser,
    scope: Scope,
    id: String,
    body: Value,
) -> ApiResult<Json<Value>> {
    let hub = state.hub.clone();
    state
        .run(move |tx, out| {
            let target_type = str_field(&body, "targetType")
                .filter(|kind| matches!(*kind, "role" | "member"))
                .ok_or_else(|| ApiError::bad_request("override_target_invalid"))?;
            let target_id = str_field(&body, "targetId")
                .filter(|id| !id.is_empty())
                .ok_or_else(|| ApiError::bad_request("override_target_required"))?;
            let space_id = scope.space_of(tx, &id)?;
            let held = require_permission_reader(tx, &user.id, &space_id)?;
            let (Some(allow), Some(deny)) =
                (override_bits(field(&body, "allow")), override_bits(field(&body, "deny")))
            else {
                return Err(ApiError::bad_request("override_bits_invalid"));
            };
            let expected = expected_version(field(&body, "version"))?;
            override_target_refusal(tx, &user.id, &space_id, target_type, target_id, true)?;
            let existing = stored_override(tx, scope, &id, target_type, target_id)?;
            if expected.is_some_and(|expected| override_version(existing) != expected) {
                return Err(ApiError::conflict("overrides_conflict"));
            }
            if let Some(refusal) = override_change_refusal(held, existing, Some((allow, deny))) {
                return Err(ApiError::forbidden(refusal.code()));
            }
            tx.execute(
                &format!(
                    "DELETE FROM {} WHERE {} = ? AND target_type = ? AND target_id = ?",
                    scope.table(),
                    scope.column()
                ),
                params![&id, target_type, target_id],
            )?;
            tx.execute(
                &format!(
                    "INSERT INTO {} ({}, target_type, target_id, allow, deny) VALUES (?, ?, ?, ?, ?)",
                    scope.table(),
                    scope.column()
                ),
                params![&id, target_type, target_id, perms::to_wire(allow), perms::to_wire(deny)],
            )?;
            match scope {
                Scope::Channel => {
                    broadcast_visibility(tx, &hub, out, &space_id, std::slice::from_ref(&id))?;
                }
                Scope::Category => broadcast_category_override(tx, &hub, out, &space_id, &id)?,
            }
            voice::check_permissions(tx, &hub, out, &space_id)?;
            Ok(Json(json!({"success": true, "version": override_version(Some((allow, deny)))})))
        })
        .await
}

/// Параметр `version` при удалении переопределения.
#[derive(Debug, serde::Deserialize)]
pub struct VersionQuery {
    version: Option<String>,
}

async fn delete_override(
    state: AppState,
    user: AuthUser,
    scope: Scope,
    (id, target_type, target_id): (String, String, String),
    query: VersionQuery,
) -> ApiResult<Json<Value>> {
    let hub = state.hub.clone();
    state
        .run(move |tx, out| {
            let expected = match query.version {
                None => None,
                Some(version) if !version.is_empty() => Some(version),
                Some(_) => return Err(ApiError::bad_request("validation_failed")),
            };
            let space_id = scope.space_of(tx, &id)?;
            let held = require_permission_reader(tx, &user.id, &space_id)?;
            override_target_refusal(tx, &user.id, &space_id, &target_type, &target_id, false)?;
            let existing = stored_override(tx, scope, &id, &target_type, &target_id)?;
            if existing.is_some()
                && expected.is_some_and(|expected| override_version(existing) != expected)
            {
                return Err(ApiError::conflict("overrides_conflict"));
            }
            if let Some(refusal) = override_change_refusal(held, existing, None) {
                return Err(ApiError::forbidden(refusal.code()));
            }
            tx.execute(
                &format!(
                    "DELETE FROM {} WHERE {} = ? AND target_type = ? AND target_id = ?",
                    scope.table(),
                    scope.column()
                ),
                params![&id, &target_type, &target_id],
            )?;
            match scope {
                Scope::Channel => {
                    broadcast_visibility(tx, &hub, out, &space_id, std::slice::from_ref(&id))?;
                }
                Scope::Category => broadcast_category_override(tx, &hub, out, &space_id, &id)?,
            }
            voice::check_permissions(tx, &hub, out, &space_id)?;
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `GET /api/channels/:id/overrides`.
pub async fn channel_overrides(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    list_overrides(state, user, Scope::Channel, id).await
}

/// `PUT /api/channels/:id/overrides`.
pub async fn put_channel_override(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    put_override(state, user, Scope::Channel, id, body).await
}

/// `DELETE /api/channels/:id/overrides/:targetType/:targetId`.
pub async fn delete_channel_override(
    State(state): State<AppState>,
    user: AuthUser,
    Path(path): Path<(String, String, String)>,
    Query(query): Query<VersionQuery>,
) -> ApiResult<Json<Value>> {
    delete_override(state, user, Scope::Channel, path, query).await
}

/// `GET /api/categories/:id/overrides`.
pub async fn category_overrides(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    list_overrides(state, user, Scope::Category, id).await
}

/// `PUT /api/categories/:id/overrides`.
pub async fn put_category_override(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    put_override(state, user, Scope::Category, id, body).await
}

/// `DELETE /api/categories/:id/overrides/:targetType/:targetId`.
pub async fn delete_category_override(
    State(state): State<AppState>,
    user: AuthUser,
    Path(path): Path<(String, String, String)>,
    Query(query): Query<VersionQuery>,
) -> ApiResult<Json<Value>> {
    delete_override(state, user, Scope::Category, path, query).await
}

/// `POST /api/spaces/:id/categories`.
pub async fn create_category(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let ids = state.ids.clone();
    state
        .run(move |tx, out| {
            space_exists(tx, &id)?;
            if !perms::has_permission(tx, &user.id, &id, Permissions::MANAGE_CHANNELS)? {
                return Err(perms::missing("MANAGE_CHANNELS"));
            }
            let name = str_field(&body, "name")
                .map(js_trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| ApiError::bad_request("category_name_required"))?;
            if utf16_len(name) > NAME_MAX {
                return Err(name_length_error("category_name_length"));
            }
            let max_position = tx
                .query_one(
                    "SELECT MAX(position) AS top FROM channel_categories WHERE space_id = ?",
                    params![&id],
                )?
                .map(|row| row.opt_number("top"))
                .transpose()?
                .flatten()
                .and_then(|top| top.as_f64())
                .map_or(-1.0, |top| top.max(-1.0));
            let category_id = ids.next();
            tx.execute(
                "INSERT INTO channel_categories (id, space_id, name, position, created_at) VALUES (?, ?, ?, ?, ?)",
                params![&category_id, &id, name, max_position + 1.0, now_ms()],
            )?;
            let row = category_row(tx, &category_id)?.ok_or_else(ApiError::internal)?;
            let data = category_base(&row)?;
            out.to_space(&id, &json!({"type": "category_created", "category": data, "spaceId": id}));
            Ok((StatusCode::CREATED, Json(data)))
        })
        .await
}

/// `PATCH /api/categories/:id`.
pub async fn update_category(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, out| {
            let category =
                category_row(tx, &id)?.ok_or_else(|| ApiError::not_found("category_not_found"))?;
            let space_id = category.text("space_id")?;
            if !perms::has_permission(tx, &user.id, &space_id, Permissions::MANAGE_CHANNELS)? {
                return Err(perms::missing("MANAGE_CHANNELS"));
            }
            let mut changed = false;
            if let Some(name) = field(&body, "name") {
                let name = js_trim(
                    name.as_str()
                        .ok_or_else(|| ApiError::bad_request("category_name_required"))?,
                );
                if name.is_empty() || utf16_len(name) > NAME_MAX {
                    return Err(name_length_error("category_name_length"));
                }
                tx.execute(
                    "UPDATE channel_categories SET name = ? WHERE id = ?",
                    params![name, &id],
                )?;
                changed = true;
            }
            if let Some(position) = field(&body, "position") {
                let position = position_of(position)?;
                tx.execute(
                    "UPDATE channel_categories SET position = ? WHERE id = ?",
                    params![position, &id],
                )?;
                changed = true;
            }
            if !changed {
                return Err(ApiError::bad_request("no_fields_to_update"));
            }
            let updated = category_row(tx, &id)?.ok_or_else(ApiError::internal)?;
            let mut data = category_base(&updated)?;
            data["isPrivate"] = json!(hidden_ids(tx, &space_id, true)?.contains(&id));
            out.to_space(
                &space_id,
                &json!({"type": "category_updated", "category": data, "spaceId": space_id}),
            );
            Ok(Json(data))
        })
        .await
}

/// `DELETE /api/categories/:id`.
pub async fn delete_category(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let hub = state.hub.clone();
    state
        .run(move |tx, out| {
            let category =
                category_row(tx, &id)?.ok_or_else(|| ApiError::not_found("category_not_found"))?;
            let space_id = category.text("space_id")?;
            if !perms::has_permission(tx, &user.id, &space_id, Permissions::MANAGE_CHANNELS)? {
                return Err(perms::missing("MANAGE_CHANNELS"));
            }
            let released = tx.query(
                "SELECT id, type FROM channels WHERE category_id = ?",
                params![&id],
            )?;
            tx.execute(
                "UPDATE channels SET category_id = NULL WHERE category_id = ?",
                params![&id],
            )?;
            tx.execute(
                "DELETE FROM category_overrides WHERE category_id = ?",
                params![&id],
            )?;
            tx.execute("DELETE FROM channel_categories WHERE id = ?", params![&id])?;
            out.to_space(
                &space_id,
                &json!({"type": "category_deleted", "categoryId": id, "spaceId": space_id}),
            );
            let mut moved = Vec::new();
            let mut voice_moved = false;
            for row in &released {
                moved.push(row.text("id")?);
                voice_moved |= row.text("type")? == "voice";
            }
            broadcast_layout(tx, &hub, out, &space_id, &moved)?;
            if voice_moved {
                voice::check_permissions(tx, &hub, out, &space_id)?;
            }
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `PATCH /api/spaces/:id/channel-layout`.
pub async fn update_layout(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let hub = state.hub.clone();
    state
        .run(move |tx, out| {
            space_exists(tx, &id)?;
            if !perms::has_permission(tx, &user.id, &id, Permissions::MANAGE_CHANNELS)? {
                return Err(perms::missing("MANAGE_CHANNELS"));
            }
            let (Some(channel_updates), Some(category_updates)) = (
                field(&body, "channels").and_then(Value::as_array),
                field(&body, "categories").and_then(Value::as_array),
            ) else {
                return Err(ApiError::bad_request("layout_arrays_required"));
            };
            let space_channels = tx.query(
                "SELECT id, type, category_id FROM channels WHERE space_id = ?",
                params![&id],
            )?;
            let mut known_channels = std::collections::HashMap::new();
            for row in &space_channels {
                known_channels.insert(
                    row.text("id")?,
                    (row.text("type")?, row.opt_text("category_id")?),
                );
            }
            let known_categories: HashSet<String> = tx
                .query(
                    "SELECT id FROM channel_categories WHERE space_id = ?",
                    params![&id],
                )?
                .iter()
                .map(|row| row.text("id"))
                .collect::<DbResult<_>>()?;
            let mut channel_writes = Vec::new();
            for update in channel_updates {
                let channel = update["id"].as_str().unwrap_or_default();
                if !known_channels.contains_key(channel) {
                    return Err(ApiError::bad_request("channel_not_in_space")
                        .detail("id", update["id"].clone()));
                }
                channel_writes.push((channel.to_owned(), position_of(&update["position"])?));
            }
            let mut category_writes = Vec::new();
            for update in category_updates {
                let category = update["id"].as_str().unwrap_or_default();
                if !known_categories.contains(category) {
                    return Err(ApiError::bad_request("category_not_in_space")
                        .detail("id", update["id"].clone()));
                }
                category_writes.push((category.to_owned(), position_of(&update["position"])?));
            }
            let mut moved = Vec::new();
            let mut voice_moved = false;
            for (update, (channel, position)) in channel_updates.iter().zip(&channel_writes) {
                let category = &update["categoryId"];
                let wanted = category.as_str();
                if !category.is_null()
                    && !wanted.is_some_and(|wanted| known_categories.contains(wanted))
                {
                    return Err(ApiError::bad_request("category_not_in_space")
                        .detail("id", category.clone()));
                }
                if let Some((kind, previous)) = known_channels.get(channel)
                    && previous.as_deref() != wanted
                {
                    moved.push(channel.clone());
                    voice_moved |= kind == "voice";
                }
                let _ = position;
            }
            for (update, (channel, position)) in channel_updates.iter().zip(&channel_writes) {
                tx.execute(
                    "UPDATE channels SET position = ?, category_id = ? WHERE id = ?",
                    params![*position, update["categoryId"].as_str(), channel],
                )?;
            }
            for (category, position) in &category_writes {
                tx.execute(
                    "UPDATE channel_categories SET position = ? WHERE id = ?",
                    params![*position, category],
                )?;
            }
            broadcast_layout(tx, &hub, out, &id, &moved)?;
            if voice_moved {
                voice::check_permissions(tx, &hub, out, &id)?;
            }
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `PUT /api/rustspace/channels/:id/nsfw` `{nsfw}` — канал «18+» (Rustspace).
/// Перед входом в такой канал клиент спрашивает, есть ли зрителю 18 лет.
pub async fn set_nsfw(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let nsfw = body
        .get("nsfw")
        .and_then(Value::as_bool)
        .ok_or_else(|| ApiError::bad_request("nsfw_invalid"))?;
    let hub = state.hub.clone();
    state
        .run(move |tx, out| {
            let channel =
                channel_row(tx, &id)?.ok_or_else(|| ApiError::not_found("channel_not_found"))?;
            let space_id = channel.text("space_id")?;
            if !perms::has_channel_permission(
                tx,
                &user.id,
                &space_id,
                &id,
                Permissions::MANAGE_CHANNELS,
            )? {
                return Err(perms::missing("MANAGE_CHANNELS"));
            }
            tx.execute(
                "DELETE FROM bs_channel_flags WHERE channel_id = ?",
                params![&id],
            )?;
            if nsfw {
                tx.execute(
                    "INSERT INTO bs_channel_flags (channel_id, nsfw) VALUES (?, 1)",
                    params![&id],
                )?;
            }
            let updated = channel_row(tx, &id)?.ok_or_else(ApiError::internal)?;
            let data = channel_base(&updated)?;
            broadcast::to_channel(
                tx,
                &hub,
                out,
                &space_id,
                &id,
                &json!({"type": "channel_updated", "channel": data, "spaceId": space_id}),
                None,
            )?;
            Ok(Json(data))
        })
        .await
}
