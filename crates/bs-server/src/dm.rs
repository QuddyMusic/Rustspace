//! Личные переписки: общие проверки.

use crate::db::{DbResult, Tx};
use crate::params;

/// Состоит ли пользователь в переписке.
///
/// # Errors
/// Ошибка базы.
pub fn is_member(tx: &mut dyn Tx, dm_channel_id: &str, user_id: &str) -> DbResult<bool> {
    Ok(tx
        .query_one(
            "SELECT 1 AS one FROM dm_members WHERE dm_channel_id = ? AND user_id = ?",
            params![dm_channel_id, user_id],
        )?
        .is_some())
}
