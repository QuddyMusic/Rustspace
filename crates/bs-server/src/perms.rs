//! Права участника в пространстве и канале (`utils/permissions.ts`).

use bs_permissions::{Permissions, Standing, parse_permission_string};

use crate::db::{DbResult, Row, Tx};
use crate::params;

/// Права из строки базы (`stringToPermissions`): пусто или мусор — нет прав.
#[must_use]
pub fn from_db(text: Option<&str>) -> Permissions {
    text.filter(|t| !t.is_empty())
        .and_then(|t| parse_permission_string(t).ok())
        .unwrap_or_else(Permissions::empty)
}

/// Есть ли право (администратор может всё).
#[must_use]
pub fn has(perms: Permissions, flag: Permissions) -> bool {
    perms.contains(Permissions::ADMINISTRATOR) || perms.contains(flag)
}

/// Применяет переопределение: сначала запрет, потом разрешение.
fn apply(base: Permissions, deny: Permissions, allow: Permissions) -> Permissions {
    (base & !deny) | allow
}

fn override_of<'a>(rows: &'a [Row], kind: &str, target: &str) -> Option<&'a Row> {
    rows.iter().find(|row| {
        row.text("target_type").is_ok_and(|t| t == kind)
            && row.text("target_id").is_ok_and(|t| t == target)
    })
}

fn bits(row: &Row, column: &str) -> DbResult<Permissions> {
    Ok(from_db(row.opt_text(column)?.as_deref()))
}

/// Состоит ли пользователь в пространстве.
///
/// # Errors
/// Ошибка базы.
pub fn is_member(tx: &mut dyn Tx, space_id: &str, user_id: &str) -> DbResult<bool> {
    Ok(tx
        .query_one(
            "SELECT 1 AS one FROM space_members WHERE space_id = ? AND user_id = ?",
            params![space_id, user_id],
        )?
        .is_some())
}

/// Права пользователя в пространстве, а с `channel_id` — в канале
/// (`computePermissions`): владелец и администратор инстанса могут всё;
/// не участник — ничего; дальше `@everyone` и роли, затем переопределения
/// категории и канала в порядке «все → роли → сам участник».
///
/// # Errors
/// Ошибка базы.
pub fn compute(
    tx: &mut dyn Tx,
    user_id: &str,
    space_id: &str,
    channel_id: Option<&str>,
) -> DbResult<Permissions> {
    let Some(space) = tx.query_one(
        "SELECT owner_id FROM spaces WHERE id = ?",
        params![space_id],
    )?
    else {
        return Ok(Permissions::empty());
    };
    if space.text("owner_id")? == user_id {
        return Ok(Permissions::all());
    }
    let admin = tx
        .query_one("SELECT is_admin FROM users WHERE id = ?", params![user_id])?
        .map(|row| row.opt_flag("is_admin"))
        .transpose()?
        .flatten()
        == Some(true);
    if admin {
        return Ok(Permissions::all());
    }
    if !is_member(tx, space_id, user_id)? {
        return Ok(Permissions::empty());
    }
    let mut base = tx
        .query_one(
            "SELECT permissions FROM roles WHERE id = ? AND space_id = ?",
            params![space_id, space_id],
        )?
        .map(|row| bits(&row, "permissions"))
        .transpose()?
        .unwrap_or_else(Permissions::empty);
    let assigned = tx.query(
        "SELECT mr.role_id AS role_id, r.permissions AS permissions FROM member_roles mr \
         LEFT JOIN roles r ON r.id = mr.role_id WHERE mr.space_id = ? AND mr.user_id = ?",
        params![space_id, user_id],
    )?;
    let mut role_ids = Vec::with_capacity(assigned.len());
    for row in &assigned {
        role_ids.push(row.text("role_id")?);
        base |= bits(row, "permissions")?;
    }
    if base.contains(Permissions::ADMINISTRATOR) {
        return Ok(Permissions::all());
    }
    let Some(channel_id) = channel_id else {
        return Ok(base);
    };
    let Some(channel) = tx.query_one(
        "SELECT category_id FROM channels WHERE id = ?",
        params![channel_id],
    )?
    else {
        return Ok(base);
    };
    let category = match channel.opt_text("category_id")? {
        Some(category) => tx.query(
            "SELECT target_type, target_id, allow, deny FROM category_overrides WHERE category_id = ?",
            params![category],
        )?,
        None => Vec::new(),
    };
    let own = tx.query(
        "SELECT target_type, target_id, allow, deny FROM channel_overrides WHERE channel_id = ?",
        params![channel_id],
    )?;
    if category.is_empty() && own.is_empty() {
        return Ok(base);
    }
    // 1. @everyone: категория, затем канал.
    for rows in [&category, &own] {
        if let Some(row) = override_of(rows, "role", space_id) {
            base = apply(base, bits(row, "deny")?, bits(row, "allow")?);
        }
    }
    // 2. Роли участника вместе: категория, затем канал.
    for rows in [&category, &own] {
        let (mut allow, mut deny) = (Permissions::empty(), Permissions::empty());
        for role in &role_ids {
            if let Some(row) = override_of(rows, "role", role) {
                allow |= bits(row, "allow")?;
                deny |= bits(row, "deny")?;
            }
        }
        base = apply(base, deny, allow);
    }
    // 3. Сам участник: категория, затем канал.
    for rows in [&category, &own] {
        if let Some(row) = override_of(rows, "member", user_id) {
            base = apply(base, bits(row, "deny")?, bits(row, "allow")?);
        }
    }
    Ok(base)
}

/// Видит ли пользователь канал.
#[must_use]
pub fn can_view(perms: Permissions) -> bool {
    perms.intersects(Permissions::VIEW_CHANNEL | Permissions::ADMINISTRATOR)
}

/// Считаются ли непрочитанные (`canCountUnread`).
#[must_use]
pub fn can_count_unread(perms: Permissions) -> bool {
    perms.contains(Permissions::ADMINISTRATOR)
        || perms.contains(Permissions::VIEW_CHANNEL | Permissions::READ_MESSAGE_HISTORY)
}

/// Права строкой, как их ждут клиенты.
#[must_use]
pub fn to_wire(perms: Permissions) -> String {
    bs_permissions::permissions_to_string(perms)
}

/// Есть ли у пользователя право в пространстве (`hasPermission`).
///
/// # Errors
/// Ошибка базы.
pub fn has_permission(
    tx: &mut dyn Tx,
    user_id: &str,
    space_id: &str,
    flag: Permissions,
) -> DbResult<bool> {
    Ok(has(compute(tx, user_id, space_id, None)?, flag))
}

/// Есть ли у пользователя право в канале.
///
/// # Errors
/// Ошибка базы.
pub fn has_channel_permission(
    tx: &mut dyn Tx,
    user_id: &str,
    space_id: &str,
    channel_id: &str,
    flag: Permissions,
) -> DbResult<bool> {
    Ok(has(compute(tx, user_id, space_id, Some(channel_id))?, flag))
}

/// Владеет ли пользователь пространством.
///
/// # Errors
/// Ошибка базы.
pub fn is_owner(tx: &mut dyn Tx, space_id: &str, user_id: &str) -> DbResult<bool> {
    Ok(tx
        .query_one(
            "SELECT 1 AS one FROM spaces WHERE id = ? AND owner_id = ?",
            params![space_id, user_id],
        )?
        .is_some())
}

/// Забанен ли пользователь в пространстве.
///
/// # Errors
/// Ошибка базы.
pub fn is_banned(tx: &mut dyn Tx, space_id: &str, user_id: &str) -> DbResult<bool> {
    Ok(tx
        .query_one(
            "SELECT 1 AS one FROM bans WHERE space_id = ? AND user_id = ?",
            params![space_id, user_id],
        )?
        .is_some())
}

/// Место пользователя в иерархии пространства (`getHierarchyStanding`).
///
/// # Errors
/// Ошибка базы.
pub fn standing(tx: &mut dyn Tx, space_id: &str, user_id: &str) -> DbResult<Standing> {
    let is_instance_admin = tx
        .query_one("SELECT is_admin FROM users WHERE id = ?", params![user_id])?
        .map(|row| row.opt_flag("is_admin"))
        .transpose()?
        .flatten()
        == Some(true);
    let top_position = tx
        .query_one(
            "SELECT MAX(r.position) AS top FROM member_roles mr JOIN roles r ON r.id = mr.role_id \
             WHERE mr.space_id = ? AND mr.user_id = ? AND r.space_id = ? AND r.id <> ?",
            params![space_id, user_id, space_id, space_id],
        )?
        .map(|row| row.opt_int("top"))
        .transpose()?
        .flatten()
        .unwrap_or(0)
        .max(0);
    Ok(Standing {
        is_owner: is_owner(tx, space_id, user_id)?,
        is_instance_admin,
        top_position,
    })
}

/// Может ли `actor` действовать на участника `target`.
///
/// # Errors
/// Ошибка базы.
pub fn can_act_on(tx: &mut dyn Tx, space_id: &str, actor: &str, target: &str) -> DbResult<bool> {
    Ok(bs_permissions::can_act_on_member(
        standing(tx, space_id, actor)?,
        standing(tx, space_id, target)?,
    ))
}

/// Ошибка 403 `missing_permission` с названием права.
#[must_use]
pub fn missing(permission: &'static str) -> crate::error::ApiError {
    crate::error::ApiError::forbidden("missing_permission").detail("permission", permission)
}
