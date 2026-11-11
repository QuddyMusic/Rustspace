//! Пространства: создание, настройки, участники, роли, баны (`routes/spaces.ts`).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use bs_permissions::{
    DEFAULT_EVERYONE_PERMISSIONS, Permissions, can_act_on_member, can_manage_role_at,
    permissions_version, role_bits_change_refusal,
};
use rand::Rng;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Row, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::JsonBody;
use crate::hub::Outbox;
use crate::routes::auth::AVATAR_COLORS;
use crate::routes::users::{USER_COLUMNS, sanitize_user};
use crate::state::{AppState, now_ms};
use crate::util::{field, js_trim, str_field, utf16_len, utf16_prefix};
use crate::{broadcast, files, params, perms, ready};

const VISIBILITIES: [&str; 3] = ["public", "request", "private"];

/// Пространство без каналов и участников (`rowToSpace`).
///
/// # Errors
/// В строке нет нужных столбцов.
pub fn space_json(row: &Row) -> DbResult<Value> {
    Ok(json!({
        "id": row.text("id")?,
        "name": row.text("name")?,
        "icon": row.opt_text("icon")?,
        "banner": row.opt_text("banner")?,
        "avatarColor": row.opt_text("avatar_color")?,
        "ownerId": row.text("owner_id")?,
        "ownerTitle": row.opt_text("owner_title")?,
        "inviteCode": row.opt_text("invite_code")?,
        "visibility": row.opt_text("visibility")?.unwrap_or_else(|| "private".to_owned()),
        "directoryListed": row.opt_flag("directory_listed")? == Some(true),
        "description": row.opt_text("description")?,
        "createdAt": row.int("created_at")?,
    }))
}

/// Строка пространства.
///
/// # Errors
/// Ошибка базы.
pub fn space_row(tx: &mut dyn Tx, id: &str) -> DbResult<Option<Row>> {
    tx.query_one(
        &format!("SELECT {} FROM spaces WHERE id = ?", ready::SPACE_COLUMNS),
        params![id],
    )
}

fn require_space(tx: &mut dyn Tx, id: &str) -> ApiResult<Row> {
    space_row(tx, id)?.ok_or_else(|| ApiError::not_found("space_not_found"))
}

fn invite_code() -> String {
    hex::encode(rand::rng().random::<[u8; 4]>())
}

/// Отметка «каталог изменился» (`markDirectoryDirty`).
///
/// # Errors
/// Ошибка базы.
pub fn mark_directory_dirty(tx: &mut dyn Tx) -> DbResult<()> {
    tx.execute(
        "UPDATE instance_settings SET directory_dirty = 1 WHERE id = 1",
        params![],
    )?;
    Ok(())
}

/// Пространство принимает ссылку на картинку, которая стала его значком или
/// обложкой: запись о вложении больше не нужна.
fn adopt_image(tx: &mut dyn Tx, url: Option<&str>) -> DbResult<()> {
    if let Some(url) = url.filter(|url| url.contains("/api/uploads/")) {
        files::delete_attachment_by_filename(tx, url)?;
    }
    Ok(())
}

/// `POST /api/spaces`.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = str_field(&body, "name")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| ApiError::bad_request("space_name_required"))?;
    let name = js_trim(name).to_owned();
    if name.is_empty() || utf16_len(&name) > 100 {
        return Err(ApiError::bad_request("space_name_length")
            .detail("min", 1)
            .detail("max", 100));
    }
    let visibility = str_field(&body, "visibility")
        .filter(|value| VISIBILITIES.contains(value))
        .unwrap_or("private")
        .to_owned();
    let description = str_field(&body, "description")
        .map(|text| utf16_prefix(js_trim(text), 200).to_owned())
        .filter(|text| !text.is_empty());
    let color = str_field(&body, "avatarColor")
        .filter(|color| AVATAR_COLORS.contains(color))
        .map_or_else(
            || AVATAR_COLORS[rand::rng().random_range(0..AVATAR_COLORS.len())].to_owned(),
            str::to_owned,
        );
    let icon = str_field(&body, "icon").map(str::to_owned);
    let banner = str_field(&body, "banner").map(str::to_owned);
    let ids = state.ids.clone();
    let config = state.config.clone();
    let images = (icon.clone(), banner.clone());
    let space = state
        .run(move |tx, out| {
            let space_id = ids.next();
            let text_category = ids.next();
            let voice_category = ids.next();
            let text_channel = ids.next();
            let voice_channel = ids.next();
            let now = now_ms();
            tx.execute(
                "INSERT INTO spaces (id, name, icon, banner, avatar_color, owner_id, invite_code, \
                 visibility, description, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    &space_id, &name, &icon, &banner, &color, &user.id, invite_code(), &visibility,
                    &description, now
                ],
            )?;
            tx.execute(
                "INSERT INTO space_members (space_id, user_id, joined_at) VALUES (?, ?, ?)",
                params![&space_id, &user.id, now],
            )?;
            for (id, name, position) in
                [(&text_category, "text-channels", 0), (&voice_category, "voice-channels", 1)]
            {
                tx.execute(
                    "INSERT INTO channel_categories (id, space_id, name, position, created_at) \
                     VALUES (?, ?, ?, ?, ?)",
                    params![id, &space_id, name, i64::from(position), now],
                )?;
            }
            for (id, name, kind, category) in [
                (&text_channel, "general", "text", &text_category),
                (&voice_channel, "voice", "voice", &voice_category),
            ] {
                tx.execute(
                    "INSERT INTO channels (id, space_id, name, type, position, category_id, created_at) \
                     VALUES (?, ?, ?, ?, 0, ?, ?)",
                    params![id, &space_id, name, kind, category, now],
                )?;
            }
            tx.execute(
                "INSERT INTO roles (id, space_id, name, color, position, permissions, created_at) \
                 VALUES (?, ?, '@everyone', '#b9bbbe', 0, ?, ?)",
                params![&space_id, &space_id, perms::to_wire(DEFAULT_EVERYONE_PERMISSIONS), now],
            )?;
            adopt_image(tx, icon.as_deref())?;
            adopt_image(tx, banner.as_deref())?;
            out.join_space(&user.id, &space_id);
            let row = space_row(tx, &space_id)?
                .ok_or_else(|| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "space_create_failed"))?;
            Ok(space_json(&row)?)
        })
        .await?;
    files::resize_profile_image(&config, images.0.as_deref(), files::ProfileImage::Icon).await;
    files::resize_profile_image(&config, images.1.as_deref(), files::ProfileImage::Banner).await;
    Ok((StatusCode::CREATED, Json(space)))
}

/// `GET /api/spaces`.
pub async fn list(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let rows = tx.query(
                "SELECT s.id, s.name, s.icon, s.banner, s.avatar_color, s.owner_id, s.invite_code, \
                 s.visibility, s.description, s.created_at, s.directory_listed, s.owner_title \
                 FROM space_members m JOIN spaces s ON s.id = m.space_id WHERE m.user_id = ?",
                params![&user.id],
            )?;
            Ok(Json(json!(
                rows.iter().map(space_json).collect::<DbResult<Vec<_>>>()?
            )))
        })
        .await
}

/// `GET /api/spaces/:id`.
pub async fn detail(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let space = require_space(tx, &id)?;
            if !perms::is_member(tx, &id, &user.id)? {
                return Err(ApiError::forbidden("not_space_member"));
            }
            let (mut view, _) = ready::space_view(tx, &space, &user.id)?;
            // В ответе REST у каналов нет `lastMessageId` — он есть только в `ready`.
            for channel in view["channels"].as_array_mut().into_iter().flatten() {
                if let Some(object) = channel.as_object_mut() {
                    object.remove("lastMessageId");
                }
            }
            Ok(Json(view))
        })
        .await
}

/// `PATCH /api/spaces/:id`.
#[allow(
    clippy::too_many_lines,
    reason = "одна форма — один обработчик, как в оригинале"
)]
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let config = state.config.clone();
    let (space, icon, banner, stale) = state
        .run(move |tx, out| {
            let space = require_space(tx, &id)?;
            let owner_title = field(&body, "ownerTitle");
            if owner_title.is_some() && space.text("owner_id")? != user.id {
                return Err(ApiError::forbidden("space_owner_only"));
            }
            if !perms::has_permission(tx, &user.id, &id, Permissions::MANAGE_SPACE)? {
                return Err(perms::missing("MANAGE_SPACE"));
            }
            let mut sets: Vec<(&str, crate::db::Value)> = Vec::new();
            if let Some(title) = owner_title {
                let title = crate::util::normalize_owner_title(title).map_err(|()| {
                    ApiError::bad_request("space_owner_title_invalid")
                        .detail("max", crate::util::MAX_LABEL_LENGTH)
                })?;
                sets.push(("owner_title", title.into()));
            }
            if let Some(name) = field(&body, "name") {
                let name = js_trim(
                    name.as_str()
                        .ok_or_else(|| ApiError::bad_request("validation_failed"))?,
                );
                if name.is_empty() || utf16_len(name) > 100 {
                    return Err(ApiError::bad_request("space_name_length")
                        .detail("min", 1)
                        .detail("max", 100));
                }
                sets.push(("name", name.into()));
            }
            let old_icon = space.opt_text("icon")?;
            let old_banner = space.opt_text("banner")?;
            let image = |name: &str| {
                field(&body, name).map(|value| {
                    value
                        .as_str()
                        .filter(|text| !text.is_empty())
                        .map(str::to_owned)
                })
            };
            let new_icon = image("icon");
            let new_banner = image("banner");
            if let Some(icon) = &new_icon {
                sets.push(("icon", icon.clone().into()));
            }
            if let Some(banner) = &new_banner {
                sets.push(("banner", banner.clone().into()));
            }
            if let Some(color) = field(&body, "avatarColor") {
                match color.as_str() {
                    Some("") => sets.push(("avatar_color", crate::db::Value::Null)),
                    Some(color) if AVATAR_COLORS.contains(&color) => {
                        sets.push(("avatar_color", color.into()));
                    }
                    _ => return Err(ApiError::bad_request("avatar_color_invalid")),
                }
            }
            let old_visibility = space
                .opt_text("visibility")?
                .unwrap_or_else(|| "private".to_owned());
            let mut visibility = old_visibility;
            if let Some(value) = field(&body, "visibility") {
                let value = value
                    .as_str()
                    .filter(|value| VISIBILITIES.contains(value))
                    .ok_or_else(|| ApiError::bad_request("space_visibility_invalid"))?;
                visibility = value.to_owned();
                sets.push(("visibility", value.into()));
            }
            if let Some(description) = field(&body, "description") {
                let text = description
                    .as_str()
                    .ok_or_else(|| ApiError::bad_request("validation_failed"))?;
                let text = utf16_prefix(js_trim(text), 200);
                sets.push((
                    "description",
                    Some(text).filter(|text| !text.is_empty()).into(),
                ));
            }
            let was_listed = space.opt_flag("directory_listed")? == Some(true);
            let mut listed: Option<bool> = None;
            if let Some(value) = field(&body, "directoryListed") {
                let value = value.as_bool().ok_or_else(|| {
                    ApiError::bad_request("field_not_boolean").detail("field", "directoryListed")
                })?;
                if value && visibility == "private" {
                    return Err(ApiError::bad_request("directory_private_space"));
                }
                listed = Some(value);
            }
            if visibility == "private" && was_listed {
                listed = Some(false);
            }
            if let Some(listed) = listed {
                sets.push(("directory_listed", listed.into()));
            }
            if sets.is_empty() {
                return Err(ApiError::bad_request("no_fields_to_update"));
            }
            let assignments = sets
                .iter()
                .map(|(column, _)| format!("{column} = ?"))
                .collect::<Vec<_>>();
            let mut values: Vec<crate::db::Value> =
                sets.iter().map(|(_, value)| value.clone()).collect();
            values.push(id.as_str().into());
            tx.execute(
                &format!("UPDATE spaces SET {} WHERE id = ?", assignments.join(", ")),
                &values,
            )?;
            let updated = space_row(tx, &id)?.ok_or_else(|| {
                ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "space_update_failed")
            })?;

            let listed_after = updated.opt_flag("directory_listed")? == Some(true);
            let served_changed = [
                "name",
                "description",
                "icon",
                "banner",
                "avatar_color",
                "visibility",
            ]
            .iter()
            .any(|column| {
                sets.iter().any(|(name, _)| name == column)
                    && space.opt_text(column).ok() != updated.opt_text(column).ok()
            });
            if listed_after != was_listed || (listed_after && served_changed) {
                mark_directory_dirty(tx)?;
            }

            // Старые файлы значка и обложки больше не нужны.
            let mut stale = Vec::new();
            for (old, new) in [(&old_icon, &new_icon), (&old_banner, &new_banner)] {
                if let (Some(old), Some(new)) = (old, new)
                    && Some(old) != new.as_ref()
                    && !old.starts_with("http")
                {
                    files::delete_attachment_by_filename(tx, old)?;
                    stale.push(old.clone());
                }
            }
            let icon = new_icon.flatten();
            let banner = new_banner.flatten();
            adopt_image(tx, icon.as_deref())?;
            adopt_image(tx, banner.as_deref())?;

            let data = space_json(&updated)?;
            out.to_space(&id, &json!({"type": "space_updated", "space": data}));
            Ok((data, icon, banner, stale))
        })
        .await?;
    for file in &stale {
        files::delete_upload_file(&config, file).await;
    }
    files::resize_profile_image(&config, icon.as_deref(), files::ProfileImage::Icon).await;
    files::resize_profile_image(&config, banner.as_deref(), files::ProfileImage::Banner).await;
    Ok(Json(space))
}

/// `DELETE /api/spaces/:id`.
pub async fn delete(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let config = state.config.clone();
    let stale = state
        .run(move |tx, out| {
            let space = require_space(tx, &id)?;
            if space.text("owner_id")? != user.id {
                return Err(ApiError::forbidden("space_owner_only"));
            }
            let mut stale: Vec<String> = tx
                .query(
                    "SELECT a.filename FROM attachments a JOIN messages m ON m.id = a.message_id \
                     JOIN channels c ON c.id = m.channel_id WHERE c.space_id = ?",
                    params![&id],
                )?
                .iter()
                .map(|row| row.text("filename"))
                .collect::<DbResult<_>>()?;
            tx.execute(
                "DELETE FROM read_states WHERE channel_id IN (SELECT id FROM channels WHERE space_id = ?)",
                params![&id],
            )?;
            tx.execute("DELETE FROM channels WHERE space_id = ?", params![&id])?;
            tx.execute("DELETE FROM space_members WHERE space_id = ?", params![&id])?;
            tx.execute("DELETE FROM space_folder_members WHERE space_id = ?", params![&id])?;
            tx.execute("DELETE FROM spaces WHERE id = ?", params![&id])?;
            if space.opt_flag("directory_listed")? == Some(true) {
                mark_directory_dirty(tx)?;
            }
            for column in ["icon", "banner"] {
                if let Some(url) = space.opt_text(column)?.filter(|url| !url.starts_with("http")) {
                    stale.push(url);
                }
            }
            // Оригинал участникам ничего не сообщает; подписки на события
            // удалённого пространства снимаем.
            out.drop_space(&id);
            Ok(stale)
        })
        .await?;
    for file in &stale {
        files::delete_upload_file(&config, file).await;
    }
    Ok(Json(json!({"success": true})))
}

/// `POST /api/spaces/:id/invite`.
pub async fn invite(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let space = require_space(tx, &id)?;
            if !perms::has_permission(tx, &user.id, &id, Permissions::CREATE_INVITE)? {
                if !perms::is_member(tx, &id, &user.id)? {
                    return Err(ApiError::forbidden("not_space_member"));
                }
                return Err(perms::missing("CREATE_INVITE"));
            }
            if let Some(code) = space
                .opt_text("invite_code")?
                .filter(|code| !code.is_empty())
            {
                return Ok(Json(json!({"inviteCode": code})));
            }
            let code = invite_code();
            tx.execute(
                "UPDATE spaces SET invite_code = ? WHERE id = ?",
                params![&code, &id],
            )?;
            Ok(Json(json!({"inviteCode": code})))
        })
        .await
}

/// Почему действующий код приглашения не впускает пользователя.
fn invite_join_refusal(tx: &mut dyn Tx, space: &Row, user_id: &str) -> ApiResult<()> {
    let space_id = space.text("id")?;
    if perms::is_banned(tx, &space_id, user_id)? {
        return Err(ApiError::forbidden("user_banned"));
    }
    if perms::is_member(tx, &space_id, user_id)? {
        return Err(ApiError::conflict("already_member"));
    }
    if space.opt_text("visibility")?.as_deref() == Some("request") {
        let pending = tx
            .query_one(
                "SELECT 1 AS one FROM join_requests WHERE space_id = ? AND user_id = ? AND status = 'pending'",
                params![&space_id, user_id],
            )?
            .is_some();
        if pending {
            return Err(ApiError::conflict("join_request_pending"));
        }
        return Err(ApiError::forbidden("join_request_required").detail("spaceId", space_id));
    }
    Ok(())
}

/// Впускает участника: строка членства, подписка на события и `member_joined`.
///
/// # Errors
/// Ошибка базы.
pub fn admit(tx: &mut dyn Tx, out: &mut Outbox, space_id: &str, user_id: &str) -> DbResult<()> {
    let now = now_ms();
    tx.execute(
        "INSERT INTO space_members (space_id, user_id, joined_at) VALUES (?, ?, ?)",
        params![space_id, user_id, now],
    )?;
    out.join_space(user_id, space_id);
    if let Some(user) = tx.query_one(
        &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"),
        params![user_id],
    )? {
        let member = ready::member_view(space_id, &user, None, now, &[], None)?;
        out.to_space(
            space_id,
            &json!({"type": "member_joined", "spaceId": space_id, "member": member}),
        );
    }
    Ok(())
}

fn invite_code_of(body: &Value) -> ApiResult<String> {
    str_field(body, "inviteCode")
        .filter(|code| !code.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ApiError::bad_request("invite_code_required"))
}

/// `POST /api/spaces/:id/join`.
pub async fn join(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let code = invite_code_of(&body)?;
    state
        .run(move |tx, out| {
            let space = require_space(tx, &id)?;
            if space.opt_text("invite_code")?.as_deref() != Some(code.as_str()) {
                return Err(ApiError::bad_request("invite_not_found"));
            }
            invite_join_refusal(tx, &space, &user.id)?;
            admit(tx, out, &id, &user.id)?;
            Ok(Json(space_json(&space)?))
        })
        .await
}

/// `POST /api/spaces/join`.
pub async fn join_by_code(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let code = invite_code_of(&body)?;
    state
        .run(move |tx, out| {
            let space = tx
                .query_one(
                    &format!(
                        "SELECT {} FROM spaces WHERE invite_code = ?",
                        ready::SPACE_COLUMNS
                    ),
                    params![&code],
                )?
                .ok_or_else(|| ApiError::not_found("invite_not_found"))?;
            invite_join_refusal(tx, &space, &user.id)?;
            admit(tx, out, &space.text("id")?, &user.id)?;
            Ok(Json(space_json(&space)?))
        })
        .await
}

/// `GET /api/spaces/invite/:code/preview` (без входа).
pub async fn invite_preview(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            invite_preview_json(tx, &code)?
                .map(Json)
                .ok_or_else(|| ApiError::not_found("invite_not_found"))
        })
        .await
}

/// Что видно по коду приглашения (`getLocalInvitePreview`).
///
/// # Errors
/// Ошибка базы.
pub fn invite_preview_json(tx: &mut dyn Tx, code: &str) -> DbResult<Option<Value>> {
    let Some(space) = tx.query_one(
        &format!(
            "SELECT {} FROM spaces WHERE invite_code = ?",
            ready::SPACE_COLUMNS
        ),
        params![code],
    )?
    else {
        return Ok(None);
    };
    let space_id = space.text("id")?;
    let members = tx
        .query_one(
            "SELECT count(*) AS n FROM space_members WHERE space_id = ?",
            params![&space_id],
        )?
        .map_or(Ok(0), |row| row.int("n"))?;
    let instance_name = tx
        .query_one(
            "SELECT instance_name FROM instance_settings WHERE id = 1",
            params![],
        )?
        .map(|row| row.opt_text("instance_name"))
        .transpose()?
        .flatten()
        .unwrap_or_else(|| "Backspace".to_owned());
    Ok(Some(json!({
        "spaceId": space_id,
        "spaceName": space.text("name")?,
        "description": space.opt_text("description")?,
        "icon": space.opt_text("icon")?,
        "avatarColor": space.opt_text("avatar_color")?,
        "memberCount": members,
        "instanceName": instance_name,
        "visibility": space.opt_text("visibility")?.unwrap_or_else(|| "private".to_owned()),
    })))
}

/// `GET /api/spaces/:id/members`.
pub async fn members(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            require_space(tx, &id)?;
            if !perms::is_member(tx, &id, &user.id)? {
                return Err(ApiError::forbidden("not_space_member"));
            }
            let roles = ready::space_roles(tx, &id)?;
            Ok(Json(json!(ready::space_members(tx, &id, &roles)?)))
        })
        .await
}

/// Участник с ролями, как он уходит клиентам.
fn member_json(tx: &mut dyn Tx, space_id: &str, user_id: &str) -> ApiResult<Value> {
    let failed = || ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "member_update_failed");
    let member = tx
        .query_one(
            "SELECT nickname, joined_at FROM space_members WHERE space_id = ? AND user_id = ?",
            params![space_id, user_id],
        )?
        .ok_or_else(failed)?;
    let user = tx
        .query_one(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"),
            params![user_id],
        )?
        .ok_or_else(|| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "user_not_found"))?;
    let roles = ready::space_roles(tx, space_id)?;
    let assigned = tx
        .query(
            "SELECT role_id FROM member_roles WHERE space_id = ? AND user_id = ?",
            params![space_id, user_id],
        )?
        .iter()
        .map(|row| row.text("role_id"))
        .collect::<DbResult<std::collections::HashSet<_>>>()?;
    Ok(ready::member_view(
        space_id,
        &user,
        member.opt_text("nickname")?,
        member.int("joined_at")?,
        &roles,
        Some(&assigned),
    )?)
}

/// Отказ по правилу «свои биты»: роль с правами `role_bits` нельзя выдать,
/// не имея этих прав.
fn role_grant_refusal(
    tx: &mut dyn Tx,
    space_id: &str,
    actor: &str,
    role_bits: Permissions,
) -> ApiResult<()> {
    let held = perms::compute(tx, actor, space_id, None)?;
    role_bits_change_refusal(held, Permissions::empty(), role_bits)
        .map_or(Ok(()), |refusal| Err(ApiError::forbidden(refusal.code())))
}

fn role_ids_of(value: &Value) -> Option<Vec<String>> {
    let list = value.as_array()?;
    let ids: Vec<String> = list
        .iter()
        .map(|id| id.as_str().map(str::to_owned))
        .collect::<Option<_>>()?;
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    (unique.len() == ids.len()).then_some(ids)
}

/// `PATCH /api/spaces/:id/members/:uid`.
pub async fn update_member(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, uid)): Path<(String, String)>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    if !body.is_object() {
        return Err(ApiError::bad_request("validation_failed"));
    }
    state
        .run(move |tx, out| {
            let space = require_space(tx, &id)?;
            let owner = space.text("owner_id")?;
            let role_ids = field(&body, "roleIds");
            let nickname = field(&body, "nickname");
            if role_ids.is_none() && nickname.is_none() {
                return Err(ApiError::bad_request("validation_failed"));
            }
            let mut new_roles = None;
            if let Some(role_ids) = role_ids {
                if !perms::has_permission(tx, &user.id, &id, Permissions::MANAGE_ROLES)? {
                    return Err(perms::missing("MANAGE_ROLES"));
                }
                if uid == user.id {
                    return Err(ApiError::bad_request("cannot_change_own_roles"));
                }
                if uid == owner {
                    return Err(ApiError::forbidden("space_owner_only"));
                }
                let role_ids = role_ids_of(role_ids)
                    .ok_or_else(|| ApiError::bad_request("role_ids_invalid"))?;
                let space_roles = ready::space_roles(tx, &id)?;
                let mut known = std::collections::HashMap::new();
                for role in &space_roles {
                    known.insert(
                        role.text("id")?,
                        (
                            role.opt_int("position")?.unwrap_or(0),
                            perms::from_db(role.opt_text("permissions")?.as_deref()),
                        ),
                    );
                }
                for role in &role_ids {
                    if !known.contains_key(role) {
                        return Err(ApiError::bad_request("role_not_in_space")
                            .detail("roleId", role.as_str()));
                    }
                    if *role == id {
                        return Err(ApiError::bad_request("everyone_role_not_assignable"));
                    }
                }
                let actor = perms::standing(tx, &id, &user.id)?;
                if !can_act_on_member(actor, perms::standing(tx, &id, &uid)?) {
                    return Err(ApiError::forbidden("role_hierarchy"));
                }
                let current: std::collections::HashSet<String> = tx
                    .query(
                        "SELECT role_id FROM member_roles WHERE space_id = ? AND user_id = ?",
                        params![&id, &uid],
                    )?
                    .iter()
                    .map(|row| row.text("role_id"))
                    .collect::<DbResult<_>>()?;
                let added: Vec<&String> = role_ids
                    .iter()
                    .filter(|role| !current.contains(*role))
                    .collect();
                let removed = current.iter().filter(|role| !role_ids.contains(role));
                for role in added.iter().copied().chain(removed) {
                    let position = known.get(role).map_or(0, |(position, _)| *position);
                    if !can_manage_role_at(actor, position) {
                        return Err(ApiError::forbidden("role_hierarchy"));
                    }
                }
                for role in added {
                    let bits = known
                        .get(role)
                        .map_or_else(Permissions::empty, |(_, bits)| *bits);
                    role_grant_refusal(tx, &id, &user.id, bits)?;
                }
                new_roles = Some(role_ids);
            }
            let mut new_nickname = None;
            if let Some(nickname) = nickname {
                if uid != user.id {
                    if !perms::has_permission(tx, &user.id, &id, Permissions::MANAGE_SPACE)? {
                        return Err(perms::missing("MANAGE_SPACE"));
                    }
                    if uid == owner {
                        return Err(ApiError::forbidden("space_owner_only"));
                    }
                    if !perms::can_act_on(tx, &id, &user.id, &uid)? {
                        return Err(ApiError::forbidden("role_hierarchy"));
                    }
                }
                new_nickname = Some(crate::util::normalize_nickname(nickname).map_err(|()| {
                    ApiError::bad_request("member_nickname_invalid")
                        .detail("max", crate::util::MAX_LABEL_LENGTH)
                })?);
            }
            if !perms::is_member(tx, &id, &uid)? {
                return Err(ApiError::not_found("member_not_found"));
            }
            let roles_changed = new_roles.is_some();
            if let Some(roles) = new_roles {
                tx.execute(
                    "DELETE FROM member_roles WHERE space_id = ? AND user_id = ?",
                    params![&id, &uid],
                )?;
                for role in roles {
                    tx.execute(
                        "INSERT INTO member_roles (space_id, user_id, role_id) VALUES (?, ?, ?)",
                        params![&id, &uid, role],
                    )?;
                }
            }
            if let Some(nickname) = new_nickname {
                tx.execute(
                    "UPDATE space_members SET nickname = ? WHERE space_id = ? AND user_id = ?",
                    params![nickname, &id, &uid],
                )?;
            }
            if roles_changed {
                broadcast::access_changed(out, &id);
                crate::voice::access_changed(tx, out, &id, std::slice::from_ref(&uid))?;
            }
            let member = member_json(tx, &id, &uid)?;
            out.to_space(
                &id,
                &json!({"type": "member_updated", "spaceId": id, "member": member}),
            );
            Ok(Json(member))
        })
        .await
}

/// Убирает участника из пространства вместе с тем, что к нему привязано.
///
/// # Errors
/// Ошибка базы.
pub fn remove_member(tx: &mut dyn Tx, space_id: &str, user_id: &str) -> DbResult<()> {
    tx.execute(
        "DELETE FROM space_members WHERE space_id = ? AND user_id = ?",
        params![space_id, user_id],
    )?;
    tx.execute(
        "DELETE FROM member_roles WHERE space_id = ? AND user_id = ?",
        params![space_id, user_id],
    )?;
    // Оригинал оставляет роли и личные переопределения исключённого, и после
    // возвращения по приглашению тот получает их обратно. Здесь доступ
    // отзывается вместе с членством.
    tx.execute(
        "DELETE FROM channel_overrides WHERE target_type = 'member' AND target_id = ? \
         AND channel_id IN (SELECT id FROM channels WHERE space_id = ?)",
        params![user_id, space_id],
    )?;
    tx.execute(
        "DELETE FROM category_overrides WHERE target_type = 'member' AND target_id = ? \
         AND category_id IN (SELECT id FROM channel_categories WHERE space_id = ?)",
        params![user_id, space_id],
    )?;
    tx.execute(
        "DELETE FROM voice_restrictions WHERE space_id = ? AND user_id = ?",
        params![space_id, user_id],
    )?;
    tx.execute(
        "DELETE FROM read_states WHERE user_id = ? AND channel_id IN \
         (SELECT id FROM channels WHERE space_id = ?)",
        params![user_id, space_id],
    )?;
    Ok(())
}

/// `DELETE /api/spaces/:id/members/:uid` — выйти или исключить.
pub async fn kick(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, uid)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, out| {
            let space = require_space(tx, &id)?;
            let owner = space.text("owner_id")?;
            let is_self = uid == user.id;
            if !is_self && !perms::has_permission(tx, &user.id, &id, Permissions::KICK_MEMBERS)? {
                return Err(perms::missing("KICK_MEMBERS"));
            }
            if is_self && owner == user.id {
                return Err(ApiError::bad_request("space_owner_cannot_leave"));
            }
            if !perms::is_member(tx, &id, &uid)? {
                return Err(ApiError::not_found("member_not_found"));
            }
            if uid == owner {
                return Err(ApiError::bad_request("cannot_target_owner"));
            }
            if !is_self && !perms::can_act_on(tx, &id, &user.id, &uid)? {
                return Err(ApiError::forbidden("role_hierarchy"));
            }
            remove_member(tx, &id, &uid)?;
            out.to_space(
                &id,
                &json!({"type": "member_left", "spaceId": id, "userId": uid}),
            );
            // Ушедший больше не получает событий пространства.
            out.leave_space(&uid, &id);
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// Приводит позиции ролей к плотному порядку `1..n` сверху вниз
/// (`normalizeRolePositions`); `@everyone` всегда на нуле.
fn write_role_order(tx: &mut dyn Tx, space_id: &str, ordered: &[(String, i64)]) -> DbResult<()> {
    let count = i64::try_from(ordered.len()).unwrap_or(i64::MAX);
    for (index, (id, position)) in ordered.iter().enumerate() {
        let wanted = count - i64::try_from(index).unwrap_or(0);
        if *position != wanted {
            tx.execute(
                "UPDATE roles SET position = ? WHERE id = ? AND space_id = ?",
                params![wanted, id, space_id],
            )?;
        }
    }
    tx.execute(
        "UPDATE roles SET position = 0 WHERE id = ? AND space_id = ?",
        params![space_id, space_id],
    )?;
    Ok(())
}

/// Роли пространства без `@everyone`, сверху вниз.
fn ordered_roles(tx: &mut dyn Tx, space_id: &str) -> DbResult<Vec<(String, i64)>> {
    tx.query(
        "SELECT id, position FROM roles WHERE space_id = ? AND id <> space_id \
         ORDER BY position DESC, created_at ASC, id ASC",
        params![space_id],
    )?
    .iter()
    .map(|row| Ok((row.text("id")?, row.opt_int("position")?.unwrap_or(0))))
    .collect()
}

fn normalize_role_positions(tx: &mut dyn Tx, space_id: &str) -> DbResult<()> {
    let ordered = ordered_roles(tx, space_id)?;
    write_role_order(tx, space_id, &ordered)
}

fn move_role(tx: &mut dyn Tx, space_id: &str, role_id: &str, position: i64) -> DbResult<()> {
    let mut ordered = ordered_roles(tx, space_id)?;
    let Some(from) = ordered.iter().position(|(id, _)| id == role_id) else {
        return Ok(());
    };
    let moving = ordered.remove(from);
    let rest = i64::try_from(ordered.len()).unwrap_or(i64::MAX);
    let clamped = position.clamp(1, rest + 1);
    let at = usize::try_from(rest + 1 - clamped).unwrap_or(0);
    ordered.insert(at.min(ordered.len()), moving);
    write_role_order(tx, space_id, &ordered)
}

/// Позиция роли, если поставить её рядом с `anchor` (`positionNextTo`).
fn position_next_to(
    tx: &mut dyn Tx,
    space_id: &str,
    role_id: &str,
    anchor: &str,
    above: bool,
) -> DbResult<Option<i64>> {
    if anchor == role_id {
        return Ok(None);
    }
    let others: Vec<(String, i64)> = ordered_roles(tx, space_id)?
        .into_iter()
        .filter(|(id, _)| id != role_id)
        .collect();
    let insert_at = if anchor == space_id {
        if !above {
            return Ok(None);
        }
        others.len()
    } else {
        match others.iter().position(|(id, _)| id == anchor) {
            Some(index) if above => index,
            Some(index) => index + 1,
            None => return Ok(None),
        }
    };
    Ok(Some(
        i64::try_from(others.len() + 1 - insert_at).unwrap_or(1),
    ))
}

fn role_row(tx: &mut dyn Tx, space_id: &str, role_id: &str) -> DbResult<Option<Row>> {
    tx.query_one(
        "SELECT id, space_id, name, color, position, permissions, created_at FROM roles \
         WHERE id = ? AND space_id = ?",
        params![role_id, space_id],
    )
}

fn role_name_taken(
    tx: &mut dyn Tx,
    space_id: &str,
    name: &str,
    except: Option<&str>,
) -> DbResult<bool> {
    // Как `COLLATE NOCASE` в SQLite: без учёта регистра только для ASCII.
    let wanted = name.to_ascii_lowercase();
    for row in tx.query(
        "SELECT id, name FROM roles WHERE space_id = ?",
        params![space_id],
    )? {
        if Some(row.text("id")?.as_str()) != except
            && row.text("name")?.to_ascii_lowercase() == wanted
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn role_response(tx: &mut dyn Tx, space_id: &str, viewer: &str, role: &Row) -> ApiResult<Value> {
    let viewer_perms = perms::compute(tx, viewer, space_id, None)?;
    Ok(ready::role_view(
        role,
        ready::reads_permission_data(viewer_perms),
    )?)
}

/// Участники, чьи права задевает изменение роли.
fn role_holders(tx: &mut dyn Tx, space_id: &str, role_id: &str) -> DbResult<Vec<String>> {
    let rows = if role_id == space_id {
        tx.query(
            "SELECT user_id FROM space_members WHERE space_id = ?",
            params![space_id],
        )?
    } else {
        tx.query(
            "SELECT user_id FROM member_roles WHERE space_id = ? AND role_id = ?",
            params![space_id, role_id],
        )?
    };
    rows.iter().map(|row| row.text("user_id")).collect()
}

pub(crate) fn parse_bits(value: &Value) -> ApiResult<Permissions> {
    value
        .as_str()
        .filter(|text| {
            *text == "0"
                || (!text.starts_with('0')
                    && !text.is_empty()
                    && text.bytes().all(|b| b.is_ascii_digit()))
        })
        .and_then(|text| text.parse::<u128>().ok())
        .map(|bits| {
            Permissions::from_bits_retain(u64::try_from(bits & u128::from(u64::MAX)).unwrap_or(0))
        })
        .ok_or_else(|| ApiError::bad_request("permissions_invalid"))
}

/// `POST /api/spaces/:id/roles`.
pub async fn create_role(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let ids = state.ids.clone();
    state
        .run(move |tx, out| {
            if !perms::has_permission(tx, &user.id, &id, Permissions::MANAGE_ROLES)? {
                return Err(perms::missing("MANAGE_ROLES"));
            }
            let held = perms::compute(tx, &user.id, &id, None)?;
            let bits = match field(&body, "permissions").filter(|value| !value.is_null()) {
                Some(value) => {
                    let requested = parse_bits(value)?;
                    if let Some(refusal) =
                        role_bits_change_refusal(held, Permissions::empty(), requested)
                    {
                        return Err(ApiError::forbidden(refusal.code()));
                    }
                    requested
                }
                None => DEFAULT_EVERYONE_PERMISSIONS & held,
            };
            let name = str_field(&body, "name")
                .map(js_trim)
                .filter(|name| !name.is_empty())
                .unwrap_or("new role");
            if role_name_taken(tx, &id, name, None)? {
                return Err(ApiError::conflict("role_name_taken"));
            }
            if !can_manage_role_at(perms::standing(tx, &id, &user.id)?, 1) {
                return Err(ApiError::forbidden("role_hierarchy"));
            }
            let role_id = ids.next();
            let color = str_field(&body, "color")
                .filter(|color| !color.is_empty())
                .unwrap_or("#b9bbbe");
            tx.execute(
                "INSERT INTO roles (id, space_id, name, color, position, permissions, created_at) \
                 VALUES (?, ?, ?, ?, 0, ?, ?)",
                params![&role_id, &id, name, color, perms::to_wire(bits), now_ms()],
            )?;
            normalize_role_positions(tx, &id)?;
            let role = role_row(tx, &id, &role_id)?.ok_or_else(ApiError::internal)?;
            broadcast::access_changed(out, &id);
            crate::voice::access_changed(tx, out, &id, &[])?;
            Ok((
                StatusCode::CREATED,
                Json(role_response(tx, &id, &user.id, &role)?),
            ))
        })
        .await
}

/// `PATCH /api/spaces/:id/roles/:roleId`.
#[allow(clippy::too_many_lines, reason = "проверки идут в порядке оригинала")]
pub async fn update_role(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, role_id)): Path<(String, String)>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, out| {
            if !perms::has_permission(tx, &user.id, &id, Permissions::MANAGE_ROLES)? {
                return Err(perms::missing("MANAGE_ROLES"));
            }
            let missing_role =
                || ApiError::not_found("role_not_in_space").detail("roleId", role_id.as_str());
            let role = role_row(tx, &id, &role_id)?.ok_or_else(missing_role)?;
            let actor = perms::standing(tx, &id, &user.id)?;
            if !can_manage_role_at(actor, role.opt_int("position")?.unwrap_or(0)) {
                return Err(ApiError::forbidden("role_hierarchy"));
            }
            let invalid = || ApiError::bad_request("validation_failed");
            let above = field(&body, "above");
            let below = field(&body, "below");
            let mut position = match field(&body, "position") {
                None => None,
                Some(value) => Some(value.as_i64().ok_or_else(invalid)?),
            };
            if above.is_some() || below.is_some() {
                let anchor = above.or(below).and_then(Value::as_str);
                let Some(anchor) =
                    anchor.filter(|_| role_id != id && !(above.is_some() && below.is_some()))
                else {
                    return Err(invalid());
                };
                if role_row(tx, &id, anchor)?.is_none() {
                    return Err(ApiError::bad_request("role_not_in_space").detail("roleId", anchor));
                }
                position = Some(
                    position_next_to(tx, &id, &role_id, anchor, above.is_some())?
                        .ok_or_else(invalid)?,
                );
            }
            if let Some(position) = position {
                if role_id == id || position < 1 {
                    return Err(invalid());
                }
                if !can_manage_role_at(actor, position) {
                    return Err(ApiError::forbidden("role_hierarchy"));
                }
            }
            let mut changed = false;
            if let Some(name) = field(&body, "name") {
                let name = js_trim(name.as_str().ok_or_else(invalid)?);
                if name.is_empty() {
                    return Err(ApiError::bad_request("role_name_required"));
                }
                if role_name_taken(tx, &id, name, Some(&role_id))? {
                    return Err(ApiError::conflict("role_name_taken"));
                }
                tx.execute(
                    "UPDATE roles SET name = ? WHERE id = ? AND space_id = ?",
                    params![name, &role_id, &id],
                )?;
                changed = true;
            }
            if let Some(color) = field(&body, "color") {
                tx.execute(
                    "UPDATE roles SET color = ? WHERE id = ? AND space_id = ?",
                    params![color.as_str(), &role_id, &id],
                )?;
                changed = true;
            }
            if let Some(value) = field(&body, "permissions") {
                let requested = parse_bits(value)?;
                let before = perms::from_db(role.opt_text("permissions")?.as_deref());
                if let Some(version) = field(&body, "permissionsVersion") {
                    let version = version
                        .as_str()
                        .filter(|text| !text.is_empty())
                        .ok_or_else(invalid)?;
                    if permissions_version(before) != version {
                        return Err(ApiError::conflict("role_permissions_conflict"));
                    }
                }
                let held = perms::compute(tx, &user.id, &id, None)?;
                if let Some(refusal) = role_bits_change_refusal(held, before, requested) {
                    return Err(ApiError::forbidden(refusal.code()));
                }
                tx.execute(
                    "UPDATE roles SET permissions = ? WHERE id = ? AND space_id = ?",
                    params![perms::to_wire(requested), &role_id, &id],
                )?;
                changed = true;
            }
            if !changed && position.is_none() {
                return Err(ApiError::bad_request("no_fields_to_update"));
            }
            if let Some(position) = position {
                move_role(tx, &id, &role_id, position)?;
            }
            let updated = role_row(tx, &id, &role_id)?.ok_or_else(missing_role)?;
            broadcast::access_changed(out, &id);
            let holders = role_holders(tx, &id, &role_id)?;
            crate::voice::access_changed(tx, out, &id, &holders)?;
            Ok(Json(role_response(tx, &id, &user.id, &updated)?))
        })
        .await
}

/// `DELETE /api/spaces/:id/roles/:roleId`.
pub async fn delete_role(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, role_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, out| {
            if !perms::has_permission(tx, &user.id, &id, Permissions::MANAGE_ROLES)? {
                return Err(perms::missing("MANAGE_ROLES"));
            }
            if role_id == id {
                return Err(ApiError::bad_request("everyone_role_not_deletable"));
            }
            let role = role_row(tx, &id, &role_id)?.ok_or_else(|| {
                ApiError::not_found("role_not_in_space").detail("roleId", role_id.as_str())
            })?;
            if !can_manage_role_at(
                perms::standing(tx, &id, &user.id)?,
                role.opt_int("position")?.unwrap_or(0),
            ) {
                return Err(ApiError::forbidden("role_hierarchy"));
            }
            let held = perms::compute(tx, &user.id, &id, None)?;
            let bits = perms::from_db(role.opt_text("permissions")?.as_deref());
            if let Some(refusal) = role_bits_change_refusal(held, bits, Permissions::empty()) {
                return Err(ApiError::forbidden(refusal.code()));
            }
            let holders = role_holders(tx, &id, &role_id)?;
            // Переопределения ищутся только в каналах и категориях этого
            // пространства: чужой идентификатор роли ничего не заденет.
            tx.execute(
                "DELETE FROM channel_overrides WHERE target_type = 'role' AND target_id = ?",
                params![&role_id],
            )?;
            tx.execute(
                "DELETE FROM category_overrides WHERE target_type = 'role' AND target_id = ?",
                params![&role_id],
            )?;
            tx.execute(
                "DELETE FROM member_roles WHERE space_id = ? AND role_id = ?",
                params![&id, &role_id],
            )?;
            tx.execute(
                "DELETE FROM roles WHERE id = ? AND space_id = ?",
                params![&role_id, &id],
            )?;
            normalize_role_positions(tx, &id)?;
            broadcast::access_changed(out, &id);
            crate::voice::access_changed(tx, out, &id, &holders)?;
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// Общие проверки для выдачи и снятия одной роли (`checkSingleRoleChange`).
fn check_single_role_change(
    tx: &mut dyn Tx,
    space_id: &str,
    actor_id: &str,
    target_id: &str,
    role_id: Option<&str>,
    adding: bool,
) -> ApiResult<()> {
    if !perms::has_permission(tx, actor_id, space_id, Permissions::MANAGE_ROLES)? {
        return Err(perms::missing("MANAGE_ROLES"));
    }
    if target_id == actor_id {
        return Err(ApiError::bad_request("cannot_change_own_roles"));
    }
    if perms::is_owner(tx, space_id, target_id)? {
        return Err(ApiError::forbidden("space_owner_only"));
    }
    if !perms::is_member(tx, space_id, target_id)? {
        return Err(ApiError::not_found("member_not_found"));
    }
    let role = match role_id {
        Some(role_id) => role_row(tx, space_id, role_id)?,
        None => None,
    };
    let Some(role) = role else {
        return Err(ApiError::bad_request("role_not_in_space")
            .detail("roleId", role_id.unwrap_or("undefined")));
    };
    if role.text("id")? == space_id {
        return Err(ApiError::bad_request("everyone_role_not_assignable"));
    }
    let actor = perms::standing(tx, space_id, actor_id)?;
    if !can_act_on_member(actor, perms::standing(tx, space_id, target_id)?)
        || !can_manage_role_at(actor, role.opt_int("position")?.unwrap_or(0))
    {
        return Err(ApiError::forbidden("role_hierarchy"));
    }
    if adding {
        let bits = perms::from_db(role.opt_text("permissions")?.as_deref());
        role_grant_refusal(tx, space_id, actor_id, bits)?;
    }
    Ok(())
}

/// `POST /api/spaces/:id/members/:uid/roles`.
pub async fn add_member_role(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, uid)): Path<(String, String)>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, out| {
            let role_id = str_field(&body, "roleId");
            check_single_role_change(tx, &id, &user.id, &uid, role_id, true)?;
            tx.execute(
                "INSERT INTO member_roles (space_id, user_id, role_id) VALUES (?, ?, ?) \
                 ON CONFLICT DO NOTHING",
                params![&id, &uid, role_id],
            )?;
            broadcast::access_changed(out, &id);
            crate::voice::access_changed(tx, out, &id, std::slice::from_ref(&uid))?;
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `DELETE /api/spaces/:id/members/:uid/roles/:roleId`.
pub async fn remove_member_role(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, uid, role_id)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, out| {
            check_single_role_change(tx, &id, &user.id, &uid, Some(&role_id), false)?;
            tx.execute(
                "DELETE FROM member_roles WHERE space_id = ? AND user_id = ? AND role_id = ?",
                params![&id, &uid, &role_id],
            )?;
            broadcast::access_changed(out, &id);
            crate::voice::access_changed(tx, out, &id, std::slice::from_ref(&uid))?;
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `PATCH /api/spaces/:id/transfer-ownership`.
pub async fn transfer_ownership(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let new_owner = str_field(&body, "newOwnerId")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ApiError::bad_request("new_owner_required"))?
        .to_owned();
    state
        .run(move |tx, out| {
            let space = require_space(tx, &id)?;
            if space.text("owner_id")? != user.id {
                return Err(ApiError::forbidden("space_owner_only"));
            }
            if new_owner == user.id {
                return Err(ApiError::bad_request("already_owner"));
            }
            if !perms::is_member(tx, &id, &new_owner)? {
                return Err(ApiError::bad_request("new_owner_not_member"));
            }
            tx.execute(
                "UPDATE spaces SET owner_id = ?, owner_title = NULL WHERE id = ?",
                params![&new_owner, &id],
            )?;
            let updated = space_row(tx, &id)?.ok_or_else(|| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "ownership_transfer_failed",
                )
            })?;
            let data = space_json(&updated)?;
            out.to_space(&id, &json!({"type": "space_updated", "space": data}));
            broadcast::access_changed(out, &id);
            crate::voice::access_changed(tx, out, &id, &[user.id.clone(), new_owner])?;
            Ok(Json(data))
        })
        .await
}

/// `GET /api/spaces/:id/bans`.
pub async fn bans(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            if !perms::has_permission(tx, &user.id, &id, Permissions::BAN_MEMBERS)? {
                return Err(perms::missing("BAN_MEMBERS"));
            }
            let rows = tx.query(
                "SELECT space_id, user_id, reason, banned_by, created_at FROM bans WHERE space_id = ?",
                params![&id],
            )?;
            let mut bans = Vec::with_capacity(rows.len());
            for row in rows {
                let user_id = row.text("user_id")?;
                let banned_by = row.opt_text("banned_by")?;
                let person = |tx: &mut dyn Tx, id: Option<&str>| -> DbResult<Value> {
                    let Some(id) = id else { return Ok(Value::Null) };
                    crate::routes::users::user_row(tx, id)?
                        .map_or(Ok(Value::Null), |user| sanitize_user(&user, false))
                };
                bans.push(json!({
                    "spaceId": row.text("space_id")?,
                    "user": person(tx, Some(&user_id))?,
                    "moderator": person(tx, banned_by.as_deref())?,
                    "userId": user_id,
                    "reason": row.opt_text("reason")?,
                    "bannedBy": banned_by,
                    "createdAt": row.int("created_at")?,
                }));
            }
            Ok(Json(json!(bans)))
        })
        .await
}

/// `POST /api/spaces/:id/bans`.
pub async fn ban(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let target = str_field(&body, "userId")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ApiError::bad_request("user_id_required"))?
        .to_owned();
    let reason = str_field(&body, "reason")
        .map(js_trim)
        .filter(|reason| !reason.is_empty())
        .map(str::to_owned);
    state
        .run(move |tx, out| {
            if !perms::has_permission(tx, &user.id, &id, Permissions::BAN_MEMBERS)? {
                return Err(perms::missing("BAN_MEMBERS"));
            }
            if perms::is_owner(tx, &id, &target)? {
                return Err(ApiError::bad_request("cannot_target_owner"));
            }
            if target == user.id {
                return Err(ApiError::bad_request("cannot_target_self"));
            }
            if !perms::can_act_on(tx, &id, &user.id, &target)? {
                return Err(ApiError::forbidden("role_hierarchy"));
            }
            if perms::is_banned(tx, &id, &target)? {
                return Err(ApiError::conflict("already_banned"));
            }
            tx.execute(
                "INSERT INTO bans (space_id, user_id, reason, banned_by, created_at) VALUES (?, ?, ?, ?, ?)",
                params![&id, &target, &reason, &user.id, now_ms()],
            )?;
            remove_member(tx, &id, &target)?;
            out.to_space(&id, &json!({"type": "member_left", "spaceId": id, "userId": target}));
            out.to_user(&target, &json!({"type": "member_banned", "spaceId": id, "reason": reason}));
            out.leave_space(&target, &id);
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `DELETE /api/spaces/:id/bans/:uid`.
pub async fn unban(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, uid)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            if !perms::has_permission(tx, &user.id, &id, Permissions::BAN_MEMBERS)? {
                return Err(perms::missing("BAN_MEMBERS"));
            }
            let removed = tx.execute(
                "DELETE FROM bans WHERE space_id = ? AND user_id = ?",
                params![&id, &uid],
            )?;
            if removed == 0 {
                return Err(ApiError::not_found("ban_not_found"));
            }
            Ok(Json(json!({"success": true})))
        })
        .await
}
