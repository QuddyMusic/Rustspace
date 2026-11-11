//! Общее состояние сервера.

use std::sync::Arc;

use crate::config::Config;
use crate::db::{Db, DbError, DbResult};
use crate::params;
use crate::ratelimit::RateLimiter;
use crate::snowflake::Snowflake;

/// Всё, что нужно обработчикам.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Конфигурация.
    pub config: Arc<Config>,
    /// База данных.
    pub db: Db,
    /// Генератор идентификаторов.
    pub ids: Arc<Snowflake>,
    /// Ограничитель частоты.
    pub limiter: RateLimiter,
    /// Эпоха инстанса (`instance_id`): меняется только при сбросе базы.
    pub instance_id: Arc<str>,
    /// Хеш для холостой проверки пароля.
    pub dummy_hash: Arc<str>,
}

impl AppState {
    /// Собирает состояние поверх открытой базы.
    ///
    /// # Errors
    /// В базе нет строки настроек инстанса (миграции не прошли).
    pub fn new(config: Config, db: Db) -> DbResult<Self> {
        let (worker, instance_id) = db.transaction_blocking(|tx| {
            let row = tx
                .query_one(
                    "SELECT worker_id, instance_id FROM instance_settings WHERE id = 1",
                    params![],
                )?
                .ok_or_else(|| DbError::Corrupt("нет строки instance_settings".into()))?;
            Ok::<_, DbError>((row.int("worker_id")?, row.text("instance_id")?))
        })?;
        let worker = u16::try_from(worker)
            .map_err(|_| DbError::Corrupt(format!("worker_id вне диапазона: {worker}")))?;
        Ok(Self {
            dummy_hash: crate::auth::dummy_hash(config.password_hash_cost).into(),
            limiter: RateLimiter::new(config.disable_rate_limits),
            config: Arc::new(config),
            db,
            ids: Arc::new(Snowflake::new(worker)),
            instance_id: instance_id.into(),
        })
    }
}

/// Текущее время, мс от начала эпохи Unix.
#[must_use]
pub fn now_ms() -> i64 {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}
