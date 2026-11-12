//! Поиск по сообщениям и «сообщения вокруг» (`routes/search.ts`).

use std::collections::HashSet;

use axum::Json;
use axum::extract::{Path, Query, State};
use bs_permissions::Permissions;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Row, Tx};
use crate::error::{ApiError, ApiResult};
use crate::messages::{MESSAGE_COLUMNS, Shape, channel_space, message_json};
use crate::state::AppState;
use crate::util::js_trim;
use crate::{dm, params, perms};

/// Параметры поиска.
#[derive(Debug, Default, serde::Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
    from: Option<String>,
    has: Option<String>,
    before: Option<String>,
    after: Option<String>,
    offset: Option<String>,
    limit: Option<String>,
}

/// `Number(text) || fallback`.
fn js_number(text: Option<&str>, fallback: f64) -> f64 {
    let parsed = text
        .map(js_trim)
        .and_then(|text| text.parse::<f64>().ok())
        .unwrap_or(f64::NAN);
    if parsed.is_nan() || parsed == 0.0 {
        fallback
    } else {
        parsed
    }
}

/// Дней от 1970-01-01 до даты (григорианский календарь).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Разбирает дату в формате ISO 8601 (`2024-05-01`, `2024-05-01T10:00:00Z`,
/// со смещением) в миллисекунды. Время без пояса считается временем UTC.
#[must_use]
pub fn parse_date(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, time) = match text.split_once(['T', ' ']) {
        Some((date, time)) => (date, Some(time)),
        None => (text, None),
    };
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next().map_or(Some(1), |m| m.parse().ok())?;
    let day: i64 = parts.next().map_or(Some(1), |d| d.parse().ok())?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut millis = days_from_civil(year, month, day) * 86_400_000;
    if let Some(time) = time {
        let (clock, offset) = if let Some(clock) = time.strip_suffix('Z') {
            (clock, 0)
        } else if let Some(index) = time.rfind(['+', '-']) {
            let (clock, zone) = time.split_at(index);
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (hours, minutes) = zone[1..]
                .split_once(':')
                .unwrap_or_else(|| (&zone[1..], "0"));
            let minutes = hours.parse::<i64>().ok()? * 60 + minutes.parse::<i64>().ok()?;
            (clock, sign * minutes * 60_000)
        } else {
            (time, 0)
        };
        let mut fields = clock.split(':');
        let hours: i64 = fields.next()?.parse().ok()?;
        let minutes: i64 = fields.next()?.parse().ok()?;
        let seconds: f64 = fields.next().map_or(Some(0.0), |s| s.parse().ok())?;
        if hours > 24 || minutes > 59 || !(0.0..60.0).contains(&seconds) {
            return None;
        }
        #[allow(clippy::cast_possible_truncation, reason = "секунды в пределах минуты")]
        let second_millis = (seconds * 1000.0).round() as i64;
        millis += hours * 3_600_000 + minutes * 60_000 + second_millis - offset;
    }
    Some(millis)
}

/// Условия поиска и их значения; `None` — автора с таким именем нет.
fn conditions(
    tx: &mut dyn Tx,
    query: &SearchQuery,
    table: &str,
    scope: &str,
    owner_column: &str,
    id: &str,
) -> DbResult<Option<(String, Vec<crate::db::Value>)>> {
    let mut filter = format!("{table}.{scope} = ?");
    let mut values: Vec<crate::db::Value> = vec![id.into()];
    if table == "messages" {
        filter.push_str(" AND messages.type = 'user'");
    }
    if let Some(q) = query.q.as_deref().map(js_trim).filter(|q| !q.is_empty()) {
        // Сообщения-стикеры (`sticker:<адрес>`) и зашифрованные сообщения в поиск
        // не попадают: совпадение с шифротекстом ничего не значит.
        filter.push_str(&format!(
            " AND {table}.content LIKE ? AND ({table}.content IS NULL OR {table}.content NOT LIKE \
             'sticker:http%://%/api/stickers/assets/%.webp') AND {table}.content NOT LIKE 'bs-e2ee:%'"
        ));
        values.push(format!("%{q}%").into());
    }
    if let Some(from) = query
        .from
        .as_deref()
        .map(js_trim)
        .filter(|from| !from.is_empty())
    {
        let Some(user) =
            tx.query_one("SELECT id FROM users WHERE username LIKE ?", params![from])?
        else {
            return Ok(None);
        };
        filter.push_str(&format!(" AND {table}.user_id = ?"));
        values.push(user.text("id")?.into());
    }
    if let Some(at) = query.before.as_deref().and_then(parse_date) {
        filter.push_str(&format!(" AND {table}.created_at < ?"));
        values.push(at.into());
    }
    if let Some(at) = query.after.as_deref().and_then(parse_date) {
        filter.push_str(&format!(" AND {table}.created_at > ?"));
        values.push(at.into());
    }
    match query.has.as_deref() {
        Some("file") => filter.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM attachments WHERE attachments.{owner_column} = {table}.id)"
        )),
        Some("image") => filter.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM attachments WHERE attachments.{owner_column} = {table}.id \
             AND attachments.mimetype LIKE 'image/%')"
        )),
        // В оригинале это условие добавляется уже после сборки запроса и не действует;
        // здесь фильтр работает, как задумано.
        Some("link") => filter.push_str(&format!(" AND {table}.content LIKE '%http%'")),
        _ => {}
    }
    Ok(Some((filter, values)))
}

fn found(
    tx: &mut dyn Tx,
    query: &SearchQuery,
    columns: &str,
    table: &str,
    scope: &str,
    owner_column: &str,
    id: &str,
) -> DbResult<(Vec<Row>, i64)> {
    #[allow(clippy::cast_possible_truncation, reason = "значения зажаты в пределы")]
    let offset = js_number(query.offset.as_deref(), 0.0).max(0.0) as i64;
    #[allow(clippy::cast_possible_truncation, reason = "значения зажаты в пределы")]
    let limit = js_number(query.limit.as_deref(), 25.0).clamp(1.0, 50.0) as i64;
    let Some((filter, mut values)) = conditions(tx, query, table, scope, owner_column, id)? else {
        return Ok((Vec::new(), 0));
    };
    let total = tx
        .query_one(
            &format!("SELECT count(*) AS n FROM {table} WHERE {filter}"),
            &values,
        )?
        .map_or(Ok(0), |row| row.int("n"))?;
    values.push(limit.into());
    values.push(offset.into());
    let rows = tx.query(
        &format!(
            "SELECT {columns} FROM {table} WHERE {filter} ORDER BY {table}.created_at DESC, {table}.id DESC \
             LIMIT ? OFFSET ?"
        ),
        &values,
    )?;
    Ok((rows, total))
}

fn require_history(tx: &mut dyn Tx, user: &str, channel: &str) -> ApiResult<()> {
    let space =
        channel_space(tx, channel)?.ok_or_else(|| ApiError::not_found("channel_not_found"))?;
    let needed = Permissions::VIEW_CHANNEL | Permissions::READ_MESSAGE_HISTORY;
    if !perms::has_channel_permission(tx, user, &space, channel, needed)? {
        return Err(perms::missing("READ_MESSAGE_HISTORY"));
    }
    Ok(())
}

fn hydrate_channel(tx: &mut dyn Tx, rows: &[Row]) -> DbResult<Vec<Value>> {
    let mut list = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(message) = message_json(tx, row, Shape::Rest, true, true)? {
            list.push(message);
        }
    }
    Ok(list)
}

fn hydrate_dm(tx: &mut dyn Tx, rows: &[Row]) -> DbResult<Vec<Value>> {
    let mut list = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(message) = dm::message_json(tx, row)? {
            list.push(message);
        }
    }
    Ok(list)
}

/// `GET /api/channels/:id/search`.
pub async fn channel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<SearchQuery>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            require_history(tx, &user.id, &id)?;
            let prefixed: Vec<String> = MESSAGE_COLUMNS
                .split(", ")
                .map(|column| format!("messages.{column}"))
                .collect();
            let (rows, total) = found(
                tx,
                &query,
                &prefixed.join(", "),
                "messages",
                "channel_id",
                "message_id",
                &id,
            )?;
            Ok(Json(
                json!({"results": hydrate_channel(tx, &rows)?, "totalCount": total}),
            ))
        })
        .await
}

/// `GET /api/dm/:id/search`.
pub async fn dm_channel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<SearchQuery>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            if !dm::is_member(tx, &id, &user.id)? {
                return Err(ApiError::forbidden("not_dm_member"));
            }
            let prefixed: Vec<String> = dm::DM_MESSAGE_COLUMNS
                .split(", ")
                .map(|column| format!("dm_messages.{column}"))
                .collect();
            let (rows, total) = found(
                tx,
                &query,
                &prefixed.join(", "),
                "dm_messages",
                "dm_channel_id",
                "dm_message_id",
                &id,
            )?;
            Ok(Json(
                json!({"results": hydrate_dm(tx, &rows)?, "totalCount": total}),
            ))
        })
        .await
}

/// Параметры «сообщений вокруг».
#[derive(Debug, serde::Deserialize)]
pub struct AroundQuery {
    #[serde(rename = "messageId")]
    message_id: Option<String>,
    limit: Option<String>,
}

/// Сообщения вокруг указанного: половина до него (с ним самим) и половина после.
fn around(
    tx: &mut dyn Tx,
    columns: &str,
    table: &str,
    scope: &str,
    id: &str,
    query: &AroundQuery,
) -> ApiResult<Vec<Row>> {
    #[allow(clippy::cast_possible_truncation, reason = "значение зажато в пределы")]
    let limit = js_number(query.limit.as_deref(), 50.0).clamp(1.0, 100.0) as i64;
    let half = limit / 2;
    let message_id = query.message_id.as_deref().unwrap_or_default();
    let target = tx.query_one(
        &format!("SELECT id FROM {table} WHERE id = ? AND {scope} = ?"),
        params![message_id, id],
    )?;
    if target.is_none() {
        return Err(ApiError::not_found("message_not_found"));
    }
    let mut before = tx.query(
        &format!(
            "SELECT {columns} FROM {table} WHERE {scope} = ? AND id <= ? \
             ORDER BY created_at DESC, id DESC LIMIT ?"
        ),
        params![id, message_id, half + 1],
    )?;
    before.reverse();
    let after = tx.query(
        &format!(
            "SELECT {columns} FROM {table} WHERE {scope} = ? AND id > ? ORDER BY created_at ASC, id ASC LIMIT ?"
        ),
        params![id, message_id, half],
    )?;
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for row in before.into_iter().chain(after) {
        if seen.insert(row.text("id")?) {
            rows.push(row);
        }
    }
    Ok(rows)
}

/// `GET /api/channels/:id/messages/around`.
pub async fn channel_around(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<AroundQuery>,
) -> ApiResult<Json<Value>> {
    if query.message_id.as_deref().is_none_or(str::is_empty) {
        return Err(ApiError::bad_request("validation_failed"));
    }
    state
        .run(move |tx, _| {
            require_history(tx, &user.id, &id)?;
            let rows = around(tx, MESSAGE_COLUMNS, "messages", "channel_id", &id, &query)?;
            Ok(Json(json!(hydrate_channel(tx, &rows)?)))
        })
        .await
}

/// `GET /api/dm/:id/messages/around`.
pub async fn dm_around(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<AroundQuery>,
) -> ApiResult<Json<Value>> {
    if query.message_id.as_deref().is_none_or(str::is_empty) {
        return Err(ApiError::bad_request("validation_failed"));
    }
    state
        .run(move |tx, _| {
            if !dm::is_member(tx, &id, &user.id)? {
                return Err(ApiError::forbidden("not_dm_member"));
            }
            let rows = around(
                tx,
                dm::DM_MESSAGE_COLUMNS,
                "dm_messages",
                "dm_channel_id",
                &id,
                &query,
            )?;
            Ok(Json(json!(hydrate_dm(tx, &rows)?)))
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(parse_date("2024-05-01"), Some(1_714_521_600_000));
        assert_eq!(parse_date("2024-05-01T10:00:00Z"), Some(1_714_557_600_000));
        assert_eq!(
            parse_date("2024-05-01T12:00:00+02:00"),
            Some(1_714_557_600_000)
        );
        assert_eq!(
            parse_date("2024-05-01T10:00:00.250Z"),
            Some(1_714_557_600_250)
        );
        assert_eq!(parse_date("yesterday"), None);
        assert_eq!(parse_date("2024-13-01"), None);
    }
}
