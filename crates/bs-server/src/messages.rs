//! Сообщения каналов: вид на проводе и общие проверки.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::db::{DbResult, Row, Tx};
use crate::mentions;
use crate::params;
use crate::routes::users::{sanitize_user, user_row};

/// Наибольшая длина текста сообщения, в единицах UTF-16.
pub const MAX_MESSAGE_LENGTH: usize = 4000;

/// Столбцы сообщения канала.
pub const MESSAGE_COLUMNS: &str =
    "id, channel_id, user_id, reply_to_id, content, edited_at, created_at, type";

const ATTACHMENT_COLUMNS: &str = "id, message_id, dm_message_id, filename, original_name, mimetype, \
    size, thumbnail_filename, width, height, duration, playable, federation_status, \
    federation_meta, created_at";

/// Вложение на проводе. `federation` добавляет поля доставки между инстансами.
///
/// # Errors
/// В строке нет нужных столбцов.
pub fn attachment_json(
    row: &Row,
    owner_key: &str,
    owner_id: &str,
    federation: bool,
) -> DbResult<Value> {
    let stored_owner = if owner_key == "dmMessageId" {
        row.opt_text("dm_message_id")?
    } else {
        row.opt_text("message_id")?
    };
    let mut attachment = json!({
        "id": row.text("id")?,
        "filename": row.text("filename")?,
        "originalName": row.text("original_name")?,
        "mimetype": row.text("mimetype")?,
        "size": row.int("size")?,
        "thumbnailFilename": row.opt_text("thumbnail_filename")?,
        "width": row.opt_number("width")?,
        "height": row.opt_number("height")?,
        "duration": row.opt_number("duration")?,
        "playable": row.opt_number("playable")?,
        "createdAt": row.int("created_at")?,
    });
    attachment[owner_key] = json!(stored_owner.unwrap_or_else(|| owner_id.to_owned()));
    if federation {
        attachment["federationStatus"] = json!(row.opt_text("federation_status")?);
        attachment["federationMeta"] = json!(row.opt_text("federation_meta")?);
    }
    Ok(attachment)
}

/// Вложения сообщения канала.
///
/// # Errors
/// Ошибка базы.
pub fn attachments(tx: &mut dyn Tx, message_id: &str, federation: bool) -> DbResult<Vec<Value>> {
    tx.query(
        &format!("SELECT {ATTACHMENT_COLUMNS} FROM attachments WHERE message_id = ?"),
        params![message_id],
    )?
    .iter()
    .map(|row| attachment_json(row, "messageId", message_id, federation))
    .collect()
}

/// Вложения личного сообщения.
///
/// # Errors
/// Ошибка базы.
pub fn dm_attachments(tx: &mut dyn Tx, message_id: &str, federation: bool) -> DbResult<Vec<Value>> {
    tx.query(
        &format!("SELECT {ATTACHMENT_COLUMNS} FROM attachments WHERE dm_message_id = ?"),
        params![message_id],
    )?
    .iter()
    .map(|row| attachment_json(row, "dmMessageId", message_id, federation))
    .collect()
}

/// Встраивание на проводе (`embedRowToEmbed`).
///
/// # Errors
/// В строке нет нужных столбцов.
pub fn embed_json(row: &Row) -> DbResult<Value> {
    Ok(json!({
        "id": row.text("id")?,
        "messageId": row.opt_text("message_id")?,
        "dmMessageId": row.opt_text("dm_message_id")?,
        "url": row.text("url")?,
        "embedType": row.text("embed_type")?,
        "provider": row.opt_text("provider")?,
        "title": row.opt_text("title")?,
        "description": row.opt_text("description")?,
        "image": row.opt_text("image")?,
        "embedUrl": row.opt_text("embed_url")?,
        "width": row.opt_number("width")?,
        "height": row.opt_number("height")?,
        "color": row.opt_text("color")?,
        "createdAt": row.int("created_at")?,
    }))
}

/// Столбцы встраивания.
pub const EMBED_COLUMNS: &str = "id, message_id, dm_message_id, url, embed_type, provider, title, \
    description, image, embed_url, width, height, color, created_at";

/// Встраивания сообщения (канала или личного — по `column`).
///
/// # Errors
/// Ошибка базы.
pub fn embeds(tx: &mut dyn Tx, column: &str, message_id: &str) -> DbResult<Vec<Value>> {
    tx.query(
        &format!("SELECT {EMBED_COLUMNS} FROM embeds WHERE {column} = ?"),
        params![message_id],
    )?
    .iter()
    .map(embed_json)
    .collect()
}

/// Реакции сообщения. `with_user` добавляет автора реакции (так отвечает REST).
/// Порядок — как у SQLite оригинала, который читает их по уникальному индексу
/// `(message_id, user_id, emoji)`.
///
/// # Errors
/// Ошибка базы.
pub fn reactions(
    tx: &mut dyn Tx,
    dm: bool,
    message_id: &str,
    with_user: bool,
) -> DbResult<Vec<Value>> {
    let sql = if dm {
        "SELECT id, dm_message_id AS message_id, user_id, emoji, created_at FROM dm_reactions \
         WHERE dm_message_id = ? ORDER BY user_id, emoji"
    } else {
        "SELECT id, message_id, user_id, emoji, created_at FROM reactions WHERE message_id = ? \
         ORDER BY user_id, emoji"
    };
    let rows = tx.query(sql, params![message_id])?;
    let mut users: HashMap<String, Option<Value>> = HashMap::new();
    let mut list = Vec::with_capacity(rows.len());
    for row in rows {
        let user_id = row.text("user_id")?;
        let mut reaction = json!({
            "id": row.text("id")?,
            "messageId": row.text("message_id")?,
            "userId": user_id,
            "emoji": row.text("emoji")?,
            "createdAt": row.int("created_at")?,
        });
        if with_user {
            if !users.contains_key(&user_id) {
                let user = user_row(tx, &user_id)?
                    .map(|user| sanitize_user(&user, false))
                    .transpose()?;
                users.insert(user_id.clone(), user);
            }
            if let Some(Some(user)) = users.get(&user_id) {
                reaction["user"] = user.clone();
            }
        }
        list.push(reaction);
    }
    Ok(list)
}

/// Сообщение, на которое отвечают, в сокращённом виде (`fetchReplyToMessages`).
fn reply_target(tx: &mut dyn Tx, channel_id: &str, reply_to_id: &str) -> DbResult<Value> {
    let Some(target) = tx.query_one(
        &format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id = ? AND channel_id = ?"),
        params![reply_to_id, channel_id],
    )?
    else {
        return Ok(Value::Null);
    };
    let Some(user) = user_row(tx, &target.text("user_id")?)? else {
        return Ok(Value::Null);
    };
    let id = target.text("id")?;
    Ok(json!({
        "channelId": target.text("channel_id")?,
        "userId": target.text("user_id")?,
        "replyToId": target.opt_text("reply_to_id")?,
        "content": target.opt_text("content")?,
        "type": target.text("type")?,
        "editedAt": target.opt_int("edited_at")?,
        "createdAt": target.int("created_at")?,
        "user": sanitize_user(&user, false)?,
        "attachments": attachments(tx, &id, false)?,
        "embeds": [],
        "reactions": [],
        "replyTo": null,
        "id": id,
    }))
}

/// Откуда запрошено сообщение: от этого зависит набор полей.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Ответ REST (`buildMessageWithUser`): реакции с авторами, у вложений
    /// поля федерации, встраивания — как переданы.
    Rest,
    /// Событие WebSocket (`getMessageWithUser`): без того и другого.
    Event,
}

/// Сообщение канала с автором, вложениями, реакциями и ответом.
/// `with_embeds: false` отдаёт пустой список встраиваний (сразу после
/// создания и правки они ещё не посчитаны).
///
/// # Errors
/// Ошибка базы. `Ok(None)` — нет автора.
pub fn message_json(
    tx: &mut dyn Tx,
    message: &Row,
    shape: Shape,
    with_embeds: bool,
    with_reactions: bool,
) -> DbResult<Option<Value>> {
    let Some(user) = user_row(tx, &message.text("user_id")?)? else {
        return Ok(None);
    };
    let id = message.text("id")?;
    let channel_id = message.text("channel_id")?;
    let kind = message.text("type")?;
    let content = message.opt_text("content")?;
    let found = if kind == "system" {
        mentions::Mentions::default()
    } else {
        mentions::parse(content.as_deref())
    };
    let reply_to = match message.opt_text("reply_to_id")? {
        Some(target) => reply_target(tx, &channel_id, &target)?,
        None => Value::Null,
    };
    Ok(Some(json!({
        "channelId": channel_id,
        "userId": message.text("user_id")?,
        "replyToId": message.opt_text("reply_to_id")?,
        "content": content,
        "type": kind,
        "editedAt": message.opt_int("edited_at")?,
        "createdAt": message.int("created_at")?,
        "mentionEveryone": found.everyone,
        "mentionRoleIds": found.roles,
        "user": sanitize_user(&user, false)?,
        "attachments": attachments(tx, &id, shape == Shape::Rest)?,
        "embeds": if with_embeds { embeds(tx, "message_id", &id)? } else { Vec::new() },
        "reactions": if with_reactions { reactions(tx, false, &id, shape == Shape::Rest)? } else { Vec::new() },
        "replyTo": reply_to,
        "id": id,
    })))
}

/// Сообщение по идентификатору.
///
/// # Errors
/// Ошибка базы.
pub fn message_row(tx: &mut dyn Tx, id: &str) -> DbResult<Option<Row>> {
    tx.query_one(
        &format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id = ?"),
        params![id],
    )
}

/// Пространство канала.
///
/// # Errors
/// Ошибка базы.
pub fn channel_space(tx: &mut dyn Tx, channel_id: &str) -> DbResult<Option<String>> {
    tx.query_one(
        "SELECT space_id FROM channels WHERE id = ?",
        params![channel_id],
    )?
    .map(|row| row.text("space_id"))
    .transpose()
}

/// Можно ли ответить на сообщение: оно в этом канале и написано участником.
///
/// # Errors
/// Ошибка базы.
pub fn reply_target_in_channel(
    tx: &mut dyn Tx,
    channel_id: &str,
    reply_to_id: &str,
) -> DbResult<bool> {
    Ok(tx
        .query_one(
            "SELECT 1 AS one FROM messages WHERE id = ? AND channel_id = ? AND type = 'user'",
            params![reply_to_id, channel_id],
        )?
        .is_some())
}

/// Страница истории (`HistoryPage`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
    /// Последние сообщения.
    Latest(i64),
    /// Старше указанного.
    Before(String, i64),
    /// Новее указанного.
    After(String, i64),
}

/// Разбирает параметры `before`, `after`, `limit` (`parseHistoryPage`).
///
/// # Errors
/// Код ошибки: оба курсора сразу или повторённый параметр.
pub fn parse_page(query: &[(String, String)]) -> Result<Page, &'static str> {
    let mut limit = 50_i64;
    let mut cursors: [Option<String>; 2] = [None, None];
    let mut seen = [0_u8; 2];
    for (name, value) in query {
        match name.as_str() {
            "limit" => {
                let parsed = value.trim().parse::<f64>().unwrap_or(0.0);
                #[allow(clippy::cast_possible_truncation, reason = "значение зажато в 1..=100")]
                let wanted = if parsed.is_nan() || parsed == 0.0 {
                    50
                } else {
                    parsed.clamp(1.0, 100.0) as i64
                };
                limit = wanted.clamp(1, 100);
            }
            "before" | "after" => {
                let slot = usize::from(name == "after");
                seen[slot] += 1;
                if !value.is_empty() {
                    cursors[slot] = Some(value.clone());
                }
            }
            _ => {}
        }
    }
    // Повторённый параметр Fastify отдаёт массивом — это не строка.
    if seen.iter().any(|count| *count > 1) {
        return Err("validation_failed");
    }
    match cursors {
        [Some(_), Some(_)] => Err("paging_cursor_conflict"),
        [Some(before), None] => Ok(Page::Before(before, limit)),
        [None, Some(after)] => Ok(Page::After(after, limit)),
        [None, None] => Ok(Page::Latest(limit)),
    }
}

/// Сравнение идентификаторов-снежинок как чисел.
#[must_use]
pub fn compare_snowflakes(a: &str, b: &str) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// Выбирает страницу истории в хронологическом порядке (`selectHistoryPage`).
/// `table` и `scope` — таблица сообщений и столбец канала.
///
/// # Errors
/// Ошибка базы.
pub fn select_page(
    tx: &mut dyn Tx,
    columns: &str,
    table: &str,
    scope: &str,
    channel_id: &str,
    page: &Page,
) -> DbResult<Vec<Row>> {
    match page {
        Page::After(cursor, limit) => {
            let rows = tx.query(
                &format!(
                    "SELECT {columns} FROM {table} WHERE {scope} = ? AND id > ? ORDER BY id ASC LIMIT ?"
                ),
                params![channel_id, cursor, *limit],
            )?;
            let mut keyed: Vec<(i64, String, Row)> = Vec::with_capacity(rows.len());
            for row in rows {
                keyed.push((row.int("created_at")?, row.text("id")?, row));
            }
            keyed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| compare_snowflakes(&a.1, &b.1)));
            Ok(keyed.into_iter().map(|(_, _, row)| row).collect())
        }
        Page::Before(cursor, limit) => {
            let mut rows = tx.query(
                &format!(
                    "SELECT {columns} FROM {table} WHERE {scope} = ? AND id < ? \
                     ORDER BY created_at DESC, id DESC LIMIT ?"
                ),
                params![channel_id, cursor, *limit],
            )?;
            rows.reverse();
            Ok(rows)
        }
        Page::Latest(limit) => {
            let mut rows = tx.query(
                &format!(
                    "SELECT {columns} FROM {table} WHERE {scope} = ? \
                     ORDER BY created_at DESC, id DESC LIMIT ?"
                ),
                params![channel_id, *limit],
            )?;
            rows.reverse();
            Ok(rows)
        }
    }
}

/// Содержимое системного сообщения «тычок» (`channelSystemContent`).
#[must_use]
pub fn poke_content(target_user_id: &str, target_display_name: &str) -> String {
    format!(
        "{{\"event\":\"channel_poke\",\"targetUserId\":{},\"targetDisplayName\":{}}}",
        Value::from(target_user_id),
        Value::from(target_display_name)
    )
}

/// Имя для системных сообщений (`dmSystemName`): отображаемое, иначе имя
/// аккаунта без домена.
#[must_use]
pub fn system_name(display_name: Option<&str>, username: &str) -> String {
    if let Some(name) = display_name.filter(|name| !name.is_empty()) {
        return name.to_owned();
    }
    let handle = username.split('@').next().unwrap_or_default();
    if handle.is_empty() {
        "Unknown".to_owned()
    } else {
        handle.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn paging_parameters() {
        assert_eq!(parse_page(&[]), Ok(Page::Latest(50)));
        assert_eq!(
            parse_page(&query(&[("limit", "abc")])),
            Ok(Page::Latest(50))
        );
        assert_eq!(parse_page(&query(&[("limit", "0")])), Ok(Page::Latest(50)));
        assert_eq!(parse_page(&query(&[("limit", "-5")])), Ok(Page::Latest(1)));
        assert_eq!(
            parse_page(&query(&[("limit", "1000")])),
            Ok(Page::Latest(100))
        );
        assert_eq!(
            parse_page(&query(&[("before", "9"), ("limit", "7")])),
            Ok(Page::Before("9".into(), 7))
        );
        assert_eq!(
            parse_page(&query(&[("after", "9")])),
            Ok(Page::After("9".into(), 50))
        );
        assert_eq!(
            parse_page(&query(&[("after", ""), ("before", "")])),
            Ok(Page::Latest(50))
        );
        assert_eq!(
            parse_page(&query(&[("after", "1"), ("before", "2")])),
            Err("paging_cursor_conflict")
        );
        assert_eq!(
            parse_page(&query(&[("after", "1"), ("after", "2")])),
            Err("validation_failed")
        );
    }

    #[test]
    fn system_content_and_names() {
        assert_eq!(
            poke_content("1", "А \"Б\""),
            r#"{"event":"channel_poke","targetUserId":"1","targetDisplayName":"А \"Б\""}"#
        );
        assert_eq!(system_name(None, "bob@example.org"), "bob");
        assert_eq!(system_name(Some("Боб"), "bob"), "Боб");
        assert_eq!(system_name(Some(""), "@x"), "Unknown");
        assert_eq!(compare_snowflakes("9", "10"), std::cmp::Ordering::Less);
    }
}
