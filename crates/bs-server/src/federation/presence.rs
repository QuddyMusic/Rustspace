//! Статус копий пользователей других инстансов (`ws/replicaPresence.ts`).
//!
//! Статус копии объявляет её дом; здесь он хранится только в памяти
//! (`Runtime::projections`). Пока у пользователя открыт сеанс здесь, копия
//! без объявленного статуса показывается «в сети».

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use crate::db::{DbResult, Row, Tx};
use crate::hub::Hub;
use crate::params;

/// Выбираемый пользователем статус (`isChosenUserStatus`).
#[must_use]
pub fn is_chosen(status: &str) -> bool {
    matches!(status, "online" | "idle" | "dnd")
}

/// Статус любой (`UserStatus`).
#[must_use]
pub fn is_status(status: &str) -> bool {
    is_chosen(status) || status == "offline"
}

/// Сам ли аккаунт выбирает статус (`ownsChosenStatus`): свой или отделённый.
///
/// # Errors
/// В строке нет нужных столбцов.
pub fn owns_status(row: &Row) -> DbResult<bool> {
    Ok(row
        .opt_text("home_instance")?
        .is_none_or(|home| home.is_empty())
        || row.opt_flag("federation_home_orphaned")? == Some(true))
}

/// Что показывать для копии (`replicaLiveStatus`).
#[must_use]
pub fn live_status(projection: Option<&str>, connected_here: bool) -> String {
    match projection.filter(|status| is_chosen(status)) {
        Some(status) => status.to_owned(),
        None if connected_here => "online".to_owned(),
        None => "offline".to_owned(),
    }
}

fn replica_row(tx: &mut dyn Tx, user_id: &str) -> DbResult<Option<Row>> {
    let Some(row) = tx.query_one(
        "SELECT id, home_instance, federation_home_orphaned, chosen_status, status FROM users WHERE id = ?",
        params![user_id],
    )?
    else {
        return Ok(None);
    };
    Ok((!owns_status(&row)?).then_some(row))
}

fn write_shown(tx: &mut dyn Tx, hub: &Hub, user_id: &str, status: &str) -> DbResult<()> {
    tx.execute(
        "UPDATE users SET status = ? WHERE id = ?",
        params![status, user_id],
    )?;
    if hub.is_online(user_id) {
        hub.set_status(user_id, status);
    }
    Ok(())
}

/// Дом объявил статус копии (`projectReplicaStatus`). Возвращает
/// показываемый статус или `None`, если строка сама владеет статусом.
///
/// # Errors
/// Ошибка базы.
pub fn project(
    tx: &mut dyn Tx,
    hub: &Hub,
    projections: &Mutex<HashMap<String, String>>,
    user_id: &str,
    projection: &str,
) -> DbResult<Option<String>> {
    if replica_row(tx, user_id)?.is_none() {
        return Ok(None);
    }
    {
        let mut map = projections.lock().unwrap_or_else(PoisonError::into_inner);
        if projection == "offline" {
            map.remove(user_id);
        } else {
            map.insert(user_id.to_owned(), projection.to_owned());
        }
    }
    let shown = live_status(Some(projection), hub.is_online(user_id));
    write_shown(tx, hub, user_id, &shown)?;
    Ok(Some(shown))
}

/// Статус копии при входе сеанса (`showReplicaStatusOnConnect`); `None` —
/// строка владеет статусом сама.
///
/// # Errors
/// Ошибка базы.
pub fn on_connect(
    tx: &mut dyn Tx,
    projections: &Mutex<HashMap<String, String>>,
    user_id: &str,
) -> DbResult<Option<String>> {
    let Some(row) = replica_row(tx, user_id)? else {
        return Ok(None);
    };
    let known = projections
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(user_id)
        .cloned();
    let projection = match known {
        Some(known) => Some(known),
        None => row.opt_text("status")?,
    };
    let shown = projection
        .filter(|status| is_chosen(status))
        .unwrap_or_else(|| "online".to_owned());
    tx.execute(
        "UPDATE users SET status = ? WHERE id = ?",
        params![&shown, user_id],
    )?;
    Ok(Some(shown))
}

/// Последний сеанс копии закончился (`showReplicaStatusOnDisconnect`):
/// статус и изменился ли он; `None` — строка владеет статусом сама.
///
/// # Errors
/// Ошибка базы.
pub fn on_disconnect(
    tx: &mut dyn Tx,
    projections: &Mutex<HashMap<String, String>>,
    user_id: &str,
) -> DbResult<Option<(String, bool)>> {
    let Some(row) = replica_row(tx, user_id)? else {
        return Ok(None);
    };
    let status = projections
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(user_id)
        .cloned()
        .unwrap_or_else(|| "offline".to_owned());
    let changed = row.opt_text("status")?.as_deref() != Some(status.as_str());
    if changed {
        tx.execute(
            "UPDATE users SET status = ? WHERE id = ?",
            params![&status, user_id],
        )?;
    }
    Ok(Some((status, changed)))
}

/// Строка удалена: её проекция забывается (`forgetReplicaProjection`).
pub fn forget(projections: &Mutex<HashMap<String, String>>, user_id: &str) {
    projections
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(user_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_status_rules() {
        assert_eq!(live_status(Some("dnd"), false), "dnd");
        assert_eq!(live_status(Some("offline"), true), "online");
        assert_eq!(live_status(None, false), "offline");
    }
}
