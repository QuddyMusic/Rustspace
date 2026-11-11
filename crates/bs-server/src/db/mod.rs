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

pub mod migrations;
mod sqlite;

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

/// Ошибка хранилища.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// Ошибка SQLite.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
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
        matches!(
            self,
            Self::Sqlite(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation
        )
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

/// База данных сервера.
#[derive(Clone)]
pub struct Db {
    sqlite: Arc<Mutex<rusqlite::Connection>>,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Db(sqlite)")
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
        sqlite::ensure_defaults(&mut connection)?;
        Ok(Self {
            sqlite: Arc::new(Mutex::new(connection)),
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
        let mut connection = self.sqlite.lock().unwrap_or_else(PoisonError::into_inner);
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(DbError::from)?;
        let mut tx = sqlite::SqliteTx(&transaction);
        let result = work(&mut tx)?;
        transaction.commit().map_err(DbError::from)?;
        drop(connection);
        Ok(result)
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
