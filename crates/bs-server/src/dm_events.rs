//! События личных переписок по WebSocket.

use serde_json::{Map, Value};

use crate::db::{DbResult, Tx};
use crate::events::Context;
use crate::hub::Outbox;
use crate::state::AppState;

/// Обрабатывает событие переписки. `false` — тип не из этой группы.
///
/// # Errors
/// Ошибка базы.
pub fn handle(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    cx: &Context,
    kind: &str,
    event: &Map<String, Value>,
    stale_files: &mut Vec<String>,
) -> DbResult<bool> {
    let _ = (app, tx, out, cx, kind, event, stale_files);
    Ok(false)
}

/// Реакция на личное сообщение: поставить (`add`) или снять.
///
/// # Errors
/// Ошибка базы.
pub fn reaction(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    cx: &Context,
    message_id: &str,
    emoji: &str,
    add: bool,
) -> DbResult<()> {
    let _ = (app, tx, out, cx, message_id, emoji, add);
    Ok(())
}
