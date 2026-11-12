//! Слой хранения.
//!
//! Обработчики не знают про конкретную СУБД: они работают через [`Tx`] —
//! запрос с параметрами [`Value`] и строки [`Row`]. Сейчас за ним SQLite с
//! той же схемой, что у оригинала (файл базы взаимозаменяем); PostgreSQL
//! подключается вторым бэкендом за тем же интерфейсом. SQL пишется в общем
//! для обеих подмножестве: параметры `?`, `ON CONFLICT` вместо
//! `INSERT OR IGNORE`.
//!
//! Работа с базой блокирующая, поэтому идёт в `spawn_blocking`; запись — в
//! одной транзакции на вызов, как у оригинала с его единственным соединением.

pub mod defaults;
pub mod extensions;
pub mod migrations;
pub mod postgres;
mod sqlite;

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

/// Ошибка хранилища.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// Ошибка SQLite.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Ошибка PostgreSQL.
    #[error("postgres: {0}")]
    Postgres(#[source] Box<::postgres::Error>),
    /// Ввод-вывод (каталог базы).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// В строке нет такого столбца или тип не тот.
    #[error("столбец {0}: {1}")]
    Column(String, &'static str),
    /// Фоновая задача с запросом оборвалась.
    #[error("задача базы данных прервана")]
    Cancelled,
    /// Данные в базе не такие, как ждёт сервер.
    #[error("{0}")]
    Corrupt(String),
}

impl DbError {
    /// Нарушено ограничение уникальности (или другое ограничение целостности).
    #[must_use]
    pub fn is_unique_violation(&self) -> bool {
        match self {
            Self::Sqlite(rusqlite::Error::SqliteFailure(error, _)) => {
                error.code == rusqlite::ErrorCode::ConstraintViolation
            }
            // Класс 23 — нарушение ограничения целостности.
            Self::Postgres(error) => error
                .code()
                .is_some_and(|code| code.code().starts_with("23")),
            _ => false,
        }
    }
}

/// Результат работы с базой.
pub type DbResult<T> = Result<T, DbError>;

/// Значение параметра или столбца.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `NULL`.
    Null,
    /// Целое.
    Int(i64),
    /// Вещественное.
    Real(f64),
    /// Текст.
    Text(String),
    /// Двоичные данные.
    Blob(Vec<u8>),
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        if value.fract() == 0.0 && value.abs() < 9.0e15 {
            #[allow(clippy::cast_possible_truncation, reason = "проверено выше")]
            Self::Int(value as i64)
        } else {
            Self::Real(value)
        }
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&String> for Value {
    fn from(value: &String) -> Self {
        Self::Text(value.clone())
    }
}

impl From<&Option<String>> for Value {
    fn from(value: &Option<String>) -> Self {
        value.as_deref().map_or(Self::Null, Into::into)
    }
}

impl<T: Into<Self>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Null, Into::into)
    }
}

/// Список параметров запроса: `params![a, b, c]`.
#[macro_export]
macro_rules! params {
    ($($value:expr),* $(,)?) => {
        &[$($crate::db::Value::from($value)),*]
    };
}

/// Строка результата.
#[derive(Debug, Clone)]
pub struct Row {
    columns: Arc<[String]>,
    values: Vec<Value>,
}

impl Row {
    fn value(&self, column: &str) -> DbResult<&Value> {
        self.columns
            .iter()
            .position(|name| name == column)
            .map(|index| &self.values[index])
            .ok_or_else(|| DbError::Column(column.to_owned(), "нет в результате"))
    }

    /// Текст; `NULL` — `None`.
    ///
    /// # Errors
    /// Нет такого столбца или в нём не текст.
    pub fn opt_text(&self, column: &str) -> DbResult<Option<String>> {
        match self.value(column)? {
            Value::Null => Ok(None),
            Value::Text(text) => Ok(Some(text.clone())),
            Value::Int(n) => Ok(Some(n.to_string())),
            _ => Err(DbError::Column(column.to_owned(), "ожидался текст")),
        }
    }

    /// Обязательный текст.
    ///
    /// # Errors
    /// Нет такого столбца, `NULL` или не текст.
    pub fn text(&self, column: &str) -> DbResult<String> {
        self.opt_text(column)?
            .ok_or_else(|| DbError::Column(column.to_owned(), "неожиданный NULL"))
    }

    /// Целое; `NULL` — `None`.
    ///
    /// # Errors
    /// Нет такого столбца или в нём не число.
    pub fn opt_int(&self, column: &str) -> DbResult<Option<i64>> {
        match self.value(column)? {
            Value::Null => Ok(None),
            Value::Int(n) => Ok(Some(*n)),
            // SQLite хранит «numeric» как получится; drizzle пишет целые.
            #[allow(
                clippy::cast_possible_truncation,
                reason = "целое, записанное вещественным"
            )]
            Value::Real(n) if n.fract() == 0.0 => Ok(Some(*n as i64)),
            Value::Text(text) => text
                .parse()
                .map(Some)
                .map_err(|_| DbError::Column(column.to_owned(), "ожидалось целое")),
            _ => Err(DbError::Column(column.to_owned(), "ожидалось целое")),
        }
    }

    /// Число как значение JSON: целое остаётся целым, дробное — дробным.
    ///
    /// # Errors
    /// Столбца нет или в нём не число.
    pub fn opt_number(&self, column: &str) -> DbResult<Option<serde_json::Value>> {
        match self.value(column)? {
            Value::Null => Ok(None),
            Value::Int(value) => Ok(Some(serde_json::json!(value))),
            Value::Real(value) if value.fract() == 0.0 => {
                Ok(Some(serde_json::json!(*value as i64)))
            }
            Value::Real(value) => Ok(Some(serde_json::json!(value))),
            _ => Err(DbError::Column(column.to_owned(), "не число")),
        }
    }

    /// Обязательное целое.
    ///
    /// # Errors
    /// Нет такого столбца, `NULL` или не число.
    pub fn int(&self, column: &str) -> DbResult<i64> {
        self.opt_int(column)?
            .ok_or_else(|| DbError::Column(column.to_owned(), "неожиданный NULL"))
    }

    /// Флаг `0/1`; `NULL` — `None`.
    ///
    /// # Errors
    /// Нет такого столбца или в нём не число.
    pub fn opt_flag(&self, column: &str) -> DbResult<Option<bool>> {
        Ok(self.opt_int(column)?.map(|n| n != 0))
    }
}

/// Доступ к базе внутри одного вызова (для записи — внутри транзакции).
pub trait Tx {
    /// Выполняет запрос и возвращает все строки.
    ///
    /// # Errors
    /// Ошибка СУБД.
    fn query(&mut self, sql: &str, params: &[Value]) -> DbResult<Vec<Row>>;

    /// Выполняет изменение и возвращает число затронутых строк.
    ///
    /// # Errors
    /// Ошибка СУБД.
    fn execute(&mut self, sql: &str, params: &[Value]) -> DbResult<u64>;

    /// Первая строка запроса, если есть.
    ///
    /// # Errors
    /// Ошибка СУБД.
    fn query_one(&mut self, sql: &str, params: &[Value]) -> DbResult<Option<Row>> {
        Ok(self.query(sql, params)?.into_iter().next())
    }
}

/// Соединение с PostgreSQL и кэш переведённых запросов.
struct Postgres {
    /// `None` только во время закрытия.
    client: Option<::postgres::Client>,
    translated: std::collections::HashMap<String, String>,
}

impl Drop for Postgres {
    fn drop(&mut self) {
        // Закрывая соединение, драйвер ждёт на своём исполнителе: из
        // асинхронного кода так можно только в блокирующем участке.
        let client = self.client.take();
        off_runtime(move || drop(client));
    }
}

#[derive(Clone)]
enum Backend {
    Sqlite(Arc<Mutex<rusqlite::Connection>>),
    // Одно соединение, как у SQLite: обработчики рассчитаны на то, что
    // транзакции идут строго по очереди.
    Postgres(Arc<Mutex<Postgres>>),
}

/// База данных сервера.
#[derive(Clone)]
pub struct Db {
    backend: Backend,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.backend {
            Backend::Sqlite(_) => "Db(sqlite)",
            Backend::Postgres(_) => "Db(postgres)",
        })
    }
}

/// Выполняет блокирующую работу с сетью так, чтобы её можно было вызвать и
/// из асинхронного кода: драйвер PostgreSQL внутри ждёт на своём исполнителе.
fn off_runtime<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(work)
        }
        _ => work(),
    }
}

impl Db {
    /// Открывает (и при необходимости создаёт) базу SQLite, применяет
    /// миграции и значения по умолчанию.
    ///
    /// # Errors
    /// Файл не открывается или миграция не применилась.
    pub fn open_sqlite(path: &Path) -> DbResult<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        Self::prepare(rusqlite::Connection::open(path)?)
    }

    /// База в памяти (для тестов).
    ///
    /// # Errors
    /// Миграция не применилась.
    pub fn open_memory() -> DbResult<Self> {
        Self::prepare(rusqlite::Connection::open_in_memory()?)
    }

    fn prepare(mut connection: rusqlite::Connection) -> DbResult<Self> {
        sqlite::configure(&connection)?;
        sqlite::migrate(&mut connection)?;
        let db = Self {
            backend: Backend::Sqlite(Arc::new(Mutex::new(connection))),
        };
        db.transaction_blocking(|tx| {
            defaults::ensure(tx)?;
            extensions::apply(tx)
        })?;
        Ok(db)
    }

    /// Подключается к PostgreSQL по адресу вида
    /// `postgres://user:password@host/database`, приводит схему к схеме
    /// оригинала и дополняет значения по умолчанию.
    ///
    /// # Errors
    /// Сервер недоступен или схему не удалось создать.
    pub fn open_postgres(url: &str) -> DbResult<Self> {
        let db = off_runtime(|| -> DbResult<Self> {
            let mut client = ::postgres::Client::connect(url, ::postgres::NoTls)
                .map_err(|error| DbError::Postgres(Box::new(error)))?;
            postgres::sync_schema(&mut client)?;
            let state = Postgres {
                client: Some(client),
                translated: std::collections::HashMap::new(),
            };
            Ok(Self {
                backend: Backend::Postgres(Arc::new(Mutex::new(state))),
            })
        })?;
        db.transaction_blocking(|tx| extensions::apply(tx))?;
        db.transaction_blocking(|tx| defaults::ensure(tx))?;
        Ok(db)
    }

    /// Переносит все данные из файла SQLite (оригинального сервера или
    /// `bs-server`) в эту базу PostgreSQL. База должна быть пустой: в ней не
    /// должно быть пользователей. Возвращает число перенесённых строк.
    ///
    /// # Errors
    /// База не PostgreSQL, не пуста, либо ошибка чтения или записи.
    pub fn import_sqlite(&self, path: &Path) -> DbResult<u64> {
        if !matches!(self.backend, Backend::Postgres(_)) {
            return Err(DbError::Corrupt(
                "перенос возможен только в PostgreSQL".to_owned(),
            ));
        }
        let source = Self::open_sqlite(path)?;
        let Backend::Sqlite(connection) = &source.backend else {
            return Err(DbError::Corrupt("источник не SQLite".to_owned()));
        };
        let mut tables = {
            let connection = connection.lock().unwrap_or_else(PoisonError::into_inner);
            postgres::tables_in_dependency_order(&connection)?
        };
        // Учёт расширений в новой базе уже свой; их данные переносятся как все.
        tables.retain(|(table, _)| table != "bs_migrations");
        self.transaction_blocking(|target| {
            if target
                .query_one("SELECT id FROM users LIMIT 1", &[])?
                .is_some()
            {
                return Err(DbError::Corrupt(
                    "база PostgreSQL не пуста: перенос отменён".to_owned(),
                ));
            }
            // Строки, созданные при подготовке пустой базы, заменяются перенесёнными.
            target.execute("DELETE FROM instance_settings", &[])?;
            let mut copied = 0;
            for (table, columns) in &tables {
                let list = columns
                    .iter()
                    .map(|column| format!("\"{column}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                let marks = vec!["?"; columns.len()].join(", ");
                let rows = source.transaction_blocking(|tx| {
                    tx.query(&format!("SELECT {list} FROM \"{table}\""), &[])
                })?;
                let insert = format!("INSERT INTO \"{table}\" ({list}) VALUES ({marks})");
                for row in rows {
                    target.execute(&insert, &row.values)?;
                    copied += 1;
                }
            }
            Ok(copied)
        })
    }

    /// Блокирующий вызов в транзакции: изменения применяются целиком или
    /// не применяются вовсе (если `work` вернул ошибку).
    ///
    /// # Errors
    /// Ошибка СУБД или ошибка из `work`.
    pub fn transaction_blocking<T, E: From<DbError>>(
        &self,
        work: impl FnOnce(&mut dyn Tx) -> Result<T, E>,
    ) -> Result<T, E> {
        match &self.backend {
            Backend::Sqlite(connection) => {
                let mut connection = connection.lock().unwrap_or_else(PoisonError::into_inner);
                let transaction = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(DbError::from)?;
                let mut tx = sqlite::SqliteTx(&transaction);
                let result = work(&mut tx)?;
                transaction.commit().map_err(DbError::from)?;
                drop(connection);
                Ok(result)
            }
            Backend::Postgres(state) => off_runtime(|| {
                let failed = |error: ::postgres::Error| DbError::Postgres(Box::new(error));
                let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
                let Postgres { client, translated } = &mut *state;
                let client = client.as_mut().ok_or(DbError::Cancelled)?;
                let mut transaction = client.transaction().map_err(failed)?;
                let result = work(&mut postgres::PgTx {
                    tx: &mut transaction,
                    translated,
                })?;
                transaction.commit().map_err(failed)?;
                Ok(result)
            }),
        }
    }
    /// То же из асинхронного кода: работа уходит в пул блокирующих задач.
    ///
    /// # Errors
    /// Ошибка СУБД, ошибка из `work` или обрыв задачи.
    pub async fn transaction<T, E>(
        &self,
        work: impl FnOnce(&mut dyn Tx) -> Result<T, E> + Send + 'static,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<DbError> + Send + 'static,
    {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.transaction_blocking(work))
            .await
            .map_err(|_| E::from(DbError::Cancelled))?
    }
}
