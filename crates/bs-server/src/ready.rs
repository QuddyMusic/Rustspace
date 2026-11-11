//! Начальный снимок состояния для клиента (`buildReadyPayload`).

use std::collections::{HashMap, HashSet};

use bs_permissions::Permissions;
use serde_json::{Map, Value, json};

use crate::db::{DbError, DbResult, Row, Tx};
use crate::hub::Hub;
use crate::params;
use crate::perms;
use crate::routes::users::{USER_COLUMNS, sanitize_user, user_columns};

/// Счётчик непрочитанных не идёт дальше этого: значок показывает «99+».
pub const UNREAD_COUNT_CAP: i64 = 100;

/// Роль, как её видит клиент (`roleView`): биты — только тем, кто управляет ролями.
///
/// # Errors
/// В строке нет нужных столбцов.
pub fn role_view(row: &Row, with_permissions: bool) -> DbResult<Value> {
    let id = row.text("id")?;
    let space = row.text("space_id")?;
    let mut role = json!({
        "id": id,
        "spaceId": space,
        "name": row.text("name")?,
        "color": row.opt_text("color")?.unwrap_or_else(|| "#b9bbbe".to_owned()),
        "position": row.opt_int("position")?.unwrap_or(0),
        "isEveryone": id == space,
        "createdAt": row.int("created_at")?,
    });
    if with_permissions {
        role["permissions"] = json!(
            row.opt_text("permissions")?
                .unwrap_or_else(|| "0".to_owned())
        );
    }
    Ok(role)
}

/// Получает ли зритель биты ролей и переопределения.
#[must_use]
pub fn reads_permission_data(space_perms: Permissions) -> bool {
    perms::has(space_perms, Permissions::MANAGE_ROLES)
}

/// Роли пространства по возрастанию позиции.
///
/// # Errors
/// Ошибка базы.
pub fn space_roles(tx: &mut dyn Tx, space_id: &str) -> DbResult<Vec<Row>> {
    tx.query(
        "SELECT id, space_id, name, color, position, permissions, created_at FROM roles \
         WHERE space_id = ? ORDER BY position",
        params![space_id],
    )
}

/// Участники пространства с пользователями и ролями (`MemberWithUser`).
///
/// # Errors
/// Ошибка базы.
pub fn space_members(tx: &mut dyn Tx, space_id: &str, roles: &[Row]) -> DbResult<Vec<Value>> {
    let assigned = tx.query(
        "SELECT user_id, role_id FROM member_roles WHERE space_id = ?",
        params![space_id],
    )?;
    let mut by_user: HashMap<String, HashSet<String>> = HashMap::new();
    for row in &assigned {
        by_user
            .entry(row.text("user_id")?)
            .or_default()
            .insert(row.text("role_id")?);
    }
    let rows = tx.query(
        &format!(
            "SELECT m.nickname AS m_nickname, m.joined_at AS m_joined_at, {} \
             FROM space_members m JOIN users u ON u.id = m.user_id WHERE m.space_id = ?",
            user_columns("u")
        ),
        params![space_id],
    )?;
    let mut members = Vec::with_capacity(rows.len());
    for row in &rows {
        let user_id = row.text("id")?;
        members.push(member_view(
            space_id,
            row,
            row.opt_text("m_nickname")?,
            row.int("m_joined_at")?,
            roles,
            by_user.get(&user_id),
        )?);
    }
    Ok(members)
}

/// Один участник: `user` — строка пользователя, `assigned` — его роли.
///
/// # Errors
/// В строках нет нужных столбцов.
pub fn member_view(
    space_id: &str,
    user: &Row,
    nickname: Option<String>,
    joined_at: i64,
    roles: &[Row],
    assigned: Option<&HashSet<String>>,
) -> DbResult<Value> {
    let mut member_roles = Vec::new();
    if let Some(assigned) = assigned {
        for role in roles {
            if assigned.contains(&role.text("id")?) {
                member_roles.push(role_view(role, false)?);
            }
        }
    }
    Ok(json!({
        "spaceId": space_id,
        "userId": user.text("id")?,
        "nickname": nickname,
        "joinedAt": joined_at,
        "user": sanitize_user(user, false)?,
        "roles": member_roles,
    }))
}

/// Каналы или категории пространства, скрытые от `@everyone`
/// (`idsHiddenFromEveryone`): запрет на роли-пространстве содержит просмотр.
///
/// # Errors
/// Ошибка базы.
pub fn hidden_ids(tx: &mut dyn Tx, space_id: &str, categories: bool) -> DbResult<HashSet<String>> {
    let sql = if categories {
        "SELECT o.category_id AS id, o.allow, o.deny FROM category_overrides o \
         JOIN channel_categories c ON c.id = o.category_id \
         WHERE c.space_id = ? AND o.target_type = 'role' AND o.target_id = ?"
    } else {
        "SELECT o.channel_id AS id, o.allow, o.deny FROM channel_overrides o \
         JOIN channels c ON c.id = o.channel_id \
         WHERE c.space_id = ? AND o.target_type = 'role' AND o.target_id = ?"
    };
    let mut hidden = HashSet::new();
    for row in tx.query(sql, params![space_id, space_id])? {
        let deny = perms::from_db(row.opt_text("deny")?.as_deref());
        if deny.contains(Permissions::VIEW_CHANNEL) {
            hidden.insert(row.text("id")?);
        }
    }
    Ok(hidden)
}

/// Категория на проводе.
///
/// # Errors
/// В строке нет нужных столбцов.
pub fn category_view(row: &Row, is_private: bool) -> DbResult<Value> {
    Ok(json!({
        "id": row.text("id")?,
        "spaceId": row.text("space_id")?,
        "name": row.text("name")?,
        "position": row.opt_number("position")?.unwrap_or_else(|| json!(0)),
        "isPrivate": is_private,
        "createdAt": row.int("created_at")?,
    }))
}

/// Категория без признака приватности (`rowToCategory`).
///
/// # Errors
/// В строке нет нужных столбцов.
pub fn category_base(row: &Row) -> DbResult<Value> {
    Ok(json!({
        "id": row.text("id")?,
        "spaceId": row.text("space_id")?,
        "name": row.text("name")?,
        "position": row.opt_number("position")?.unwrap_or_else(|| json!(0)),
        "createdAt": row.int("created_at")?,
    }))
}

/// Столбцы канала.
pub const CHANNEL_COLUMNS: &str =
    "id, space_id, name, type, topic, position, category_id, created_at";

/// Канал без полей зрителя (`rowToChannel`).
///
/// # Errors
/// В строке нет нужных столбцов.
pub fn channel_base(row: &Row) -> DbResult<Value> {
    Ok(json!({
        "id": row.text("id")?,
        "spaceId": row.text("space_id")?,
        "name": row.text("name")?,
        "type": row.text("type")?,
        "topic": row.opt_text("topic")?,
        "position": row.opt_number("position")?.unwrap_or_else(|| json!(0)),
        "categoryId": row.opt_text("category_id")?,
        "createdAt": row.int("created_at")?,
    }))
}

/// Канал, как его видит обладатель прав `channel_perms`.
///
/// # Errors
/// В строке нет нужных столбцов.
pub fn channel_for_viewer(
    row: &Row,
    is_private: bool,
    channel_perms: Permissions,
) -> DbResult<Value> {
    let mut channel = channel_base(row)?;
    channel["isPrivate"] = json!(is_private);
    channel["myPermissions"] = json!(perms::to_wire(channel_perms));
    Ok(channel)
}

/// Канал в `ready`: ещё и с последним сообщением.
///
/// # Errors
/// В строке нет нужных столбцов.
pub fn channel_view(
    row: &Row,
    is_private: bool,
    last_message_id: Option<String>,
    channel_perms: Permissions,
) -> DbResult<Value> {
    let mut channel = channel_for_viewer(row, is_private, channel_perms)?;
    channel["lastMessageId"] = json!(last_message_id);
    Ok(channel)
}

/// Последнее написанное участником сообщение канала.
///
/// # Errors
/// Ошибка базы.
pub fn last_message_id(tx: &mut dyn Tx, channel_id: &str) -> DbResult<Option<String>> {
    tx.query_one(
        "SELECT MAX(id) AS last_id FROM messages WHERE channel_id = ? AND type = 'user'",
        params![channel_id],
    )?
    .map_or(Ok(None), |row| row.opt_text("last_id"))
}

/// Непрочитанные в канале пространства после `cursor`, не свои, до предела.
///
/// # Errors
/// Ошибка базы.
pub fn count_channel_unread(
    tx: &mut dyn Tx,
    user_id: &str,
    channel_id: &str,
    cursor: &str,
) -> DbResult<i64> {
    tx.query_one(
        "SELECT count(*) AS n FROM (SELECT 1 AS one FROM messages WHERE channel_id = ? \
         AND CAST(id AS INTEGER) > CAST(? AS INTEGER) AND user_id <> ? AND type = 'user' \
         LIMIT 100) AS unread",
        params![channel_id, cursor, user_id],
    )?
    .map_or(Ok(0), |row| row.int("n"))
}

/// Непрочитанные в личной переписке после `cursor`, не свои, до предела.
///
/// # Errors
/// Ошибка базы.
pub fn count_dm_unread(
    tx: &mut dyn Tx,
    user_id: &str,
    dm_channel_id: &str,
    cursor: &str,
) -> DbResult<i64> {
    tx.query_one(
        "SELECT count(*) AS n FROM (SELECT 1 AS one FROM dm_messages WHERE dm_channel_id = ? \
         AND CAST(id AS INTEGER) > CAST(? AS INTEGER) AND user_id <> ? LIMIT 100) AS unread",
        params![dm_channel_id, cursor, user_id],
    )?
    .map_or(Ok(0), |row| row.int("n"))
}

/// Позиция чтения пользователя в канале (`'0'`, если её нет).
///
/// # Errors
/// Ошибка базы.
pub fn read_cursor(tx: &mut dyn Tx, user_id: &str, channel_id: &str) -> DbResult<String> {
    Ok(tx
        .query_one(
            "SELECT last_read_message_id FROM read_states WHERE user_id = ? AND channel_id = ?",
            params![user_id, channel_id],
        )?
        .map(|row| row.text("last_read_message_id"))
        .transpose()?
        .unwrap_or_else(|| "0".to_owned()))
}

/// Пространство целиком, как его получает `viewer` (в `ready`, при
/// вступлении, в ответах REST). Вторым значением — каналы, где считаются
/// непрочитанные.
///
/// # Errors
/// Ошибка базы.
pub fn space_view(tx: &mut dyn Tx, space: &Row, viewer: &str) -> DbResult<(Value, Vec<String>)> {
    let space_id = space.text("id")?;
    let roles = space_roles(tx, &space_id)?;
    let members = space_members(tx, &space_id, &roles)?;
    let space_perms = perms::compute(tx, viewer, &space_id, None)?;
    let private_channels = hidden_ids(tx, &space_id, false)?;
    let private_categories = hidden_ids(tx, &space_id, true)?;

    let mut categories = Vec::new();
    for row in tx.query(
        "SELECT id, space_id, name, position, created_at FROM channel_categories WHERE space_id = ?",
        params![&space_id],
    )? {
        let private = private_categories.contains(&row.text("id")?);
        categories.push(category_view(&row, private)?);
    }

    let mut channels = Vec::new();
    let mut countable = Vec::new();
    for row in tx.query(
        &format!("SELECT {CHANNEL_COLUMNS} FROM channels WHERE space_id = ?"),
        params![&space_id],
    )? {
        let id = row.text("id")?;
        let channel_perms = perms::compute(tx, viewer, &space_id, Some(&id))?;
        if !perms::can_view(channel_perms) {
            continue;
        }
        if perms::can_count_unread(channel_perms) {
            countable.push(id.clone());
        }
        let last = last_message_id(tx, &id)?;
        channels.push(channel_view(
            &row,
            private_channels.contains(&id),
            last,
            channel_perms,
        )?);
    }

    let with_bits = reads_permission_data(space_perms);
    let role_views = roles
        .iter()
        .map(|row| role_view(row, with_bits))
        .collect::<DbResult<Vec<_>>>()?;
    let view = json!({
        "id": space_id,
        "name": space.text("name")?,
        "icon": space.opt_text("icon")?,
        "banner": space.opt_text("banner")?,
        "avatarColor": space.opt_text("avatar_color")?,
        "ownerId": space.text("owner_id")?,
        "ownerTitle": space.opt_text("owner_title")?,
        "inviteCode": space.opt_text("invite_code")?,
        "visibility": space.opt_text("visibility")?.unwrap_or_else(|| "private".to_owned()),
        "directoryListed": space.opt_flag("directory_listed")? == Some(true),
        "description": space.opt_text("description")?,
        "createdAt": space.int("created_at")?,
        "channels": channels,
        "categories": categories,
        "members": members,
        "roles": role_views,
        "myPermissions": perms::to_wire(space_perms),
    });
    Ok((view, countable))
}

/// Столбцы пространства.
pub const SPACE_COLUMNS: &str = "id, name, icon, banner, avatar_color, owner_id, invite_code, \
    visibility, description, created_at, directory_listed, owner_title";

/// Личная переписка на проводе (`toDmChannelWire`).
///
/// # Errors
/// Ошибка базы.
pub fn dm_channel_view(tx: &mut dyn Tx, channel: &Row) -> DbResult<Value> {
    let id = channel.text("id")?;
    let members = tx
        .query(
            &format!(
                "SELECT {} FROM dm_members m JOIN users u ON u.id = m.user_id \
                 WHERE m.dm_channel_id = ?",
                user_columns("u")
            ),
            params![&id],
        )?
        .iter()
        .map(|row| sanitize_user(row, false))
        .collect::<DbResult<Vec<_>>>()?;
    let last = tx.query_one(
        "SELECT id, dm_channel_id, user_id, content, created_at, type FROM dm_messages \
         WHERE dm_channel_id = ? ORDER BY created_at DESC LIMIT 1",
        params![&id],
    )?;
    let last_message = match last {
        Some(message) => {
            let message_id = message.text("id")?;
            let attachments = tx
                .query(
                    "SELECT mimetype, original_name FROM attachments WHERE dm_message_id = ?",
                    params![&message_id],
                )?
                .iter()
                .map(|row| {
                    Ok(json!({"type": row.text("mimetype")?, "filename": row.text("original_name")?}))
                })
                .collect::<DbResult<Vec<_>>>()?;
            let kind = if message.opt_text("type")?.as_deref() == Some("system") {
                "system"
            } else {
                "user"
            };
            json!({
                "id": message_id,
                "dmChannelId": message.text("dm_channel_id")?,
                "userId": message.text("user_id")?,
                "content": message.opt_text("content")?,
                "createdAt": message.int("created_at")?,
                "type": kind,
                "attachments": attachments,
            })
        }
        None => Value::Null,
    };
    Ok(json!({
        "id": id,
        "federatedId": channel.opt_text("federated_id")?,
        "ownerId": channel.opt_text("owner_id")?,
        "ownerHomeUserId": channel.opt_text("owner_home_user_id")?,
        "ownerHomeInstance": channel.opt_text("owner_home_instance")?,
        "createdAt": channel.int("created_at")?,
        "name": channel.opt_text("name")?,
        "icon": channel.opt_text("icon")?,
        "metadataUpdatedAt": channel.opt_int("metadata_updated_at")?.unwrap_or(0),
        "members": members,
        "lastMessage": last_message,
    }))
}

/// Столбцы личной переписки.
pub const DM_CHANNEL_COLUMNS: &str = "id, owner_id, federated_id, owner_home_user_id, \
    owner_home_instance, deleted_at, created_at, name, icon, metadata_updated_at";

/// Открытые переписки пользователя (`loadOpenDmChannels`).
///
/// # Errors
/// Ошибка базы.
pub fn open_dm_channels(tx: &mut dyn Tx, user_id: &str) -> DbResult<Vec<Value>> {
    let rows = tx.query(
        "SELECT c.id, c.owner_id, c.federated_id, c.owner_home_user_id, c.owner_home_instance, \
         c.deleted_at, c.created_at, c.name, c.icon, c.metadata_updated_at \
         FROM dm_members m JOIN dm_channels c ON c.id = m.dm_channel_id \
         WHERE m.user_id = ? AND m.closed = 0 AND c.deleted_at IS NULL",
        params![user_id],
    )?;
    rows.iter().map(|row| dm_channel_view(tx, row)).collect()
}

/// Папки пространств пользователя.
///
/// # Errors
/// Ошибка базы.
pub fn folders(tx: &mut dyn Tx, user_id: &str) -> DbResult<Vec<Value>> {
    let rows = tx.query(
        "SELECT id, user_id, name, color, position FROM space_folders WHERE user_id = ? \
         ORDER BY position",
        params![user_id],
    )?;
    let mut folders = Vec::with_capacity(rows.len());
    for row in rows {
        let id = row.text("id")?;
        let spaces = tx
            .query(
                "SELECT space_id FROM space_folder_members WHERE folder_id = ? ORDER BY position",
                params![&id],
            )?
            .iter()
            .map(|member| member.text("space_id"))
            .collect::<DbResult<Vec<_>>>()?;
        folders.push(json!({
            "id": id,
            "userId": row.text("user_id")?,
            "name": row.text("name")?,
            "color": row.opt_text("color")?,
            "position": row.opt_int("position")?.unwrap_or(0),
            "spaceIds": spaces,
        }));
    }
    Ok(folders)
}

/// Федеративная личность строки пользователя: пара `null` для своего.
#[must_use]
pub fn presence_identity(home_user_id: Option<String>, home_instance: Option<String>) -> Value {
    match home_instance.filter(|instance| !instance.is_empty()) {
        Some(instance) => json!({"homeUserId": home_user_id, "homeInstance": instance}),
        None => json!({"homeUserId": null, "homeInstance": null}),
    }
}

/// Активность для снимка: живая, иначе статус-подпись как `custom`.
#[must_use]
pub fn snapshot_activities(live: Vec<Value>, custom_status: Option<&str>) -> Vec<Value> {
    if !live.is_empty() {
        return live;
    }
    custom_status
        .filter(|status| !status.is_empty())
        .map(|status| vec![json!({"type": "custom", "name": status})])
        .unwrap_or_default()
}

fn peer_origins(tx: &mut dyn Tx, status: &str) -> DbResult<Vec<String>> {
    tx.query(
        "SELECT origin FROM federation_peers WHERE status = ?",
        params![status],
    )?
    .iter()
    .map(|row| row.text("origin"))
    .collect()
}

/// Собирает `ready` (без поля `type`) и запоминает в концентраторе
/// пространства, статус и настройку активности пользователя.
///
/// # Errors
/// Ошибка базы; пользователя нет.
#[allow(
    clippy::too_many_lines,
    reason = "один снимок — один проход, как в оригинале"
)]
pub fn build(tx: &mut dyn Tx, hub: &Hub, user_id: &str) -> DbResult<Map<String, Value>> {
    let user_row = tx
        .query_one(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"),
            params![user_id],
        )?
        .ok_or_else(|| DbError::Corrupt("пользователь не найден".into()))?;
    let federated = user_row
        .opt_text("home_instance")?
        .is_some_and(|instance| !instance.is_empty());
    hub.set_show_activity(user_id, user_row.opt_flag("show_activity")? != Some(false));
    hub.set_status(
        user_id,
        &user_row
            .opt_text("status")?
            .unwrap_or_else(|| "offline".to_owned()),
    );

    let space_rows = tx.query(
        "SELECT s.id, s.name, s.icon, s.banner, s.avatar_color, s.owner_id, s.invite_code, \
         s.visibility, s.description, s.created_at, s.directory_listed, s.owner_title \
         FROM space_members m JOIN spaces s ON s.id = m.space_id WHERE m.user_id = ?",
        params![user_id],
    )?;
    let mut spaces = Vec::with_capacity(space_rows.len());
    let mut space_ids = Vec::with_capacity(space_rows.len());
    let mut countable = Vec::new();
    let mut visible: HashSet<String> = HashSet::new();
    // Чьи активности попадают в снимок: (id, homeUserId, homeInstance, customStatus).
    let mut subjects: Vec<(String, Option<String>, Option<String>, Option<String>)> = Vec::new();
    for row in &space_rows {
        let (view, mut count_here) = space_view(tx, row, user_id)?;
        for channel in view["channels"].as_array().into_iter().flatten() {
            if let Some(id) = channel["id"].as_str() {
                visible.insert(id.to_owned());
            }
        }
        for member in view["members"].as_array().into_iter().flatten() {
            subjects.push(subject_of(&member["user"]));
        }
        countable.append(&mut count_here);
        space_ids.push(row.text("id")?);
        spaces.push(view);
    }
    hub.set_user_spaces(user_id, space_ids);

    let dm_channels = open_dm_channels(tx, user_id)?;
    let mut dm_ids = Vec::with_capacity(dm_channels.len());
    for dm in &dm_channels {
        let Some(id) = dm["id"].as_str() else {
            continue;
        };
        dm_ids.push(id.to_owned());
        visible.insert(id.to_owned());
        for member in dm["members"].as_array().into_iter().flatten() {
            subjects.push(subject_of(member));
        }
        // Реплицированному аккаунту переписка, существовавшая до него,
        // засчитывается прочитанной.
        if federated
            && let Some(last) = dm["lastMessage"]["id"].as_str()
            && tx
                .query_one(
                    "SELECT 1 AS one FROM read_states WHERE user_id = ? AND channel_id = ?",
                    params![user_id, id],
                )?
                .is_none()
        {
            tx.execute(
                "INSERT INTO read_states (user_id, channel_id, last_read_message_id, updated_at) \
                 VALUES (?, ?, ?, ?)",
                params![user_id, id, last, crate::state::now_ms()],
            )?;
        }
    }

    let layout = tx.query_one(
        "SELECT layout, updated_at FROM user_space_layout WHERE user_id = ?",
        params![user_id],
    )?;
    let (space_layout, layout_updated_at) = match layout {
        Some(row) => (
            serde_json::from_str::<Value>(&row.text("layout")?).unwrap_or(Value::Null),
            json!(row.opt_int("updated_at")?),
        ),
        None => (Value::Null, Value::Null),
    };

    let read_rows = tx.query(
        "SELECT channel_id, last_read_message_id FROM read_states WHERE user_id = ?",
        params![user_id],
    )?;
    let mut cursors = HashMap::with_capacity(read_rows.len());
    let mut read_states = Vec::with_capacity(read_rows.len());
    for row in &read_rows {
        let channel = row.text("channel_id")?;
        let last = row.text("last_read_message_id")?;
        if !federated || visible.contains(&channel) {
            read_states.push(json!({"channelId": channel, "lastReadMessageId": last}));
        }
        cursors.insert(channel, last);
    }

    for row in tx.query(
        "SELECT u.id, u.home_user_id, u.home_instance, u.custom_status FROM friends f \
         JOIN users u ON u.id = CASE WHEN f.user_id = ? THEN f.friend_id ELSE f.user_id END \
         WHERE f.user_id = ? OR f.friend_id = ?",
        params![user_id, user_id, user_id],
    )? {
        subjects.push((
            row.text("id")?,
            row.opt_text("home_user_id")?,
            row.opt_text("home_instance")?,
            row.opt_text("custom_status")?,
        ));
    }
    let mut user_activities = Map::new();
    let mut activity_identities = Map::new();
    let mut seen = HashSet::new();
    for (id, home_user, home_instance, custom) in subjects {
        if !seen.insert(id.clone()) {
            continue;
        }
        let activities = snapshot_activities(hub.activities(&id), custom.as_deref());
        if !activities.is_empty() {
            user_activities.insert(id.clone(), json!(activities));
            activity_identities.insert(id, presence_identity(home_user, home_instance));
        }
    }

    let pending_approvals = if user_row.opt_flag("is_admin")? == Some(true) {
        tx.query_one(
            "SELECT count(*) AS n FROM peer_approval_requests",
            params![],
        )?
        .map_or(Ok(0), |row| row.int("n"))?
    } else {
        0
    };

    let mut unread = Map::new();
    for id in &countable {
        let cursor = cursors.get(id).map_or("0", String::as_str);
        unread.insert(
            id.clone(),
            json!(count_channel_unread(tx, user_id, id, cursor)?),
        );
    }
    for id in &dm_ids {
        let cursor = cursors.get(id).map_or("0", String::as_str);
        unread.insert(id.clone(), json!(count_dm_unread(tx, user_id, id, cursor)?));
    }

    let mut ready = Map::new();
    ready.insert("supportsPoke".into(), json!(true));
    ready.insert("unreadCounts".into(), Value::Object(unread));
    ready.insert("user".into(), sanitize_user(&user_row, true)?);
    ready.insert("spaces".into(), json!(spaces));
    ready.insert("dmChannels".into(), json!(dm_channels));
    ready.insert("folders".into(), json!(folders(tx, user_id)?));
    ready.insert("spaceLayout".into(), space_layout);
    ready.insert("layoutUpdatedAt".into(), layout_updated_at);
    ready.insert("voiceStates".into(), json!({}));
    ready.insert("voiceChannelElapsedSeconds".into(), json!({}));
    ready.insert("voiceUserStates".into(), json!({}));
    ready.insert("spaceVoiceStates".into(), json!({}));
    ready.insert("readStates".into(), json!(read_states));
    ready.insert("activeCalls".into(), json!([]));
    ready.insert("userActivities".into(), Value::Object(user_activities));
    ready.insert(
        "userActivityIdentities".into(),
        Value::Object(activity_identities),
    );
    ready.insert(
        "rejectedPeerOrigins".into(),
        json!(peer_origins(tx, "rejected")?),
    );
    ready.insert(
        "awaitingApprovalPeerOrigins".into(),
        json!(peer_origins(tx, "awaiting_approval")?),
    );
    ready.insert(
        "activePeerOrigins".into(),
        json!(peer_origins(tx, "active")?),
    );
    ready.insert("pendingApprovalCount".into(), json!(pending_approvals));
    Ok(ready)
}

fn subject_of(user: &Value) -> (String, Option<String>, Option<String>, Option<String>) {
    let text = |key: &str| user[key].as_str().map(str::to_owned);
    (
        text("id").unwrap_or_default(),
        text("homeUserId"),
        text("homeInstance"),
        text("customStatus"),
    )
}
