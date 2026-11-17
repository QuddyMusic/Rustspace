//! Личные переписки по REST (`routes/dm.ts`).

use std::collections::HashSet;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Tx};
use crate::dm::{self, GROUP_MAX_MEMBERS};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ClientIp, JsonBody};
use crate::messages::{Page, parse_page, select_page};
use crate::ratelimit::Limit;
use crate::routes::users::sanitize_user;
use crate::state::{AppState, now_ms};
use crate::util::{field, js_trim, str_field, utf16_len};
use crate::{broadcast, embeds, federation, files, params, ready, voice};

const SEND_LIMIT: Limit = Limit {
    name: "dm.send",
    max: 5,
    window: Duration::from_secs(5),
};
const INVITE_LIMIT: Limit = Limit {
    name: "dm.space-invite",
    max: 30,
    window: Duration::from_mins(1),
};
const NAME_MAX: usize = 50;
const ICON_MAX_BYTES: i64 = 8 * 1024 * 1024;

fn internal() -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
}

fn group_row(tx: &mut dyn Tx, id: &str) -> ApiResult<(crate::db::Row, String)> {
    let row = dm::channel_row(tx, id)?.ok_or_else(|| ApiError::not_found("dm_not_found"))?;
    let owner = row
        .opt_text("owner_id")?
        .ok_or_else(|| ApiError::bad_request("dm_not_group"))?;
    Ok((row, owner))
}

/// `GET /api/dm`.
pub async fn list(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let mut channels = ready::open_dm_channels(tx, &user.id)?;
            let time = |channel: &Value| {
                channel["lastMessage"]["createdAt"]
                    .as_i64()
                    .or_else(|| channel["createdAt"].as_i64())
                    .unwrap_or(0)
            };
            channels.sort_by_key(|channel| std::cmp::Reverse(time(channel)));
            Ok(Json(json!(channels)))
        })
        .await
}

/// `POST /api/dm`.
pub async fn open(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let app = state.clone();
    state
        .run(move |tx, _| {
            let target = dm::resolve_target(
                tx,
                str_field(&body, "userId"),
                str_field(&body, "homeUserId"),
                str_field(&body, "homeInstance"),
            )?
            .ok_or_else(|| ApiError::not_found("user_not_found"))?;
            let target_id = target.text("id")?;
            if target_id == user.id {
                return Err(ApiError::bad_request("cannot_dm_self"));
            }
            let (channel, created) =
                dm::open_one_on_one(tx, &app.config, &app.ids, &user.id, &target_id)?;
            let wire = dm::channel_wire(tx, &channel)?.ok_or_else(internal)?;
            Ok((
                if created {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                Json(wire),
            ))
        })
        .await
}

/// `POST /api/dm/group`.
pub async fn create_group(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let app = state.clone();
    state
        .run(move |tx, out| {
            let identities = field(&body, "users").and_then(Value::as_array).filter(|list| list.len() >= 2);
            let Some(identities) = identities else {
                return Err(ApiError::bad_request("group_dm_too_few_members").detail("min", 2));
            };
            if identities.iter().any(|identity| identity["id"].as_str().is_none_or(str::is_empty)) {
                return Err(ApiError::bad_request("validation_failed"));
            }
            let total = identities.len() + 1;
            if total > GROUP_MAX_MEMBERS {
                return Err(ApiError::bad_request("group_dm_too_many_members")
                    .detail("max", GROUP_MAX_MEMBERS)
                    .detail("requested", total));
            }
            let mut targets = Vec::with_capacity(identities.len());
            for identity in identities {
                let home_user = identity["homeUserId"].as_str().filter(|id| !id.is_empty());
                let home_instance = identity["homeInstance"].as_str().filter(|home| !home.is_empty());
                let found = if home_user.is_some() && home_instance.is_some() {
                    dm::resolve_target(tx, None, home_user, home_instance)?
                } else {
                    crate::routes::users::user_row(tx, identity["id"].as_str().unwrap_or_default())?
                };
                let row = found
                    .filter(|row| row.opt_flag("is_deleted").ok().flatten() != Some(true))
                    .ok_or_else(|| ApiError::not_found("users_not_found"))?;
                targets.push(row);
            }
            let mut target_ids = Vec::with_capacity(targets.len());
            for target in &targets {
                target_ids.push(target.text("id")?);
            }
            let unique: HashSet<&String> = target_ids.iter().collect();
            if unique.len() != target_ids.len() {
                return Err(ApiError::bad_request("duplicate_users"));
            }
            if target_ids.contains(&user.id) {
                return Err(ApiError::bad_request("group_dm_includes_self"));
            }
            // Участники переписки один на один, из которой делают группу,
            // могут быть не в друзьях у создателя.
            let mut exempt = HashSet::new();
            if let Some(source) = str_field(&body, "fromDmChannelId").filter(|id| !id.is_empty())
                && let Some(row) = dm::channel_row(tx, source)?
                && row.opt_text("owner_id")?.is_none()
                && dm::is_member(tx, source, &user.id)?
            {
                for member in tx.query(
                    "SELECT user_id FROM dm_members WHERE dm_channel_id = ?",
                    params![source],
                )? {
                    exempt.insert(member.text("user_id")?);
                }
            }
            for (target, id) in targets.iter().zip(&target_ids) {
                if exempt.contains(id) || dm::are_friends(tx, &user.id, id)? {
                    continue;
                }
                let name = target
                    .opt_text("display_name")?
                    .filter(|name| !name.is_empty())
                    .map_or_else(|| target.text("username"), Ok)?;
                return Err(ApiError::forbidden("not_a_friend").detail("name", name));
            }
            let channel = app.ids.next();
            let now = now_ms();
            tx.execute(
                "INSERT INTO dm_channels (id, owner_id, created_at) VALUES (?, ?, ?)",
                params![&channel, &user.id, now],
            )?;
            dm::insert_member(tx, &channel, &user.id, false, now)?;
            for id in &target_ids {
                dm::insert_member(tx, &channel, id, false, now)?;
            }
            let has_remote = targets.iter().any(|row| {
                row.opt_text("home_instance").ok().flatten().is_some_and(|home| !home.is_empty())
            });
            let mut federated = None;
            if has_remote && federation::relay_enabled(tx)? {
                let (home_user, home_instance) = federation::identity(tx, &app.config, &user.id)?;
                let key = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "UPDATE dm_channels SET federated_id = ?, owner_home_user_id = ?, owner_home_instance = ? \
                     WHERE id = ?",
                    params![&key, home_user, crate::federation_auth::canonical_home_instance(Some(&home_instance)), &channel],
                )?;
                // Часы каждого участника начинаются с создания группы.
                for id in std::iter::once(&user.id).chain(&target_ids) {
                    let member = federation::relay::identity_value(tx, &app.config, id)?;
                    federation::relay::record_member_change(tx, &app.config, &key, &member, now)?;
                }
                federated = Some(key);
            }
            let wire = dm::channel_wire(tx, &channel)?.ok_or_else(internal)?;
            let created = json!({"type": "dm_channel_created", "dmChannel": wire});
            let local = |row: &crate::db::Row| {
                row.opt_text("home_instance").ok().flatten().is_none_or(|home| home.is_empty())
            };
            let mut audience = vec![user.id.clone()];
            for (target, id) in targets.iter().zip(&target_ids) {
                if local(target) {
                    audience.push(id.clone());
                }
            }
            for member in &audience {
                broadcast::to_user(tx, out, member, &created)?;
            }
            for id in &target_ids {
                let content = dm::member_added_content(tx, id)?;
                let message = dm::system_message(tx, &app.ids, &channel, &user.id, &content, None, now)?;
                let event = json!({"type": "dm_message_created", "message": message});
                for member in &audience {
                    broadcast::to_user(tx, out, member, &event)?;
                }
            }
            if federated.is_some() {
                let caller = federation::relay::identity_value(tx, &app.config, &user.id)?;
                let ours = federation::our_origin(&app.config);
                for (target, id) in targets.iter().zip(&target_ids) {
                    let home = target.opt_text("home_instance")?.filter(|home| !home.is_empty());
                    if home.is_none_or(|home| home == ours) {
                        continue;
                    }
                    federation::relay::member_add(
                        tx,
                        federation::relay::Relay { config: &app.config, ids: &app.ids },
                        &channel,
                        id,
                        &caller,
                        &caller,
                        now,
                    )?;
                }
            }
            Ok((StatusCode::CREATED, Json(wire)))
        })
        .await
}

fn valid_asset_url(url: &str) -> bool {
    url.starts_with("/api/uploads/")
        || url.starts_with("https://")
        || url.starts_with("http://")
        || (!url.contains('/') && !url.contains('\\') && !url.contains(".."))
}

/// `PATCH /api/dm/:id` — имя и значок группы.
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let app = state.clone();
    let config = state.config.clone();
    let (reply, stale) = state
        .run(move |tx, out| {
            let (row, owner) = group_row(tx, &id)?;
            if !dm::is_member(tx, &id, &user.id)? {
                return Err(ApiError::forbidden("not_dm_member"));
            }
            if owner != user.id {
                return Err(ApiError::forbidden("dm_owner_only"));
            }
            let old_name = row.opt_text("name")?;
            let old_icon = row.opt_text("icon")?;
            let unchanged = json!({
                "id": id,
                "name": old_name,
                "icon": old_icon,
                "metadataUpdatedAt": row.opt_int("metadata_updated_at")?.unwrap_or(0),
            });
            let mut name = old_name.clone();
            let mut icon = old_icon.clone();
            if let Some(raw) = field(&body, "name") {
                let candidate = match raw {
                    Value::Null => None,
                    Value::String(text) => Some(js_trim(text)).filter(|text| !text.is_empty()).map(str::to_owned),
                    _ => return Err(ApiError::bad_request("validation_failed")),
                };
                if candidate != old_name {
                    if candidate.as_deref().is_some_and(|name| utf16_len(name) > NAME_MAX) {
                        return Err(ApiError::bad_request("group_dm_name_length")
                            .detail("min", 1)
                            .detail("max", NAME_MAX));
                    }
                    name = candidate;
                }
            }
            if let Some(raw) = field(&body, "icon") {
                let candidate = match raw {
                    Value::Null => None,
                    Value::String(text) => {
                        let trimmed = js_trim(text);
                        if trimmed.is_empty() {
                            None
                        } else if !valid_asset_url(trimmed) {
                            return Err(ApiError::bad_request("icon_url_invalid"));
                        } else {
                            Some(trimmed.strip_prefix("/api/uploads/").unwrap_or(trimmed).to_owned())
                        }
                    }
                    _ => return Err(ApiError::bad_request("validation_failed")),
                };
                if candidate != old_icon {
                    if let Some(file) = candidate.as_deref().filter(|icon| !icon.starts_with("http")) {
                        let attachment = tx
                            .query_one(
                                "SELECT uploader_id, mimetype, size FROM attachments WHERE filename = ?",
                                params![file],
                            )?
                            .ok_or_else(|| ApiError::bad_request("attachment_not_found"))?;
                        if attachment.opt_text("uploader_id")?.as_deref() != Some(user.id.as_str()) {
                            return Err(ApiError::forbidden("attachment_not_owned"));
                        }
                        if !attachment.text("mimetype")?.starts_with("image/") {
                            return Err(ApiError::bad_request("icon_not_image"));
                        }
                        if attachment.int("size")? > ICON_MAX_BYTES {
                            return Err(ApiError::bad_request("icon_too_large").detail("max", 8));
                        }
                    }
                    icon = candidate;
                }
            }
            let name_changed = name != old_name;
            let icon_changed = icon != old_icon;
            if !name_changed && !icon_changed {
                return Ok((unchanged, Vec::new()));
            }
            let at = now_ms();
            let event_id = app.ids.next();
            tx.execute(
                "UPDATE dm_channels SET name = ?, icon = ?, metadata_updated_at = ? WHERE id = ?",
                params![&name, &icon, at, &id],
            )?;
            let mut system = Vec::new();
            if name_changed {
                let content = dm::system_content(&[
                    ("event", json!("name_changed")),
                    ("oldName", json!(old_name)),
                    ("newName", json!(name)),
                ]);
                let source = format!("{event_id}:name");
                system.push(dm::system_message(tx, &app.ids, &id, &user.id, &content, Some(&source), at)?);
            }
            if icon_changed {
                let content = dm::system_content(&[("event", json!("icon_changed"))]);
                let source = format!("{event_id}:icon");
                system.push(dm::system_message(tx, &app.ids, &id, &user.id, &content, Some(&source), at)?);
            }
            broadcast::to_dm_members(
                tx,
                out,
                &id,
                &json!({"type": "dm_channel_updated", "dmChannelId": id, "name": name, "icon": icon}),
                None,
            )?;
            for message in system {
                broadcast::to_dm_members(
                    tx,
                    out,
                    &id,
                    &json!({"type": "dm_message_created", "message": message}),
                    None,
                )?;
            }
            federation::relay::group_metadata(
                tx,
                federation::relay::Relay { config: &app.config, ids: &app.ids },
                &id,
                &user.id,
                name.as_deref(),
                icon.as_deref(),
                at,
            )?;
            let mut stale = Vec::new();
            if icon_changed {
                if let Some(old) = old_icon.filter(|icon| !icon.starts_with("http")) {
                    files::delete_attachment_by_filename(tx, &old)?;
                    stale.push(old);
                }
                if let Some(new) = icon.as_deref().filter(|icon| !icon.starts_with("http")) {
                    files::delete_attachment_by_filename(tx, new)?;
                }
            }
            Ok((json!({"id": id, "name": name, "icon": icon, "metadataUpdatedAt": at}), stale))
        })
        .await?;
    for file in &stale {
        files::delete_upload_file(&config, file).await;
    }
    Ok(Json(reply))
}

/// `DELETE /api/dm/:id` — закрыть переписку у себя.
pub async fn close(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let app = state.clone();
    state
        .run(move |tx, out| {
            if !dm::is_member(tx, &id, &user.id)? {
                return Err(ApiError::forbidden("not_dm_member"));
            }
            dm::set_closed(tx, &id, &user.id, true)?;
            out.to_user(
                &user.id,
                &json!({"type": "dm_channel_closed", "dmChannelId": id}),
            );
            federation::queue_dm_close(tx, &app.config, &app.ids, &id, &user.id, false)?;
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `POST /api/dm/:id/members`.
pub async fn add_member(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let app = state.clone();
    state
        .run(move |tx, out| {
            let target = dm::resolve_target(
                tx,
                str_field(&body, "userId"),
                str_field(&body, "homeUserId"),
                str_field(&body, "homeInstance"),
            )?
            .ok_or_else(|| ApiError::not_found("user_not_found"))?;
            let target_id = target.text("id")?;
            if !dm::is_member(tx, &id, &user.id)? {
                return Err(ApiError::forbidden("not_dm_member"));
            }
            if crate::routes::saved::is_saved(tx, &id)? {
                return Err(ApiError::bad_request("saved_messages_private"));
            }
            group_row(tx, &id)?;
            if !dm::are_friends(tx, &user.id, &target_id)? {
                return Err(ApiError::forbidden("not_a_friend"));
            }
            if dm::is_member(tx, &id, &target_id)? {
                return Err(ApiError::bad_request("already_member"));
            }
            let current: Vec<String> = tx
                .query("SELECT user_id FROM dm_members WHERE dm_channel_id = ?", params![&id])?
                .iter()
                .map(|row| row.text("user_id"))
                .collect::<DbResult<_>>()?;
            if current.len() >= GROUP_MAX_MEMBERS {
                return Err(ApiError::bad_request("group_dm_too_many_members").detail("max", GROUP_MAX_MEMBERS));
            }
            dm::insert_member(tx, &id, &target_id, false, now_ms())?;
            let remote = target.opt_text("home_instance")?.is_some_and(|home| !home.is_empty());
            if remote && federation::relay_enabled(tx)? && federation::dm_federated_id(tx, &id)?.is_none() {
                let owner = group_row(tx, &id)?.1;
                let (home_user, home_instance) = federation::identity(tx, &app.config, &owner)?;
                tx.execute(
                    "UPDATE dm_channels SET federated_id = ?, owner_home_user_id = ?, owner_home_instance = ? \
                     WHERE id = ?",
                    params![
                        uuid::Uuid::new_v4().to_string(),
                        home_user,
                        crate::federation_auth::canonical_home_instance(Some(&home_instance)),
                        &id
                    ],
                )?;
            }
            let wire = dm::channel_wire(tx, &id)?.ok_or_else(|| ApiError::not_found("dm_not_found"))?;
            let added = json!({
                "type": "dm_member_added",
                "dmChannelId": id,
                "user": sanitize_user(&target, false)?,
            });
            for member in &current {
                out.to_user(member, &added);
            }
            broadcast::to_user(tx, out, &target_id, &json!({"type": "dm_channel_created", "dmChannel": wire}))?;
            let content = dm::member_added_content(tx, &target_id)?;
            let message = dm::system_message(tx, &app.ids, &id, &user.id, &content, None, now_ms())?;
            broadcast::to_dm_members(
                tx,
                out,
                &id,
                &json!({"type": "dm_message_created", "message": message}),
                None,
            )?;
            let added_at = now_ms();
            if let Some(federated) = federation::dm_federated_id(tx, &id)? {
                let added = federation::relay::identity_value(tx, &app.config, &target_id)?;
                federation::relay::record_member_change(tx, &app.config, &federated, &added, added_at)?;
                if federation::relay_enabled(tx)? {
                    let adder = federation::relay::identity_value(tx, &app.config, &user.id)?;
                    let ours = federation::our_origin(&app.config);
                    let channel = dm::channel_row(tx, &id)?;
                    let owner_field = |column: &str| -> DbResult<Option<String>> {
                        Ok(channel
                            .as_ref()
                            .map(|row| row.opt_text(column))
                            .transpose()?
                            .flatten()
                            .filter(|value| !value.is_empty()))
                    };
                    let owner = json!({
                        "homeUserId": owner_field("owner_home_user_id")?.unwrap_or_else(|| user.id.clone()),
                        "homeInstance": owner_field("owner_home_instance")?.unwrap_or(ours),
                    });
                    federation::relay::member_add(
                        tx,
                        federation::relay::Relay { config: &app.config, ids: &app.ids },
                        &id,
                        &target_id,
                        &adder,
                        &owner,
                        added_at,
                    )?;
                }
            }
            voice::dm_member_added(tx, &app.hub, out, &id, &target_id)?;
            Ok(Json(wire))
        })
        .await
}

/// `DELETE /api/dm/:id/members` — выйти из группы.
pub async fn leave(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let app = state.clone();
    state
        .run(move |tx, out| {
            if !dm::is_member(tx, &id, &user.id)? {
                return Err(ApiError::forbidden("not_dm_member"));
            }
            group_row(tx, &id)?;
            voice::dm_member_left(tx, &app.hub, out, &id, &user.id)?;
            dm::remove_member(
                tx,
                out,
                &app.config,
                &app.ids,
                &id,
                &user.id,
                &user.id,
                false,
            )?;
            out.to_user(
                &user.id,
                &json!({"type": "dm_channel_closed", "dmChannelId": id}),
            );
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// Параметр `homeInstance` при исключении участника.
#[derive(Debug, serde::Deserialize)]
pub struct KickQuery {
    #[serde(rename = "homeInstance")]
    home_instance: Option<String>,
}

/// `DELETE /api/dm/:id/members/:targetUserId` — исключить из группы.
pub async fn kick(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, target)): Path<(String, String)>,
    Query(query): Query<KickQuery>,
) -> ApiResult<Json<Value>> {
    let app = state.clone();
    state
        .run(move |tx, out| {
            let home = query
                .home_instance
                .as_deref()
                .filter(|home| !home.is_empty());
            let row = match home {
                Some(home) => dm::resolve_target(tx, None, Some(&target), Some(home))?,
                None => crate::routes::users::user_row(tx, &target)?,
            }
            .ok_or_else(|| ApiError::not_found("user_not_found"))?;
            let target_id = row.text("id")?;
            let (_, owner) = group_row(tx, &id)?;
            if owner != user.id {
                return Err(ApiError::forbidden("dm_owner_only"));
            }
            if target_id == user.id {
                return Err(ApiError::bad_request("owner_cannot_kick_self"));
            }
            if !dm::is_member(tx, &id, &target_id)? {
                return Err(ApiError::not_found("target_not_dm_member"));
            }
            voice::dm_member_left(tx, &app.hub, out, &id, &target_id)?;
            dm::remove_member(
                tx,
                out,
                &app.config,
                &app.ids,
                &id,
                &user.id,
                &target_id,
                true,
            )?;
            out.to_user(
                &target_id,
                &json!({"type": "dm_channel_closed", "dmChannelId": id}),
            );
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `POST /api/dm/:id/transfer` — передать владение группой.
pub async fn transfer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let app = state.clone();
    state
        .run(move |tx, out| {
            let home_user = str_field(&body, "homeUserId").filter(|id| !id.is_empty());
            let home_instance = str_field(&body, "homeInstance").filter(|home| !home.is_empty());
            let local = str_field(&body, "newOwnerId").filter(|id| !id.is_empty());
            let federated = home_user.is_some() && home_instance.is_some();
            if !federated && local.is_none() {
                return Err(ApiError::bad_request("new_owner_required"));
            }
            let row = if federated {
                dm::resolve_target(tx, None, home_user, home_instance)?
            } else {
                crate::routes::users::user_row(tx, local.unwrap_or_default())?
            }
            .ok_or_else(|| ApiError::not_found("user_not_found"))?;
            let new_owner = row.text("id")?;
            let (_, owner) = group_row(tx, &id)?;
            if owner != user.id {
                return Err(ApiError::forbidden("dm_owner_only"));
            }
            if new_owner == owner {
                return Err(ApiError::bad_request("already_owner"));
            }
            if !dm::is_member(tx, &id, &new_owner)? {
                return Err(ApiError::bad_request("target_not_dm_member"));
            }
            dm::transfer_ownership(tx, out, &app.config, &app.ids, &id, &owner, &new_owner)?;
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `GET /api/dm/:id/messages`.
pub async fn history(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
) -> ApiResult<Response> {
    let page = parse_page(&query).map_err(ApiError::bad_request)?;
    let after = matches!(page, Page::After(..));
    let list = state
        .run(move |tx, _| {
            if !dm::is_member(tx, &id, &user.id)? {
                return Err(ApiError::forbidden("not_dm_member"));
            }
            let rows = select_page(
                tx,
                dm::DM_MESSAGE_COLUMNS,
                "dm_messages",
                "dm_channel_id",
                &id,
                &page,
            )?;
            let mut list = Vec::with_capacity(rows.len());
            for row in &rows {
                if let Some(message) = dm::message_json(tx, row)? {
                    list.push(message);
                }
            }
            Ok(list)
        })
        .await?;
    let mut response = Json(json!(list)).into_response();
    if after {
        response.headers_mut().insert(
            HeaderName::from_static("x-backspace-paging"),
            HeaderValue::from_static("after"),
        );
    }
    Ok(response)
}

/// `POST /api/dm/:id/messages`.
pub async fn create_message(
    State(state): State<AppState>,
    user: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<(StatusCode, Json<Value>)> {
    state.limiter.check(SEND_LIMIT, ip)?;
    let app = state.clone();
    let channel = id.clone();
    let message = state
        .run(move |tx, out| {
            let new = dm::check_create(tx, &id, &user.id, &body)?.map_err(dm::Refusal::into_api)?;
            dm::create_message(tx, out, &app.config, &app.ids, &id, &user.id, &new)?.ok_or_else(
                || ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "message_create_failed"),
            )
        })
        .await?;
    embeds::schedule(
        &state,
        message["id"].as_str().unwrap_or_default().to_owned(),
        message["content"].as_str().map(str::to_owned),
        embeds::Place::Dm {
            dm_channel_id: channel,
        },
    );
    Ok((StatusCode::CREATED, Json(message)))
}

/// `PATCH /api/dm/messages/:id`.
pub async fn update_message(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let app = state.clone();
    let message = state
        .run(move |tx, out| {
            let (row, content) = dm::check_edit(tx, &id, &user.id, field(&body, "content"))?
                .map_err(dm::Refusal::into_api)?;
            dm::edit_message(tx, out, &app.config, &app.ids, &row, &content)?.ok_or_else(|| {
                ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "message_update_failed")
            })
        })
        .await?;
    embeds::schedule(
        &state,
        message["id"].as_str().unwrap_or_default().to_owned(),
        message["content"].as_str().map(str::to_owned),
        embeds::Place::Dm {
            dm_channel_id: message["dmChannelId"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        },
    );
    Ok(Json(message))
}

/// `DELETE /api/dm/messages/:id`.
pub async fn delete_message(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let app = state.clone();
    let stale = state
        .run(move |tx, out| {
            let row = dm::check_delete(tx, &id, &user.id)?.map_err(dm::Refusal::into_api)?;
            Ok(dm::delete_message(
                tx,
                out,
                &app.config,
                &app.ids,
                &row,
                &user.id,
            )?)
        })
        .await?;
    for file in &stale {
        files::delete_upload_file(&state.config, file).await;
    }
    Ok(Json(json!({"success": true})))
}

/// `POST /api/dm/space-invite` — приглашение в пространство личным сообщением.
pub async fn space_invite(
    State(state): State<AppState>,
    user: AuthUser,
    ClientIp(ip): ClientIp,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    state.limiter.check(INVITE_LIMIT, ip)?;
    let app = state.clone();
    state
        .run(move |tx, out| {
            let space_id = str_field(&body, "spaceId").filter(|id| !id.is_empty());
            let code = str_field(&body, "inviteCode").filter(|code| !code.is_empty());
            let target = field(&body, "target").filter(|target| crate::util::truthy(Some(target)));
            let (Some(space_id), Some(code), Some(target)) = (space_id, code, target) else {
                return Err(ApiError::bad_request("invalid_body"));
            };
            let row = if target.get("userId").is_some() {
                crate::routes::users::user_row(tx, target["userId"].as_str().unwrap_or_default())?
            } else if target.get("homeUserId").is_some() && target.get("homeInstance").is_some() {
                dm::resolve_target(
                    tx,
                    None,
                    target["homeUserId"].as_str(),
                    target["homeInstance"].as_str(),
                )
                .unwrap_or(None)
            } else {
                return Err(ApiError::bad_request("invalid_target"));
            }
            .ok_or_else(|| ApiError::not_found("user_not_found"))?;
            let target_id = row.text("id")?;
            if target_id == user.id {
                return Err(ApiError::bad_request("cannot_invite_self"));
            }
            if !dm::are_friends(tx, &user.id, &target_id)? {
                return Err(ApiError::bad_request("not_a_friend"));
            }
            let ours = federation::our_origin(&app.config);
            let origin =
                str_field(&body, "spaceInstanceOrigin").filter(|origin| !origin.is_empty());
            // Снимок чужого пространства запрашивается у его инстанса; эта
            // часть федерации ещё не перенесена.
            if origin.is_some_and(|origin| origin != ours) {
                return Err(ApiError::bad_request("invite_invalid"));
            }
            let snapshot = crate::routes::spaces::invite_preview_json(tx, code)?
                .filter(|snapshot| snapshot["spaceId"].as_str() == Some(space_id))
                .ok_or_else(|| ApiError::bad_request("invite_invalid"))?;
            let content = dm::system_content(&[
                ("event", json!("space_invite")),
                ("spaceId", json!(space_id)),
                ("spaceInstanceOrigin", json!(ours)),
                ("inviteCode", json!(code)),
                (
                    "snapshot",
                    Value::String(String::new()), // заменяется ниже: порядок ключей вложенного объекта
                ),
            ]);
            let inner = dm::system_content(&[
                ("spaceName", snapshot["spaceName"].clone()),
                ("icon", snapshot["icon"].clone()),
                ("avatarColor", snapshot["avatarColor"].clone()),
                ("memberCount", snapshot["memberCount"].clone()),
                ("description", snapshot["description"].clone()),
                ("instanceName", snapshot["instanceName"].clone()),
            ]);
            let content =
                content.replacen("\"snapshot\":\"\"", &format!("\"snapshot\":{inner}"), 1);
            let (channel, _) =
                dm::open_one_on_one(tx, &app.config, &app.ids, &user.id, &target_id)?;
            let message_id = app.ids.next();
            tx.execute(
                "INSERT INTO dm_messages (id, dm_channel_id, user_id, content, type, created_at) \
                 VALUES (?, ?, ?, ?, 'system', ?)",
                params![&message_id, &channel, &user.id, &content, now_ms()],
            )?;
            let stored = dm::message_row(tx, &message_id)?;
            let message = match stored {
                Some(stored) => dm::message_json(tx, &stored)?,
                None => None,
            }
            .ok_or_else(|| {
                ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "message_lookup_failed")
            })?;
            dm::broadcast_message(tx, out, &channel, &message)?;
            federation::queue_dm_message(tx, &app.config, &app.ids, &channel, &message_id, false)?;
            Ok(Json(
                json!({"dmChannelId": channel, "messageId": message_id, "message": message}),
            ))
        })
        .await
}
