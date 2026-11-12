//! Бэкенд PostgreSQL.
//!
//! Схема не ведётся отдельно: она выводится из схемы SQLite. При запуске
//! миграции оригинала применяются к пустой базе SQLite в памяти, её таблицы,
//! столбцы, ключи и индексы читаются и переносятся в PostgreSQL; на
//! существующей базе добавляется только недостающее. Так обе базы всегда
//! описывает один источник — миграции оригинала.
//!
//! Запросы обработчиков написаны на общем подмножестве SQL; здесь они лишь
//! приводятся к записи PostgreSQL: параметры `$n`, `ILIKE` (у SQLite `LIKE`
//! не различает регистр латиницы), 64-битные целые.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::Arc;

use postgres::types::{FromSql, IsNull, ToSql, Type, private::BytesMut, to_sql_checked};

use super::{DbError, DbResult, Row, Tx, Value};

/// Столбец с порядком вставки строк. SQLite отдаёт строки запроса без
/// `ORDER BY` в порядке вставки, и оригинал на это полагается (участники,
/// вложения, результаты поиска людей); PostgreSQL порядка не обещает, поэтому
/// он хранится явно и подставляется в такие запросы.
const SEQUENCE: &str = "bs_seq";

/// Столбец таблицы SQLite.
struct Column {
    name: String,
    kind: &'static str,
    not_null: bool,
    default: Option<String>,
    primary: i64,
}

/// Имя не длиннее 63 байт — столько PostgreSQL хранит от идентификатора.
fn short(name: String) -> String {
    let mut end = name.len().min(63);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_owned()
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Тип PostgreSQL для объявленного типа SQLite. Целые — всегда 64 бита:
/// идентификаторы и время в миллисекундах не помещаются в 32.
fn column_type(declared: &str) -> &'static str {
    let declared = declared.to_ascii_lowercase();
    if declared.contains("int") || declared.contains("serial") {
        "BIGINT"
    } else if declared.contains("real")
        || declared.contains("floa")
        || declared.contains("doub")
        || declared.contains("num")
    {
        "DOUBLE PRECISION"
    } else if declared.contains("blob") {
        "BYTEA"
    } else {
        "TEXT"
    }
}

fn default_value(raw: &str, kind: &str) -> String {
    match (raw.trim().to_ascii_lowercase().as_str(), kind) {
        ("true", "BIGINT") => "1".to_owned(),
        ("false", "BIGINT") => "0".to_owned(),
        _ => raw.replace('`', "\""),
    }
}

/// Всё, что нужно создать в PostgreSQL для схемы SQLite: таблицы, затем
/// индексы и внешние ключи (ключи — отдельно, чтобы порядок таблиц не имел
/// значения).
struct Schema {
    tables: Vec<(String, Vec<Column>)>,
    /// Имя индекса и оператор его создания.
    indexes: Vec<(String, String)>,
    /// Имя ограничения, таблица и оператор.
    foreign_keys: Vec<(String, String, String)>,
}

fn read_schema(sqlite: &rusqlite::Connection) -> DbResult<Schema> {
    let mut schema = Schema {
        tables: Vec::new(),
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
    };
    let names: Vec<String> = sqlite
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name <> '__drizzle_migrations' ORDER BY name")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    for table in names {
        let columns: Vec<Column> = sqlite
            .prepare(&format!("PRAGMA table_info({})", quote(&table)))?
            .query_map([], |row| {
                let declared: String = row.get(2)?;
                Ok(Column {
                    name: row.get(1)?,
                    kind: column_type(&declared),
                    not_null: row.get::<_, i64>(3)? != 0,
                    default: row.get(4)?,
                    primary: row.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        let listed: Vec<(String, bool, String)> = sqlite
            .prepare(&format!("PRAGMA index_list({})", quote(&table)))?
            .query_map([], |row| {
                Ok((row.get(1)?, row.get::<_, i64>(2)? != 0, row.get(3)?))
            })?
            .collect::<Result<_, _>>()?;
        for (index, unique, origin) in listed {
            match origin.as_str() {
                // Первичный ключ создаётся вместе с таблицей.
                "pk" => {}
                "u" => {
                    let fields: Vec<String> = sqlite
                        .prepare(&format!("PRAGMA index_info({})", quote(&index)))?
                        .query_map([], |row| row.get::<_, String>(2))?
                        .collect::<Result<_, _>>()?;
                    let name = short(format!("{table}_{}_key", fields.join("_")));
                    let list = fields
                        .iter()
                        .map(|field| quote(field))
                        .collect::<Vec<_>>()
                        .join(", ");
                    schema.indexes.push((
                        name.clone(),
                        format!(
                            "CREATE UNIQUE INDEX {} ON {} ({list})",
                            quote(&name),
                            quote(&table)
                        ),
                    ));
                }
                _ => {
                    let sql: Option<String> = sqlite.query_row(
                        "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1",
                        [&index],
                        |row| row.get(0),
                    )?;
                    if let Some(sql) = sql {
                        let _ = unique;
                        schema.indexes.push((short(index), index_statement(&sql)));
                    }
                }
            }
        }
        let mut keys: HashMap<i64, (String, String, Vec<String>, Vec<String>)> = HashMap::new();
        let mut order = Vec::new();
        let mut statement =
            sqlite.prepare(&format!("PRAGMA foreign_key_list({})", quote(&table)))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let entry = keys.entry(id).or_insert_with(|| {
                order.push(id);
                (
                    row.get(2).unwrap_or_default(),
                    row.get(6).unwrap_or_default(),
                    Vec::new(),
                    Vec::new(),
                )
            });
            entry.2.push(row.get(3)?);
            entry
                .3
                .push(row.get::<_, Option<String>>(4)?.unwrap_or_default());
        }
        for id in order {
            let Some((target, on_delete, from, to)) = keys.remove(&id) else {
                continue;
            };
            let name = short(format!("{table}_{}_fkey", from.join("_")));
            let list = |fields: &[String]| {
                fields
                    .iter()
                    .map(|field| quote(field))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let references = if to.iter().all(String::is_empty) {
                String::new()
            } else {
                format!(" ({})", list(&to))
            };
            let action = match on_delete.to_ascii_uppercase().as_str() {
                "CASCADE" => " ON DELETE CASCADE",
                "SET NULL" => " ON DELETE SET NULL",
                "SET DEFAULT" => " ON DELETE SET DEFAULT",
                "RESTRICT" => " ON DELETE RESTRICT",
                _ => "",
            };
            let statement = format!(
                "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {}{references}{action}",
                quote(&table),
                quote(&name),
                list(&from),
                quote(&target)
            );
            schema.foreign_keys.push((name, table.clone(), statement));
        }
        schema.tables.push((table, columns));
    }
    Ok(schema)
}

/// Оператор создания индекса в записи PostgreSQL: кавычки вместо обратных
/// апострофов, 64-битное приведение, выражения в скобках.
fn index_statement(sqlite_sql: &str) -> String {
    let sql = sqlite_sql.replace('`', "\"");
    let mut out = String::with_capacity(sql.len() + 16);
    let mut rest = sql.as_str();
    while let Some(start) = rest.find("CAST(") {
        let end = rest[start..]
            .find(')')
            .map_or(rest.len(), |end| start + end + 1);
        out.push_str(&rest[..start]);
        out.push('(');
        out.push_str(&rest[start..end].replace(" AS INTEGER)", " AS BIGINT)"));
        out.push(')');
        rest = &rest[end..];
    }
    out.push_str(rest);
    out.replacen(
        "CREATE UNIQUE INDEX ",
        "CREATE UNIQUE INDEX IF NOT EXISTS ",
        1,
    )
    .replacen("CREATE INDEX ", "CREATE INDEX IF NOT EXISTS ", 1)
}

fn column_definition(column: &Column, with_default: bool) -> String {
    let mut text = format!("{} {}", quote(&column.name), column.kind);
    if let Some(default) = column.default.as_deref().filter(|_| with_default) {
        text.push_str(&format!(" DEFAULT {}", default_value(default, column.kind)));
    }
    if column.not_null {
        text.push_str(" NOT NULL");
    }
    text
}

fn pg(error: postgres::Error) -> DbError {
    DbError::Postgres(Box::new(error))
}

/// Приводит схему PostgreSQL к схеме миграций оригинала: создаёт
/// недостающие таблицы, столбцы, индексы и внешние ключи.
pub(super) fn sync_schema(client: &mut postgres::Client) -> DbResult<()> {
    let mut sqlite = rusqlite::Connection::open_in_memory()?;
    super::sqlite::migrate(&mut sqlite)?;
    let schema = read_schema(&sqlite)?;
    let mut tx = client.transaction().map_err(pg)?;
    let existing: HashMap<String, HashSet<String>> = {
        let mut map: HashMap<String, HashSet<String>> = HashMap::new();
        for row in tx
            .query("SELECT table_name, column_name FROM information_schema.columns WHERE table_schema = current_schema()", &[])
            .map_err(pg)?
        {
            map.entry(row.get(0)).or_default().insert(row.get(1));
        }
        map
    };
    let names = |sql: &str, tx: &mut postgres::Transaction<'_>| -> DbResult<HashSet<String>> {
        Ok(tx
            .query(sql, &[])
            .map_err(pg)?
            .iter()
            .map(|row| row.get(0))
            .collect())
    };
    let indexes = names(
        "SELECT indexname::text FROM pg_indexes WHERE schemaname = current_schema()",
        &mut tx,
    )?;
    let constraints = names(
        "SELECT conname::text FROM pg_constraint c JOIN pg_namespace n ON n.oid = c.connamespace WHERE n.nspname = current_schema()",
        &mut tx,
    )?;
    let mut created = 0;
    for (table, columns) in &schema.tables {
        match existing.get(table) {
            None => {
                let mut parts: Vec<String> = columns
                    .iter()
                    .map(|column| column_definition(column, true))
                    .collect();
                parts.push(format!(
                    "{SEQUENCE} BIGINT GENERATED BY DEFAULT AS IDENTITY"
                ));
                let mut primary: Vec<&Column> =
                    columns.iter().filter(|column| column.primary > 0).collect();
                primary.sort_by_key(|column| column.primary);
                if !primary.is_empty() {
                    parts.push(format!(
                        "PRIMARY KEY ({})",
                        primary
                            .iter()
                            .map(|column| quote(&column.name))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                tx.batch_execute(&format!(
                    "CREATE TABLE {} ({})",
                    quote(table),
                    parts.join(", ")
                ))
                .map_err(pg)?;
                created += 1;
            }
            Some(present) => {
                if !present.contains(SEQUENCE) {
                    tx.batch_execute(&format!(
                        "ALTER TABLE {} ADD COLUMN {SEQUENCE} BIGINT GENERATED BY DEFAULT AS IDENTITY",
                        quote(table)
                    ))
                    .map_err(pg)?;
                }
                for column in columns
                    .iter()
                    .filter(|column| !present.contains(&column.name))
                {
                    // Обязательный столбец без значения по умолчанию к
                    // заполненной таблице не добавить: миграции оригинала в
                    // таких случаях всегда задают значение.
                    tx.batch_execute(&format!(
                        "ALTER TABLE {} ADD COLUMN {}",
                        quote(table),
                        column_definition(column, true)
                    ))
                    .map_err(pg)?;
                    created += 1;
                }
            }
        }
    }
    for (name, statement) in &schema.indexes {
        if !indexes.contains(name) {
            tx.batch_execute(statement).map_err(pg)?;
        }
    }
    for (name, _, statement) in &schema.foreign_keys {
        if !constraints.contains(name) {
            tx.batch_execute(statement).map_err(pg)?;
        }
    }
    tx.commit().map_err(pg)?;
    let composite = schema
        .tables
        .iter()
        .filter_map(|(table, columns)| {
            let mut key: Vec<&Column> =
                columns.iter().filter(|column| column.primary > 0).collect();
            key.sort_by_key(|column| column.primary);
            // Есть отдельный индекс по первому столбцу ключа — SQLite отбирает
            // по нему, и строки идут в порядке вставки.
            let shadowed = key.first().is_some_and(|first| {
                let lead = format!("ON {} ({}", quote(table), quote(&first.name));
                schema
                    .indexes
                    .iter()
                    .any(|(_, statement)| statement.contains(&lead))
            });
            (key.len() > 1 && !shadowed).then(|| {
                (
                    table.clone(),
                    key.iter().map(|column| column.name.clone()).collect(),
                )
            })
        })
        .collect();
    let _ = COMPOSITE_KEYS.set(composite);
    if created > 0 {
        tracing::info!(created, "схема PostgreSQL приведена к схеме оригинала");
    }
    Ok(())
}

/// Таблицы в порядке, в котором их можно заполнять с учётом внешних ключей
/// (сначала те, на кого ссылаются).
pub(super) fn tables_in_dependency_order(
    sqlite: &rusqlite::Connection,
) -> DbResult<Vec<(String, Vec<String>)>> {
    let schema = read_schema(sqlite)?;
    let mut depends: HashMap<String, HashSet<String>> = HashMap::new();
    for (_, table, statement) in &schema.foreign_keys {
        if let Some(target) = statement
            .split(" REFERENCES ")
            .nth(1)
            .and_then(|rest| rest.split('"').nth(1))
            && target != table
        {
            depends
                .entry(table.clone())
                .or_default()
                .insert(target.to_owned());
        }
    }
    let mut left: Vec<(String, Vec<String>)> = schema
        .tables
        .into_iter()
        .map(|(table, columns)| {
            (
                table,
                columns.into_iter().map(|column| column.name).collect(),
            )
        })
        .collect();
    let mut ordered: Vec<(String, Vec<String>)> = Vec::new();
    while !left.is_empty() {
        let done: HashSet<String> = ordered.iter().map(|(table, _)| table.clone()).collect();
        let (ready, waiting): (Vec<_>, Vec<_>) = left.into_iter().partition(|(table, _)| {
            depends
                .get(table)
                .is_none_or(|needed| needed.iter().all(|target| done.contains(target)))
        });
        if ready.is_empty() {
            // Взаимные ссылки: дальше порядок уже не важен.
            ordered.extend(waiting);
            break;
        }
        ordered.extend(ready);
        left = waiting;
    }
    Ok(ordered)
}

/// Запрос в записи PostgreSQL: `?` → `$n`, `LIKE` → `ILIKE`, обратные
/// апострофы → кавычки, `AS INTEGER` → `AS BIGINT`. Строковые литералы не
/// затрагиваются.
#[must_use]
pub fn translate(sql: &str) -> String {
    translate_with(sql, COMPOSITE_KEYS.get())
}

/// Составные первичные ключи таблиц: по ним SQLite отдаёт строки, когда
/// запрос отбирает по первому столбцу ключа.
static COMPOSITE_KEYS: std::sync::OnceLock<HashMap<String, Vec<String>>> =
    std::sync::OnceLock::new();

fn translate_with(sql: &str, keys: Option<&HashMap<String, Vec<String>>>) -> String {
    let mut out = String::with_capacity(sql.len() + 16);
    let mut parameter = 0;
    let mut literal = false;
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        match word.to_ascii_uppercase().as_str() {
            "LIKE" => out.push_str("ILIKE"),
            "INTEGER" if out.trim_end().to_ascii_uppercase().ends_with(" AS") => {
                out.push_str("BIGINT")
            }
            _ => out.push_str(word),
        }
        word.clear();
    };
    for c in sql.chars() {
        if literal {
            out.push(c);
            literal = c != '\'';
            continue;
        }
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
            continue;
        }
        flush(&mut word, &mut out);
        match c {
            '\'' => {
                literal = true;
                out.push(c);
            }
            '?' => {
                parameter += 1;
                out.push('$');
                out.push_str(&parameter.to_string());
            }
            '`' => out.push('"'),
            _ => out.push(c),
        }
    }
    flush(&mut word, &mut out);
    with_insertion_order(out, keys)
}

/// Слова запроса вне скобок и строковых литералов: начало и слово в
/// верхнем регистре.
fn top_level_words(sql: &str) -> Vec<(usize, String)> {
    let mut words = Vec::new();
    let (mut depth, mut literal, mut start) = (0i32, false, None);
    for (at, c) in sql.char_indices().chain(std::iter::once((sql.len(), ' '))) {
        let in_word = !literal
            && depth == 0
            && (c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '"');
        if in_word {
            start.get_or_insert(at);
            continue;
        }
        if let Some(from) = start.take() {
            words.push((from, sql[from..at].to_ascii_uppercase()));
        }
        if literal {
            literal = c != '\'';
        } else {
            match c {
                '\'' => literal = true,
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
        }
    }
    words
}

/// Есть ли в списке выборки итоговая функция (вложенные запросы не в счёт).
fn has_aggregate(select_list: &str) -> bool {
    let upper = select_list.to_ascii_uppercase();
    let mut outer = String::with_capacity(upper.len());
    let mut skip_depth = 0;
    let mut rest = upper.as_str();
    while let Some(c) = rest.chars().next() {
        if skip_depth == 0 && rest.starts_with("(SELECT") {
            skip_depth = 1;
        } else if skip_depth > 0 {
            match c {
                '(' => skip_depth += 1,
                ')' => skip_depth -= 1,
                _ => {}
            }
        } else {
            outer.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    ["COUNT(", "SUM(", "MAX(", "MIN(", "AVG("]
        .iter()
        .any(|function| outer.contains(function))
}

/// Добавляет к простому запросу без своего порядка сортировку по порядку
/// вставки строк первой таблицы — так, как их отдал бы SQLite.
fn with_insertion_order(sql: String, keys: Option<&HashMap<String, Vec<String>>>) -> String {
    const AFTER_TABLE: [&str; 13] = [
        "WHERE", "JOIN", "LEFT", "INNER", "CROSS", "RIGHT", "FULL", "ON", "LIMIT", "OFFSET",
        "NATURAL", "FOR", "AS",
    ];
    let words = top_level_words(&sql);
    let is = |index: usize, word: &str| words.get(index).is_some_and(|(_, found)| found == word);
    if !is(0, "SELECT") || is(1, "DISTINCT") {
        return sql;
    }
    let has = |word: &str| words.iter().any(|(_, found)| found == word);
    if has("ORDER") || has("GROUP") || has("UNION") || has("EXCEPT") || has("INTERSECT") {
        return sql;
    }
    let Some(from) = words.iter().position(|(_, word)| word == "FROM") else {
        return sql;
    };
    // Итоговые функции возвращают одну строку: порядок им не нужен.
    if has_aggregate(&sql[..words[from].0]) {
        return sql;
    }
    let Some((table_at, table_word)) = words.get(from + 1) else {
        return sql;
    };
    let table = &sql[*table_at..table_at + table_word.len()];
    // Таблицы расширений создаются без столбца порядка: их запросы от него не зависят.
    if table.trim_matches('"').starts_with("bs_") {
        return sql;
    }
    let alias = match words.get(from + 2) {
        Some((_, word)) if word == "AS" => words
            .get(from + 3)
            .map(|(at, word)| &sql[*at..at + word.len()]),
        Some((at, word)) if !AFTER_TABLE.contains(&word.as_str()) => {
            Some(&sql[*at..at + word.len()])
        }
        _ => None,
    };
    let name = alias.unwrap_or(table);
    // Отбор по первому столбцу составного ключа SQLite выполняет по индексу
    // ключа и отдаёт строки в его порядке, а не в порядке вставки.
    let by_key = keys
        .and_then(|keys| keys.get(table.trim_matches('"')))
        .filter(|key| {
            let condition = sql[words[from].0..].replace(' ', "");
            key.first()
                .is_some_and(|first| condition.contains(&format!("{first}=")))
        });
    let order = match by_key {
        Some(key) => format!(
            " ORDER BY {}",
            key.iter()
                .map(|column| format!("{name}.{column}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => format!(" ORDER BY {name}.{SEQUENCE}"),
    };
    match words
        .iter()
        .skip(from)
        .find(|(_, word)| matches!(word.as_str(), "LIMIT" | "OFFSET" | "FOR"))
    {
        Some((at, _)) => format!("{}{order} {}", sql[..*at].trim_end(), &sql[*at..]),
        None => format!("{}{order}", sql.trim_end()),
    }
}

type Boxed = Box<dyn Error + Sync + Send>;

impl ToSql for Value {
    /// Значение записывается в том типе, которого ждёт PostgreSQL для этого
    /// параметра: SQLite прощает число вместо текста и наоборот, а здесь
    /// типы строгие.
    fn to_sql(&self, ty: &Type, out: &mut BytesMut) -> Result<IsNull, Boxed> {
        let integer = *ty == Type::INT8 || *ty == Type::INT4 || *ty == Type::INT2;
        let real = *ty == Type::FLOAT8 || *ty == Type::FLOAT4 || *ty == Type::NUMERIC;
        let write_int = |n: i64, out: &mut BytesMut| -> Result<IsNull, Boxed> {
            if *ty == Type::INT4 {
                i32::try_from(n)?.to_sql(ty, out)
            } else if *ty == Type::INT2 {
                i16::try_from(n)?.to_sql(ty, out)
            } else {
                n.to_sql(&Type::INT8, out)
            }
        };
        match self {
            Self::Null => Ok(IsNull::Yes),
            Self::Int(n) if integer => write_int(*n, out),
            #[allow(clippy::cast_precision_loss, reason = "столбец вещественный")]
            Self::Int(n) if real => (*n as f64).to_sql(&Type::FLOAT8, out),
            Self::Int(n) if *ty == Type::BOOL => (*n != 0).to_sql(ty, out),
            Self::Int(n) => n.to_string().to_sql(&Type::TEXT, out),
            Self::Real(x) if real => x.to_sql(&Type::FLOAT8, out),
            #[allow(
                clippy::cast_possible_truncation,
                reason = "проверено, что дробной части нет"
            )]
            Self::Real(x) if integer && x.fract() == 0.0 => write_int(*x as i64, out),
            Self::Real(x) if integer => Err(format!("дробное число {x} для целого столбца").into()),
            Self::Real(x) => x.to_string().to_sql(&Type::TEXT, out),
            Self::Text(text) if integer => write_int(text.trim().parse()?, out),
            Self::Text(text) if real => text.trim().parse::<f64>()?.to_sql(&Type::FLOAT8, out),
            Self::Text(text) if *ty == Type::BYTEA => text.as_bytes().to_sql(ty, out),
            Self::Text(text) => text.to_sql(&Type::TEXT, out),
            Self::Blob(bytes) => bytes.as_slice().to_sql(&Type::BYTEA, out),
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

/// Число `NUMERIC` в двоичной записи PostgreSQL (его возвращают `sum` и `avg`).
fn numeric(raw: &[u8]) -> Option<f64> {
    let field = |at: usize| {
        raw.get(at..at + 2)
            .map(|pair| i16::from_be_bytes([pair[0], pair[1]]))
    };
    let (count, weight, sign) = (field(0)?, field(2)?, field(4)?);
    let mut value = 0f64;
    for index in 0..count {
        let digit = field(8 + usize::try_from(index).ok()? * 2)?;
        value += f64::from(digit) * 10_000f64.powi(i32::from(weight) - i32::from(index));
    }
    Some(if sign == 0x4000 { -value } else { value })
}

impl<'a> FromSql<'a> for Value {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self, Boxed> {
        Ok(if *ty == Type::INT8 {
            Self::Int(i64::from_sql(ty, raw)?)
        } else if *ty == Type::INT4 {
            Self::Int(i64::from(i32::from_sql(ty, raw)?))
        } else if *ty == Type::INT2 {
            Self::Int(i64::from(i16::from_sql(ty, raw)?))
        } else if *ty == Type::OID {
            Self::Int(i64::from(u32::from_sql(ty, raw)?))
        } else if *ty == Type::BOOL {
            Self::Int(i64::from(bool::from_sql(ty, raw)?))
        } else if *ty == Type::FLOAT8 {
            Self::Real(f64::from_sql(ty, raw)?)
        } else if *ty == Type::FLOAT4 {
            Self::Real(f64::from(f32::from_sql(ty, raw)?))
        } else if *ty == Type::NUMERIC {
            let value = numeric(raw).ok_or("некорректное значение numeric")?;
            #[allow(
                clippy::cast_possible_truncation,
                reason = "проверено, что дробной части нет"
            )]
            if value.fract() == 0.0 && value.abs() < 9.0e15 {
                Self::Int(value as i64)
            } else {
                Self::Real(value)
            }
        } else if *ty == Type::BYTEA {
            Self::Blob(raw.to_vec())
        } else {
            Self::Text(String::from_utf8_lossy(raw).into_owned())
        })
    }

    fn from_sql_null(_: &Type) -> Result<Self, Boxed> {
        Ok(Self::Null)
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

/// Транзакция PostgreSQL за общим интерфейсом.
pub(super) struct PgTx<'a, 'b> {
    pub(super) tx: &'a mut postgres::Transaction<'b>,
    /// Уже переведённые запросы: текст обработчика → текст PostgreSQL.
    pub(super) translated: &'a mut HashMap<String, String>,
}

impl PgTx<'_, '_> {
    fn statement(&mut self, sql: &str) -> DbResult<postgres::Statement> {
        if !self.translated.contains_key(sql) {
            if self.translated.len() > 4096 {
                self.translated.clear();
            }
            self.translated.insert(sql.to_owned(), translate(sql));
        }
        let text = self.translated.get(sql).map_or(sql, String::as_str);
        self.tx.prepare(text).map_err(pg)
    }
}

fn arguments(params: &[Value]) -> Vec<&(dyn ToSql + Sync)> {
    params
        .iter()
        .map(|value| value as &(dyn ToSql + Sync))
        .collect()
}

impl Tx for PgTx<'_, '_> {
    fn query(&mut self, sql: &str, params: &[Value]) -> DbResult<Vec<Row>> {
        let statement = self.statement(sql)?;
        let columns: Arc<[String]> = statement
            .columns()
            .iter()
            .map(|column| column.name().to_owned())
            .collect();
        let rows = self.tx.query(&statement, &arguments(params)).map_err(pg)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let mut values = Vec::with_capacity(columns.len());
            for index in 0..columns.len() {
                values.push(row.try_get::<_, Value>(index).map_err(pg)?);
            }
            out.push(Row {
                columns: Arc::clone(&columns),
                values,
            });
        }
        Ok(out)
    }

    fn execute(&mut self, sql: &str, params: &[Value]) -> DbResult<u64> {
        let statement = self.statement(sql)?;
        self.tx.execute(&statement, &arguments(params)).map_err(pg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_rewritten_outside_literals() {
        assert_eq!(
            translate(
                "SELECT `a` FROM t WHERE x = ? AND name LIKE ? AND note = 'what? LIKE `this`' LIMIT ?"
            ),
            "SELECT \"a\" FROM t WHERE x = $1 AND name ILIKE $2 AND note = 'what? LIKE `this`' ORDER BY t.bs_seq LIMIT $3"
        );
        assert_eq!(
            translate("CAST(id AS INTEGER) > CAST(? AS INTEGER)"),
            "CAST(id AS BIGINT) > CAST($1 AS BIGINT)"
        );
        assert_eq!(
            translate("x NOT LIKE 'a%' AND likes = ?"),
            "x NOT ILIKE 'a%' AND likes = $1"
        );
        assert_eq!(
            translate("UPDATE t SET integer_col = ?"),
            "UPDATE t SET integer_col = $1"
        );
    }

    #[test]
    fn unordered_selects_follow_insertion_order() {
        assert_eq!(
            translate("SELECT a FROM t WHERE b = ?"),
            "SELECT a FROM t WHERE b = $1 ORDER BY t.bs_seq"
        );
        assert_eq!(
            translate(
                "SELECT m.a FROM members m JOIN users u ON u.id = m.user_id WHERE m.x = ? LIMIT ?"
            ),
            "SELECT m.a FROM members m JOIN users u ON u.id = m.user_id WHERE m.x = $1 ORDER BY m.bs_seq LIMIT $2"
        );
        assert_eq!(
            translate("SELECT a FROM `t` AS x"),
            "SELECT a FROM \"t\" AS x ORDER BY x.bs_seq"
        );
        let keys = HashMap::from([(
            "members".to_owned(),
            vec!["room".to_owned(), "user_id".to_owned()],
        )]);
        assert_eq!(
            translate_with(
                "SELECT u.a FROM members m JOIN users u ON u.id = m.user_id WHERE m.room = ?",
                Some(&keys)
            ),
            "SELECT u.a FROM members m JOIN users u ON u.id = m.user_id WHERE m.room = $1 ORDER BY m.room, m.user_id"
        );
        assert_eq!(
            translate_with("SELECT room FROM members WHERE user_id = ?", Some(&keys)),
            "SELECT room FROM members WHERE user_id = $1 ORDER BY members.bs_seq"
        );
        for kept in [
            "SELECT a FROM t ORDER BY a",
            "SELECT count(*) AS n FROM t",
            "SELECT DISTINCT a FROM t",
            "SELECT a, count(*) FROM t GROUP BY a",
            "SELECT 1",
            "SELECT name FROM bs_migrations WHERE name = $1",
            "SELECT CAST(COALESCE(sum(size), 0) AS BIGINT) AS n FROM t",
            "DELETE FROM t",
        ] {
            assert_eq!(translate(kept), kept);
        }
        assert_eq!(
            translate(
                "SELECT a, (SELECT count(*) FROM x WHERE x.t = t.id) AS n FROM t WHERE note = 'FROM z ORDER BY'"
            ),
            "SELECT a, (SELECT count(*) FROM x WHERE x.t = t.id) AS n FROM t WHERE note = 'FROM z ORDER BY' ORDER BY t.bs_seq"
        );
    }

    #[test]
    fn schema_follows_the_original_migrations() -> DbResult<()> {
        let mut sqlite = rusqlite::Connection::open_in_memory()?;
        super::super::sqlite::migrate(&mut sqlite)?;
        let schema = read_schema(&sqlite)?;
        assert!(schema.tables.len() >= 40);
        let users = schema
            .tables
            .iter()
            .find(|(table, _)| table == "users")
            .map(|(_, columns)| columns);
        let created =
            users.and_then(|columns| columns.iter().find(|column| column.name == "created_at"));
        assert_eq!(created.map(|column| column.kind), Some("BIGINT"));
        assert!(
            schema
                .foreign_keys
                .iter()
                .any(|(_, table, statement)| table == "space_members"
                    && statement.contains("ON DELETE CASCADE"))
        );
        let unread = schema
            .indexes
            .iter()
            .find(|(name, _)| name == "idx_messages_channel_unread")
            .map(|(_, sql)| sql.as_str());
        assert_eq!(
            unread,
            Some(
                "CREATE INDEX IF NOT EXISTS \"idx_messages_channel_unread\" ON \"messages\" (\"channel_id\",(CAST(id AS BIGINT)),\"user_id\",\"type\")"
            )
        );
        let order = tables_in_dependency_order(&sqlite)?;
        let at = |name: &str| order.iter().position(|(table, _)| table == name);
        assert!(
            at("users") < at("space_members")
                && at("spaces") < at("channels")
                && at("channels") < at("messages")
        );
        Ok(())
    }

    #[test]
    fn numeric_values() {
        // 12345.5: цифры по основанию 10000 — 1, 2345, 5000; вес первой — 1.
        let raw = [0, 3, 0, 1, 0, 0, 0, 1, 0, 1, 0x09, 0x29, 0x13, 0x88];
        assert_eq!(numeric(&raw), Some(12345.5));
        assert_eq!(numeric(&[0, 0, 0, 0, 0, 0, 0, 0]), Some(0.0));
        assert_eq!(numeric(&[0, 1]), None);
    }
}
