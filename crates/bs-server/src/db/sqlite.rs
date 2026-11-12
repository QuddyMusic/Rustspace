//! Бэкенд SQLite: соединение, миграции drizzle и значения по умолчанию.

use sha2::{Digest as _, Sha256};

use super::migrations::MIGRATIONS;
use super::{DbError, DbResult, Row, Tx, Value};

/// Режимы соединения, как у оригинала: WAL и внешние ключи.
pub(super) fn configure(connection: &rusqlite::Connection) -> DbResult<()> {
    // В памяти WAL недоступен — SQLite молча оставит свой режим.
    connection.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
    Ok(())
}

/// Применяет миграции так же, как мигратор drizzle для better-sqlite3:
/// таблица `__drizzle_migrations`, применяется всё, что новее последней
/// записи по времени журнала, вся пачка — одной транзакцией.
pub(super) fn migrate(connection: &mut rusqlite::Connection) -> DbResult<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS \"__drizzle_migrations\" (
            id SERIAL PRIMARY KEY,
            hash text NOT NULL,
            created_at numeric
        )",
    )?;
    let last: Option<i64> = connection
        .query_row(
            "SELECT created_at FROM \"__drizzle_migrations\" ORDER BY created_at DESC LIMIT 1",
            [],
            |row| row.get::<_, Option<f64>>(0),
        )
        .map(|value| {
            #[allow(clippy::cast_possible_truncation, reason = "миллисекунды журнала")]
            value.map(|v| v as i64)
        })
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    let pending: Vec<_> = MIGRATIONS
        .iter()
        .filter(|migration| last.is_none_or(|last| last < migration.when))
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    // Миграции оригинала сами переключают внешние ключи там, где пересоздают
    // таблицы; прагма внутри транзакции не действует, как и у оригинала.
    let transaction = connection.transaction()?;
    for migration in pending {
        for statement in migration.sql.split("--> statement-breakpoint") {
            if statement.trim().is_empty() {
                continue;
            }
            transaction
                .execute_batch(statement)
                .map_err(|err| DbError::Corrupt(format!("миграция {}: {err}", migration.tag)))?;
        }
        transaction.execute(
            "INSERT INTO \"__drizzle_migrations\" (\"hash\", \"created_at\") VALUES (?1, ?2)",
            rusqlite::params![hex::encode(Sha256::digest(migration.sql)), migration.when],
        )?;
        tracing::info!(migration = migration.tag, "миграция применена");
    }
    transaction.commit()?;
    Ok(())
}

impl rusqlite::ToSql for Value {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, ValueRef};
        Ok(ToSqlOutput::Borrowed(match self {
            Self::Null => ValueRef::Null,
            Self::Int(n) => ValueRef::Integer(*n),
            Self::Real(n) => ValueRef::Real(*n),
            Self::Text(text) => ValueRef::Text(text.as_bytes()),
            Self::Blob(bytes) => ValueRef::Blob(bytes),
        }))
    }
}

/// Транзакция SQLite за общим интерфейсом.
pub(super) struct SqliteTx<'a>(pub(super) &'a rusqlite::Transaction<'a>);

impl Tx for SqliteTx<'_> {
    fn query(&mut self, sql: &str, params: &[Value]) -> DbResult<Vec<Row>> {
        let mut statement = self.0.prepare_cached(sql)?;
        let columns: std::sync::Arc<[String]> = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let count = columns.len();
        let mut rows = statement.query(rusqlite::params_from_iter(params))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let mut values = Vec::with_capacity(count);
            for index in 0..count {
                use rusqlite::types::ValueRef;
                values.push(match row.get_ref(index)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => Value::Int(n),
                    ValueRef::Real(n) => Value::Real(n),
                    ValueRef::Text(text) => Value::Text(String::from_utf8_lossy(text).into_owned()),
                    ValueRef::Blob(bytes) => Value::Blob(bytes.to_vec()),
                });
            }
            out.push(Row {
                columns: std::sync::Arc::clone(&columns),
                values,
            });
        }
        Ok(out)
    }

    fn execute(&mut self, sql: &str, params: &[Value]) -> DbResult<u64> {
        let mut statement = self.0.prepare_cached(sql)?;
        let changed = statement.execute(rusqlite::params_from_iter(params))?;
        Ok(changed as u64)
    }
}
