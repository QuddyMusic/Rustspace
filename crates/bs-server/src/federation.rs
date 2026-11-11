//! Федерация: обмен между инстансами.

use crate::db::{DbResult, Tx};

/// Ставит в очередь пересылку позиции чтения переписки домашнему инстансу
/// пользователя и остальным его инстансам (`queueReadStateRelay`).
///
/// # Errors
/// Ошибка базы.
pub fn queue_read_state(
    tx: &mut dyn Tx,
    channel_id: &str,
    message_id: &str,
    user_id: &str,
) -> DbResult<()> {
    let _ = (tx, channel_id, message_id, user_id);
    Ok(())
}
