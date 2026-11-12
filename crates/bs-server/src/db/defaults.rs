//! Значения по умолчанию (`ensureDefaults` оригинала): строка настроек,
//! номер воркера для snowflake, эпоха инстанса, дата установки и первый
//! администратор. Общие для обоих бэкендов.

use rand::Rng as _;

use super::{DbResult, Tx};
use crate::params;
use crate::state::now_ms;

/// Дополняет базу тем, без чего сервер не работает.
///
/// # Errors
/// Ошибка базы.
pub fn ensure(tx: &mut dyn Tx) -> DbResult<()> {
    tx.execute(
        "INSERT INTO instance_settings
            (id, max_bitrate_kbps, min_bitrate_kbps, bitrate_step_kbps,
             allowed_resolutions, allowed_framerates, max_resolution, max_framerate, updated_at)
         VALUES (1, 20000, 500, 500, '540,720,1080', '30,45,60', 1080, 60, ?)
         ON CONFLICT (id) DO NOTHING",
        params![now_ms()],
    )?;
    let row = tx.query_one(
        "SELECT worker_id, instance_id, installed_at FROM instance_settings WHERE id = 1",
        params![],
    )?;
    let (worker, instance, installed) = match &row {
        Some(row) => (
            row.opt_int("worker_id")?,
            row.opt_text("instance_id")?,
            row.opt_int("installed_at")?,
        ),
        None => (None, None, None),
    };
    if worker.is_none() {
        let id: i64 = rand::rng().random_range(0..1024);
        tx.execute(
            "UPDATE instance_settings SET worker_id = ? WHERE id = 1",
            params![id],
        )?;
    }
    if instance.is_none() {
        tx.execute(
            "UPDATE instance_settings SET instance_id = ? WHERE id = 1",
            params![uuid::Uuid::new_v4().to_string()],
        )?;
    }
    if installed.is_none() {
        let oldest = tx
            .query_one(
                "SELECT created_at FROM users
                 WHERE home_instance IS NULL AND (is_deleted IS NULL OR is_deleted = 0)
                 ORDER BY created_at ASC LIMIT 1",
                params![],
            )?
            .map(|row| row.int("created_at"))
            .transpose()?;
        tx.execute(
            "UPDATE instance_settings SET installed_at = ? WHERE id = 1",
            params![oldest.unwrap_or_else(now_ms)],
        )?;
    }
    tx.execute(
        "UPDATE users SET is_admin = 1
         WHERE id = (SELECT id FROM users ORDER BY created_at ASC LIMIT 1)
           AND NOT EXISTS (SELECT 1 FROM users WHERE is_admin = 1)",
        params![],
    )?;
    Ok(())
}
