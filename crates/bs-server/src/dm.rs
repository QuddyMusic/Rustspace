//! Личные переписки: поиск и создание, сообщения, участники.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::db::{DbResult, Row, Tx};
use crate::error::{ApiError, ApiResult};
use crate::hub::Outbox;
use crate::messages::{MAX_MESSAGE_LENGTH, dm_attachments, embeds, reactions, system_name};
use crate::ready::{DM_CHANNEL_COLUMNS, dm_channel_view};
use crate::routes::users::{sanitize_user, user_row};
use crate::snowflake::Snowflake;
use crate::state::now_ms;
use crate::util::{js_trim, utf16_len};
use crate::{broadcast, federation, params};

/// Столбцы личного сообщения.
pub const DM_MESSAGE_COLUMNS: &str = "id, dm_channel_id, user_id, reply_to_id, content, edited_at, \
    created_at, type, source_message_id, source_instance";

/// Наибольшее число участников группы.
pub const GROUP_MAX_MEMBERS: usize = 10;

/// Состоит ли пользователь в переписке.
///
/// # Errors
/// Ошибка базы.
pub fn is_member(tx: &mut dyn Tx, dm_channel_id: &str, user_id: &str) -> DbResult<bool> {
    Ok(tx
        .query_one(
            "SELECT 1 AS one FROM dm_members WHERE dm_channel_id = ? AND user_id = ?",
            params![dm_channel_id, user_id],
        )?
        .is_some())
}

/// Переписка один на один, где все собеседники удалены (`isDeadOneOnOne`).
///
/// # Errors
/// Ошибка базы.
pub fn is_dead_one_on_one(tx: &mut dyn Tx, dm_channel_id: &str, requester: &str) -> DbResult<bool> {
    let Some(channel) = tx.query_one(
        "SELECT owner_id FROM dm_channels WHERE id = ?",
        params![dm_channel_id],
    )?
    else {
        return Ok(false);
    };
    if channel.opt_text("owner_id")?.is_some() {
        return Ok(false);
    }
    let others = tx.query(
        "SELECT u.is_deleted FROM dm_members m JOIN users u ON u.id = m.user_id \
         WHERE m.dm_channel_id = ? AND m.user_id <> ?",
        params![dm_channel_id, requester],
    )?;
    if others.is_empty() {
        return Ok(false);
    }
    for row in &others {
        if row.opt_flag("is_deleted")? != Some(true) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Живая (не удалённая) переписка.
///
/// # Errors
/// Ошибка базы.
pub fn channel_row(tx: &mut dyn Tx, id: &str) -> DbResult<Option<Row>> {
    tx.query_one(
        &format!(
            "SELECT {DM_CHANNEL_COLUMNS} FROM dm_channels WHERE id = ? AND deleted_at IS NULL"
        ),
        params![id],
    )
}

/// Переписка на проводе (`loadDmChannelWire`).
///
/// # Errors
/// Ошибка базы.
pub fn channel_wire(tx: &mut dyn Tx, id: &str) -> DbResult<Option<Value>> {
    match channel_row(tx, id)? {
        Some(row) => dm_channel_view(tx, &row).map(Some),
        None => Ok(None),
    }
}

/// Добавляет участника (`insertDmMember`).
///
/// # Errors
/// Ошибка базы.
pub fn insert_member(
    tx: &mut dyn Tx,
    channel: &str,
    user: &str,
    closed: bool,
    at: i64,
) -> DbResult<()> {
    tx.execute(
        "INSERT INTO dm_members (dm_channel_id, user_id, closed, closed_changed_at) VALUES (?, ?, ?, ?)",
        params![channel, user, closed, at],
    )?;
    Ok(())
}

/// Закрывает или открывает переписку для участника; `true` — состояние
/// изменилось (`setDmMemberClosed`).
///
/// # Errors
/// Ошибка базы.
pub fn set_closed(tx: &mut dyn Tx, channel: &str, user: &str, closed: bool) -> DbResult<bool> {
    Ok(tx.execute(
        "UPDATE dm_members SET closed = ?, closed_changed_at = ? \
         WHERE dm_channel_id = ? AND user_id = ? AND closed <> ?",
        params![closed, now_ms(), channel, user, closed],
    )? > 0)
}

/// Переносит позицию чтения, оставляя более новую (`keepNewerReadPointer`).
///
/// # Errors
/// Ошибка базы.
pub fn keep_newer_read_pointer(
    tx: &mut dyn Tx,
    from: (&str, &str),
    to: (&str, &str),
) -> DbResult<()> {
    if from == to {
        return Ok(());
    }
    let read = |tx: &mut dyn Tx, (user, channel): (&str, &str)| {
        tx.query_one(
            "SELECT last_read_message_id, updated_at FROM read_states WHERE user_id = ? AND channel_id = ?",
            params![user, channel],
        )
    };
    let Some(moving) = read(tx, from)? else {
        return Ok(());
    };
    tx.execute(
        "DELETE FROM read_states WHERE user_id = ? AND channel_id = ?",
        params![from.0, from.1],
    )?;
    let last = moving.text("last_read_message_id")?;
    let updated = moving.opt_int("updated_at")?.unwrap_or(0);
    let Some(held) = read(tx, to)? else {
        tx.execute(
            "INSERT INTO read_states (user_id, channel_id, last_read_message_id, updated_at) VALUES (?, ?, ?, ?)",
            params![to.0, to.1, &last, updated],
        )?;
        return Ok(());
    };
    let held_last = held.text("last_read_message_id")?;
    let newer = match (last.parse::<u128>(), held_last.parse::<u128>()) {
        (Ok(a), Ok(b)) => a > b,
        _ => last > held_last,
    };
    if newer {
        tx.execute(
            "UPDATE read_states SET last_read_message_id = ?, updated_at = ? WHERE user_id = ? AND channel_id = ?",
            params![&last, updated.max(held.opt_int("updated_at")?.unwrap_or(0)), to.0, to.1],
        )?;
    }
    Ok(())
}

/// Итог сверки ключа переписки (`DmReconcileResult`).
#[derive(Debug, Clone)]
pub struct Reconciled {
    /// Слита в другую (`merged`), а не перезаписан ключ (`rekeyed`).
    pub merged: bool,
    /// Исходная переписка.
    pub channel: String,
    /// Переписка, которая осталась.
    pub target: String,
    /// Кого это касается.
    pub affected: Vec<String>,
}

/// Может ли строка быть перепиской один на один (`mayBeOneOnOneRow`).
fn may_be_one_on_one(owner: Option<&str>, federated: Option<&str>) -> bool {
    owner.is_none()
        && federated.is_none_or(|key| {
            key.len() == 32
                && key
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

/// Сверяет ключ переписки один на один с её участниками
/// (`reconcileDmChannelFederatedId`): перезаписывает ключ или сливает с
/// перепиской, у которой он уже есть.
///
/// # Errors
/// Ошибка базы.
pub fn reconcile_federated_id(tx: &mut dyn Tx, channel: &str) -> DbResult<Option<Reconciled>> {
    let Some(row) = tx.query_one(
        "SELECT id, federated_id, owner_id FROM dm_channels WHERE id = ? AND deleted_at IS NULL",
        params![channel],
    )?
    else {
        return Ok(None);
    };
    let federated = row.opt_text("federated_id")?;
    if !may_be_one_on_one(row.opt_text("owner_id")?.as_deref(), federated.as_deref()) {
        return Ok(None);
    }
    let members_of = |tx: &mut dyn Tx, channel: &str| -> DbResult<Vec<(String, String, bool)>> {
        tx.query(
            "SELECT m.user_id, u.home_user_id, m.closed FROM dm_members m JOIN users u ON u.id = m.user_id \
             WHERE m.dm_channel_id = ?",
            params![channel],
        )?
        .iter()
        .map(|row| {
            let user = row.text("user_id")?;
            let identity = row
                .opt_text("home_user_id")?
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| user.clone());
            Ok((user, identity, row.opt_flag("closed")? == Some(true)))
        })
        .collect()
    };
    let members = members_of(tx, channel)?;
    if members.len() != 2 {
        return Ok(None);
    }
    let expected = one_on_one_key(&members[0].1, &members[1].1);
    if federated.as_deref() == Some(expected.as_str()) {
        return Ok(None);
    }
    let target = tx.query_one(
        "SELECT id FROM dm_channels WHERE federated_id = ? AND id <> ?",
        params![&expected, channel],
    )?;
    let Some(target) = target else {
        tx.execute(
            "UPDATE dm_channels SET federated_id = ? WHERE id = ?",
            params![&expected, channel],
        )?;
        return Ok(Some(Reconciled {
            merged: false,
            channel: channel.to_owned(),
            target: channel.to_owned(),
            affected: members.iter().map(|m| m.0.clone()).collect(),
        }));
    };
    let target = target.text("id")?;
    let target_members = members_of(tx, &target)?;
    let mut affected: Vec<String> = Vec::new();
    for user in members.iter().chain(target_members.iter()).map(|m| &m.0) {
        if !affected.contains(user) {
            affected.push(user.clone());
        }
    }
    tx.execute(
        "UPDATE dm_messages SET dm_channel_id = ? WHERE dm_channel_id = ?",
        params![&target, channel],
    )?;
    let mut kept: Vec<(String, String)> = Vec::new();
    for (user, identity, closed) in &members {
        if let Some(held) = target_members.iter().find(|t| &t.1 == identity) {
            if !closed && held.2 {
                set_closed(tx, &target, &held.0, false)?;
            }
            tx.execute(
                "DELETE FROM dm_members WHERE dm_channel_id = ? AND user_id = ?",
                params![channel, user],
            )?;
            kept.push((user.clone(), held.0.clone()));
        } else {
            tx.execute(
                "UPDATE dm_members SET dm_channel_id = ? WHERE dm_channel_id = ? AND user_id = ?",
                params![&target, channel, user],
            )?;
        }
    }
    for row in tx.query(
        "SELECT user_id FROM read_states WHERE channel_id = ?",
        params![channel],
    )? {
        let user = row.text("user_id")?;
        let to = kept
            .iter()
            .find(|(from, _)| *from == user)
            .map_or_else(|| user.clone(), |(_, to)| to.clone());
        keep_newer_read_pointer(tx, (&user, channel), (&to, &target))?;
    }
    tx.execute(
        "UPDATE federation_mutation_log SET context_id = ? WHERE context_id = ? AND context_type = 'dm'",
        params![&target, channel],
    )?;
    tx.execute(
        "UPDATE federation_outbox SET context_id = ? WHERE context_id = ? AND context_type = 'dm'",
        params![&target, channel],
    )?;
    tx.execute("DELETE FROM dm_channels WHERE id = ?", params![channel])?;
    Ok(Some(Reconciled {
        merged: true,
        channel: channel.to_owned(),
        target,
        affected,
    }))
}

/// Объявляет итоги сверки ключей (`announceDmReconcile`).
///
/// # Errors
/// Ошибка базы.
pub fn announce_reconciled(
    tx: &mut dyn Tx,
    out: &mut crate::hub::Outbox,
    results: &[Reconciled],
) -> DbResult<()> {
    for result in results {
        if result.merged {
            for user in &result.affected {
                crate::broadcast::to_user(
                    tx,
                    out,
                    user,
                    &serde_json::json!({"type": "dm_channel_closed", "dmChannelId": result.channel}),
                )?;
            }
        }
        let Some(wire) = channel_wire(tx, &result.target)? else {
            continue;
        };
        let event = serde_json::json!({"type": "dm_channel_created", "dmChannel": wire});
        for row in tx.query(
            "SELECT user_id FROM dm_members WHERE dm_channel_id = ? AND closed = 0",
            params![&result.target],
        )? {
            crate::broadcast::to_user(tx, out, &row.text("user_id")?, &event)?;
        }
    }
    Ok(())
}

/// Ключ переписки один на один: одинаков на всех инстансах (`oneOnOneKey`).
#[must_use]
pub fn one_on_one_key(a: &str, b: &str) -> String {
    let mut pair = [a, b];
    pair.sort_unstable();
    let digest = Sha256::digest(pair.join(":").as_bytes());
    hex::encode(digest)[..32].to_owned()
}

fn home_identity(tx: &mut dyn Tx, user_id: &str) -> DbResult<String> {
    Ok(tx
        .query_one(
            "SELECT home_user_id FROM users WHERE id = ?",
            params![user_id],
        )?
        .map(|row| row.opt_text("home_user_id"))
        .transpose()?
        .flatten()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| user_id.to_owned()))
}

/// Находит или создаёт переписку один на один; собеседник до первого
/// сообщения её не видит. Возвращает идентификатор и признак создания
/// (`findOrCreateOneOnOne` с `open: 'first'`).
///
/// # Errors
/// Ошибка базы.
pub fn find_or_create_one_on_one(
    tx: &mut dyn Tx,
    ids: &Snowflake,
    caller: &str,
    target: &str,
) -> DbResult<(String, bool)> {
    find_or_create_pair(tx, ids, caller, target, false)
}

/// Как [`find_or_create_one_on_one`]; `open_both` — оба участника видят
/// переписку сразу (`open: 'both'`, переписка создаётся пришедшим сообщением).
///
/// # Errors
/// Ошибка базы.
pub fn find_or_create_pair(
    tx: &mut dyn Tx,
    ids: &Snowflake,
    caller: &str,
    target: &str,
    open_both: bool,
) -> DbResult<(String, bool)> {
    let key = one_on_one_key(&home_identity(tx, caller)?, &home_identity(tx, target)?);
    let keyed = tx
        .query_one(
            "SELECT id FROM dm_channels WHERE federated_id = ?",
            params![&key],
        )?
        .map(|row| row.text("id"))
        .transpose()?;
    if let Some(channel) = keyed {
        for (party, closed) in [(caller, false), (target, !open_both)] {
            if !is_member(tx, &channel, party)? {
                insert_member(tx, &channel, party, closed, now_ms())?;
            }
        }
        return Ok((channel, false));
    }
    // Переписка, созданная до появления ключей: та же пара, ключа нет.
    let legacy = tx.query(
        "SELECT c.id, c.federated_id FROM dm_channels c \
         WHERE c.owner_id IS NULL AND c.deleted_at IS NULL \
         AND EXISTS (SELECT 1 FROM dm_members m WHERE m.dm_channel_id = c.id AND m.user_id = ?) \
         AND EXISTS (SELECT 1 FROM dm_members m WHERE m.dm_channel_id = c.id AND m.user_id = ?) \
         AND (SELECT count(*) FROM dm_members m WHERE m.dm_channel_id = c.id) = 2 \
         ORDER BY c.created_at, c.id",
        params![caller, target],
    )?;
    for row in legacy {
        let shaped = row.opt_text("federated_id")?.is_none_or(|id| {
            id.len() == 32
                && id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
        if shaped {
            let id = row.text("id")?;
            tx.execute(
                "UPDATE dm_channels SET federated_id = ? WHERE id = ?",
                params![&key, &id],
            )?;
            return Ok((id, false));
        }
    }
    let channel = ids.next();
    let now = now_ms();
    tx.execute(
        "INSERT INTO dm_channels (id, owner_id, federated_id, created_at) VALUES (?, NULL, ?, ?)",
        params![&channel, &key, now],
    )?;
    insert_member(tx, &channel, caller, false, now)?;
    insert_member(tx, &channel, target, !open_both, now)?;
    Ok((channel, true))
}

/// Открывает переписку один на один для вызывающего (`openOneOnOne`).
///
/// # Errors
/// Ошибка базы.
pub fn open_one_on_one(
    tx: &mut dyn Tx,
    config: &Config,
    ids: &Snowflake,
    caller: &str,
    target: &str,
) -> DbResult<(String, bool)> {
    let (channel, created) = find_or_create_one_on_one(tx, ids, caller, target)?;
    if !created && set_closed(tx, &channel, caller, false)? {
        federation::queue_dm_close(tx, config, ids, &channel, caller, true)?;
    }
    Ok((channel, created))
}

/// Пользователь, на которого указывает клиент: локальный идентификатор или
/// пара «домашний идентификатор + инстанс» уже известной реплики.
///
/// # Errors
/// Ошибка базы.
pub fn resolve_target(
    tx: &mut dyn Tx,
    user_id: Option<&str>,
    home_user_id: Option<&str>,
    home_instance: Option<&str>,
) -> ApiResult<Option<Row>> {
    if let (Some(home_user), Some(home_instance)) = (
        home_user_id.filter(|id| !id.is_empty()),
        home_instance.filter(|home| !home.is_empty()),
    ) {
        let bare = home_instance
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        let columns = crate::routes::users::USER_COLUMNS;
        return Ok(tx.query_one(
            &format!(
                "SELECT {columns} FROM users WHERE home_user_id = ? AND home_instance IN (?, ?, ?)"
            ),
            params![
                home_user,
                bare,
                format!("https://{bare}"),
                format!("http://{bare}")
            ],
        )?);
    }
    match user_id.filter(|id| !id.is_empty()) {
        Some(id) => Ok(user_row(tx, id)?),
        None => Err(ApiError::bad_request("dm_target_required")),
    }
}

/// Дружат ли двое.
///
/// # Errors
/// Ошибка базы.
pub fn are_friends(tx: &mut dyn Tx, a: &str, b: &str) -> DbResult<bool> {
    Ok(tx
        .query_one(
            "SELECT 1 AS one FROM friends WHERE (user_id = ? AND friend_id = ?) \
             OR (user_id = ? AND friend_id = ?)",
            params![a, b, b, a],
        )?
        .is_some())
}

/// Сообщение по идентификатору.
///
/// # Errors
/// Ошибка базы.
pub fn message_row(tx: &mut dyn Tx, id: &str) -> DbResult<Option<Row>> {
    tx.query_one(
        &format!("SELECT {DM_MESSAGE_COLUMNS} FROM dm_messages WHERE id = ?"),
        params![id],
    )
}

fn kind_of(row: &Row) -> DbResult<String> {
    Ok(row.opt_text("type")?.unwrap_or_else(|| "user".to_owned()))
}

fn reply_preview(tx: &mut dyn Tx, channel: &str, reply_to: &str) -> DbResult<Value> {
    let Some(target) = tx.query_one(
        &format!("SELECT {DM_MESSAGE_COLUMNS} FROM dm_messages WHERE id = ? AND dm_channel_id = ?"),
        params![reply_to, channel],
    )?
    else {
        return Ok(Value::Null);
    };
    let Some(user) = user_row(tx, &target.text("user_id")?)? else {
        return Ok(Value::Null);
    };
    Ok(json!({
        "id": target.text("id")?,
        "dmChannelId": target.text("dm_channel_id")?,
        "userId": target.text("user_id")?,
        "replyToId": target.opt_text("reply_to_id")?,
        "content": target.opt_text("content")?,
        "type": kind_of(&target)?,
        "editedAt": target.opt_int("edited_at")?,
        "createdAt": target.int("created_at")?,
        "user": sanitize_user(&user, false)?,
        "attachments": [],
        "embeds": [],
        "reactions": [],
    }))
}

/// Личное сообщение на проводе (`buildDmMessageWithUser`). `Ok(None)` — нет
/// автора.
///
/// # Errors
/// Ошибка базы.
pub fn message_json(tx: &mut dyn Tx, message: &Row) -> DbResult<Option<Value>> {
    let Some(user) = user_row(tx, &message.text("user_id")?)? else {
        return Ok(None);
    };
    let id = message.text("id")?;
    let channel = message.text("dm_channel_id")?;
    let reply_to = match message.opt_text("reply_to_id")? {
        Some(target) => reply_preview(tx, &channel, &target)?,
        None => Value::Null,
    };
    // У вложений личных сообщений ключ владельца называется `messageId`.
    let mut attachments = dm_attachments(tx, &id, true)?;
    for attachment in &mut attachments {
        if let Some(object) = attachment.as_object_mut()
            && let Some(owner) = object.remove("dmMessageId")
        {
            object.insert("messageId".to_owned(), owner);
        }
    }
    Ok(Some(json!({
        "dmChannelId": channel,
        "userId": message.text("user_id")?,
        "replyToId": message.opt_text("reply_to_id")?,
        "content": message.opt_text("content")?,
        "type": kind_of(message)?,
        "editedAt": message.opt_int("edited_at")?,
        "createdAt": message.int("created_at")?,
        "sourceMessageId": message.opt_text("source_message_id")?,
        "sourceInstance": message.opt_text("source_instance")?,
        "user": sanitize_user(&user, false)?,
        "attachments": attachments,
        "embeds": embeds(tx, "dm_message_id", &id)?,
        "reactions": reactions(tx, true, &id, true)?,
        "replyTo": reply_to,
        "id": id,
    })))
}

/// Рассылает новое сообщение участникам: снимает «печатает», открывает
/// переписку тем, кто её закрыл (`broadcastDmMessage`).
///
/// # Errors
/// Ошибка базы.
pub fn broadcast_message(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    channel: &str,
    message: &Value,
) -> DbResult<()> {
    let author = message["userId"].as_str().unwrap_or_default();
    let members: Vec<String> = tx
        .query(
            "SELECT user_id FROM dm_members WHERE dm_channel_id = ?",
            params![channel],
        )?
        .iter()
        .map(|row| row.text("user_id"))
        .collect::<DbResult<_>>()?;
    for member in &members {
        if member != author {
            out.to_user(
                member,
                &json!({"type": "dm_typing_stop", "dmChannelId": channel, "userId": author}),
            );
        }
    }
    reopen_closed_with(tx, out, channel, Some(message))?;
    let event = json!({"type": "dm_message_created", "message": message});
    for member in &members {
        broadcast::to_user(tx, out, member, &event)?;
    }
    Ok(())
}

/// Открывает переписку тем участникам, кто её закрыл, и сообщает им о ней
/// (`reopenForClosedMembers`).
///
/// # Errors
/// Ошибка базы.
pub fn reopen_closed(tx: &mut dyn Tx, out: &mut Outbox, channel: &str) -> DbResult<()> {
    reopen_closed_with(tx, out, channel, None)
}

fn reopen_closed_with(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    channel: &str,
    last_message: Option<&Value>,
) -> DbResult<()> {
    let closed: Vec<String> = tx
        .query(
            "SELECT user_id FROM dm_members WHERE dm_channel_id = ? AND closed = 1",
            params![channel],
        )?
        .iter()
        .map(|row| row.text("user_id"))
        .collect::<DbResult<_>>()?;
    if closed.is_empty() {
        return Ok(());
    }
    tx.execute(
        "UPDATE dm_members SET closed = 0, closed_changed_at = ? WHERE dm_channel_id = ? AND closed = 1",
        params![now_ms(), channel],
    )?;
    if let Some(mut wire) = channel_wire(tx, channel)? {
        // Последнее сообщение уходит целиком, как его передал отправитель.
        if let Some(message) = last_message {
            wire["lastMessage"] = message.clone();
        }
        let event = json!({"type": "dm_channel_created", "dmChannel": wire});
        for user in &closed {
            broadcast::to_user(tx, out, user, &event)?;
        }
    }
    Ok(())
}

/// Отказ в действии с личным сообщением: статус, код и подробности.
#[derive(Debug)]
pub struct Refusal {
    /// HTTP-статус для REST.
    pub status: u16,
    /// Код ошибки.
    pub code: &'static str,
    /// Подробности.
    pub details: Option<Value>,
}

impl Refusal {
    const fn new(status: u16, code: &'static str) -> Self {
        Self {
            status,
            code,
            details: None,
        }
    }

    /// Ответ REST.
    #[must_use]
    pub fn into_api(self) -> ApiError {
        let status = axum::http::StatusCode::from_u16(self.status)
            .unwrap_or(axum::http::StatusCode::BAD_REQUEST);
        let mut error = ApiError::new(status, self.code);
        if let Some(Value::Object(details)) = self.details {
            for (key, value) in details {
                error = error.detail(leak_key(&key), value);
            }
        }
        error
    }
}

/// Имена подробностей известны заранее; строка нужна со статическим временем жизни.
fn leak_key(key: &str) -> &'static str {
    match key {
        "max" => "max",
        "min" => "min",
        "name" => "name",
        "requested" => "requested",
        _ => "detail",
    }
}

/// Проверенные поля нового сообщения.
#[derive(Debug)]
pub struct NewMessage {
    /// Текст.
    pub content: Option<String>,
    /// Вложения.
    pub attachments: Vec<String>,
    /// На что отвечает.
    pub reply_to: Option<String>,
}

/// Проверяет новое личное сообщение (`checkDmMessageCreate`).
///
/// # Errors
/// Ошибка базы; отказ — во внутреннем `Result`.
pub fn check_create(
    tx: &mut dyn Tx,
    channel: &str,
    user: &str,
    fields: &Value,
) -> DbResult<Result<NewMessage, Refusal>> {
    if !is_member(tx, channel, user)? {
        return Ok(Err(Refusal::new(403, "not_dm_member")));
    }
    if is_dead_one_on_one(tx, channel, user)? {
        return Ok(Err(Refusal::new(403, "recipient_deleted")));
    }
    let invalid = || Ok(Err(Refusal::new(400, "validation_failed")));
    let content = match fields.get("content") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.as_str()),
        Some(_) => return invalid(),
    };
    let attachments = match fields.get("attachments") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(list)) => {
            let ids: Option<Vec<String>> = list
                .iter()
                .map(|id| id.as_str().map(str::to_owned))
                .collect();
            match ids {
                Some(ids) => ids,
                None => return invalid(),
            }
        }
        Some(_) => return invalid(),
    };
    let reply_to = match fields.get("replyToId") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) => Some(id.as_str()).filter(|id| !id.is_empty()),
        Some(_) => return invalid(),
    };
    let text = content.map(js_trim).unwrap_or_default();
    if text.is_empty() && attachments.is_empty() {
        return Ok(Err(Refusal::new(400, "content_required")));
    }
    if content.is_some_and(|content| utf16_len(content) > MAX_MESSAGE_LENGTH) {
        return Ok(Err(Refusal {
            status: 400,
            code: "content_too_long",
            details: Some(json!({"max": MAX_MESSAGE_LENGTH})),
        }));
    }
    if let Some(reply) = reply_to
        && tx
            .query_one(
                "SELECT 1 AS one FROM dm_messages WHERE id = ? AND dm_channel_id = ?",
                params![reply, channel],
            )?
            .is_none()
    {
        return Ok(Err(Refusal::new(400, "reply_target_invalid")));
    }
    for id in &attachments {
        let row = tx.query_one(
            "SELECT message_id, dm_message_id, uploader_id FROM attachments WHERE id = ?",
            params![id],
        )?;
        let Some(row) = row else {
            return Ok(Err(Refusal::new(400, "attachment_invalid")));
        };
        if row.opt_text("message_id")?.is_some() || row.opt_text("dm_message_id")?.is_some() {
            return Ok(Err(Refusal::new(400, "attachment_invalid")));
        }
        if row.opt_text("uploader_id")?.as_deref() != Some(user) {
            return Ok(Err(Refusal::new(400, "attachment_not_owned")));
        }
    }
    Ok(Ok(NewMessage {
        content: Some(text)
            .filter(|text| !text.is_empty())
            .map(str::to_owned),
        attachments,
        reply_to: reply_to.map(str::to_owned),
    }))
}

/// Создаёт проверенное сообщение, рассылает его и возвращает.
///
/// # Errors
/// Ошибка базы.
pub fn create_message(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    config: &Config,
    ids: &Snowflake,
    channel: &str,
    user: &str,
    new: &NewMessage,
) -> DbResult<Option<Value>> {
    let id = ids.next();
    tx.execute(
        "INSERT INTO dm_messages (id, dm_channel_id, user_id, reply_to_id, content, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
        params![&id, channel, user, &new.reply_to, &new.content, now_ms()],
    )?;
    for attachment in &new.attachments {
        tx.execute(
            "UPDATE attachments SET dm_message_id = ? WHERE id = ?",
            params![&id, attachment],
        )?;
    }
    let Some(row) = message_row(tx, &id)? else {
        return Ok(None);
    };
    let Some(message) = message_json(tx, &row)? else {
        return Ok(None);
    };
    broadcast_message(tx, out, channel, &message)?;
    federation::queue_dm_message(tx, config, ids, channel, &id, false)?;
    Ok(Some(message))
}

/// Проверяет правку (`checkDmMessageEdit`): возвращает сообщение и новый текст.
///
/// # Errors
/// Ошибка базы; отказ — во внутреннем `Result`.
pub fn check_edit(
    tx: &mut dyn Tx,
    message_id: &str,
    editor: &str,
    content: Option<&Value>,
) -> DbResult<Result<(Row, String), Refusal>> {
    let Some(content) = content
        .and_then(Value::as_str)
        .filter(|text| !js_trim(text).is_empty())
    else {
        return Ok(Err(Refusal::new(400, "content_required")));
    };
    if utf16_len(content) > MAX_MESSAGE_LENGTH {
        return Ok(Err(Refusal {
            status: 400,
            code: "content_too_long",
            details: Some(json!({"max": MAX_MESSAGE_LENGTH})),
        }));
    }
    let Some(message) = message_row(tx, message_id)? else {
        return Ok(Err(Refusal::new(404, "message_not_found")));
    };
    let channel = message.text("dm_channel_id")?;
    if !is_member(tx, &channel, editor)? {
        return Ok(Err(Refusal::new(403, "not_dm_member")));
    }
    if kind_of(&message)? == "system" {
        return Ok(Err(Refusal::new(403, "system_message_immutable")));
    }
    if message.text("user_id")? != editor {
        return Ok(Err(Refusal::new(403, "not_message_author")));
    }
    if is_dead_one_on_one(tx, &channel, editor)? {
        return Ok(Err(Refusal::new(403, "recipient_deleted")));
    }
    Ok(Ok((message, js_trim(content).to_owned())))
}

/// Применяет проверенную правку и рассылает её.
///
/// # Errors
/// Ошибка базы.
pub fn edit_message(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    config: &Config,
    ids: &Snowflake,
    message: &Row,
    content: &str,
) -> DbResult<Option<Value>> {
    let id = message.text("id")?;
    let channel = message.text("dm_channel_id")?;
    tx.execute(
        "UPDATE dm_messages SET content = ?, edited_at = ? WHERE id = ?",
        params![content, now_ms(), &id],
    )?;
    tx.execute("DELETE FROM embeds WHERE dm_message_id = ?", params![&id])?;
    let Some(row) = message_row(tx, &id)? else {
        return Ok(None);
    };
    let Some(updated) = message_json(tx, &row)? else {
        return Ok(None);
    };
    broadcast::to_dm_members(
        tx,
        out,
        &channel,
        &json!({"type": "dm_message_updated", "message": updated}),
        None,
    )?;
    federation::queue_dm_message(tx, config, ids, &channel, &id, true)?;
    Ok(Some(updated))
}

/// Проверяет удаление (`checkDmMessageDelete`).
///
/// # Errors
/// Ошибка базы; отказ — во внутреннем `Result`.
pub fn check_delete(
    tx: &mut dyn Tx,
    message_id: &str,
    user: &str,
) -> DbResult<Result<Row, Refusal>> {
    let Some(message) = message_row(tx, message_id)? else {
        return Ok(Err(Refusal::new(404, "message_not_found")));
    };
    let channel = message.text("dm_channel_id")?;
    if !is_member(tx, &channel, user)? {
        return Ok(Err(Refusal::new(403, "not_dm_member")));
    }
    if message.text("user_id")? != user {
        return Ok(Err(Refusal::new(403, "not_message_author")));
    }
    if is_dead_one_on_one(tx, &channel, user)? {
        return Ok(Err(Refusal::new(403, "recipient_deleted")));
    }
    Ok(Ok(message))
}

/// Удаляет проверенное сообщение; возвращает файлы вложений.
///
/// # Errors
/// Ошибка базы.
pub fn delete_message(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    config: &Config,
    ids: &Snowflake,
    message: &Row,
    actor: &str,
) -> DbResult<Vec<String>> {
    let id = message.text("id")?;
    let channel = message.text("dm_channel_id")?;
    let target = federation::mutation_target(tx, config, &channel, &id, actor)?;
    let stale = tx
        .query(
            "SELECT filename FROM attachments WHERE dm_message_id = ?",
            params![&id],
        )?
        .iter()
        .map(|row| row.text("filename"))
        .collect::<DbResult<Vec<_>>>()?;
    tx.execute(
        "DELETE FROM attachments WHERE dm_message_id = ?",
        params![&id],
    )?;
    tx.execute(
        "DELETE FROM dm_reactions WHERE dm_message_id = ?",
        params![&id],
    )?;
    tx.execute("DELETE FROM dm_messages WHERE id = ?", params![&id])?;
    broadcast::to_dm_members(
        tx,
        out,
        &channel,
        &json!({"type": "dm_message_deleted", "messageId": id, "dmChannelId": channel}),
        None,
    )?;
    federation::queue_dm_delete(tx, config, ids, &channel, &id, target)?;
    Ok(stale)
}

/// Вставляет системное сообщение и возвращает его в виде для рассылки.
///
/// # Errors
/// Ошибка базы.
pub fn system_message(
    tx: &mut dyn Tx,
    ids: &Snowflake,
    channel: &str,
    actor: &str,
    content: &str,
    source_message_id: Option<&str>,
    at: i64,
) -> DbResult<Value> {
    let id = ids.next();
    tx.execute(
        "INSERT INTO dm_messages (id, dm_channel_id, user_id, content, type, source_message_id, created_at) \
         VALUES (?, ?, ?, ?, 'system', ?, ?)",
        params![&id, channel, actor, content, source_message_id, at],
    )?;
    let mut message = json!({
        "id": id,
        "dmChannelId": channel,
        "userId": actor,
        "content": content,
        "type": "system",
        "createdAt": at,
        "attachments": [],
        "embeds": [],
        "reactions": [],
    });
    if let Some(user) = user_row(tx, actor)? {
        message["user"] = sanitize_user(&user, false)?;
    }
    Ok(message)
}

fn display_name(tx: &mut dyn Tx, user_id: &str) -> DbResult<String> {
    Ok(match user_row(tx, user_id)? {
        Some(user) => system_name(
            user.opt_text("display_name")?.as_deref(),
            &user.text("username")?,
        ),
        None => "Unknown".to_owned(),
    })
}

/// Содержимое системного сообщения: ключи в порядке оригинала.
#[must_use]
pub fn system_content(pairs: &[(&str, Value)]) -> String {
    let body: Vec<String> = pairs
        .iter()
        .map(|(key, value)| format!("{}:{value}", Value::from(*key)))
        .collect();
    format!("{{{}}}", body.join(","))
}

/// Сообщение «участник добавлен».
///
/// # Errors
/// Ошибка базы.
pub fn member_added_content(tx: &mut dyn Tx, target: &str) -> DbResult<String> {
    Ok(system_content(&[
        ("event", json!("member_added")),
        ("targetUserId", json!(target)),
        ("targetDisplayName", json!(display_name(tx, target)?)),
    ]))
}

/// Передаёт владение группой (`transferGroupDmOwnership`).
///
/// # Errors
/// Ошибка базы.
pub fn transfer_ownership(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    config: &Config,
    ids: &Snowflake,
    channel: &str,
    actor: &str,
    new_owner: &str,
) -> DbResult<()> {
    let relay = federation::relay_enabled(tx)?;
    let federated = if relay {
        federation::dm_federated_id(tx, channel)?
    } else {
        None
    };
    let targets = match &federated {
        Some(_) => federation::relay::dm_targets(tx, &federation::our_origin(config), channel)?,
        None => Vec::new(),
    };
    let previous_owner = channel_row(tx, channel)?
        .map(|row| row.opt_text("owner_id"))
        .transpose()?
        .flatten()
        .unwrap_or_default();
    let new_row = tx.query_one(
        "SELECT home_user_id, home_instance FROM users WHERE id = ?",
        params![new_owner],
    )?;
    let (home_user, home_instance) = match new_row {
        Some(row) => (
            row.opt_text("home_user_id")?,
            row.opt_text("home_instance")?,
        ),
        None => (None, None),
    };
    let home_user = home_user
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| new_owner.to_owned());
    let home_instance = home_instance
        .filter(|home| !home.is_empty())
        .or_else(|| relay.then(|| federation::our_origin(config)))
        .map(|home| {
            let home = home.trim().trim_end_matches('/').to_owned();
            if home.starts_with("http://") || home.starts_with("https://") {
                home
            } else {
                format!("https://{home}")
            }
        });
    tx.execute(
        "UPDATE dm_channels SET owner_id = ?, owner_home_user_id = ?, owner_home_instance = ? WHERE id = ?",
        params![new_owner, &home_user, &home_instance, channel],
    )?;
    let content = system_content(&[
        ("event", json!("owner_changed")),
        ("newOwnerId", json!(new_owner)),
        ("newOwnerDisplayName", json!(display_name(tx, new_owner)?)),
    ]);
    let at = now_ms();
    let message = system_message(tx, ids, channel, actor, &content, None, at)?;
    if let Some(federated) = &federated {
        let origin = federation::our_origin(config);
        let previous = tx.query_one(
            "SELECT home_user_id, home_instance FROM users WHERE id = ?",
            params![&previous_owner],
        )?;
        let (previous_user, previous_home) = match previous {
            Some(row) => (
                row.opt_text("home_user_id")?.filter(|id| !id.is_empty()),
                row.opt_text("home_instance")?
                    .filter(|home| !home.is_empty()),
            ),
            None => (None, None),
        };
        let previous_home = crate::federation_auth::canonical_home_instance(Some(
            previous_home.as_deref().unwrap_or(&origin),
        ))
        .unwrap_or_default();
        federation::relay::ownership_transfer(
            tx,
            federation::relay::Relay { config, ids },
            channel,
            federated,
            new_owner,
            &json!({"homeUserId": home_user, "homeInstance": home_instance.clone().unwrap_or_default()}),
            &json!({
                "homeUserId": previous_user.unwrap_or_else(|| previous_owner.clone()),
                "homeInstance": previous_home,
            }),
            at,
            &targets,
        )?;
    }
    broadcast::to_dm_members(
        tx,
        out,
        channel,
        &json!({
            "type": "dm_owner_updated",
            "dmChannelId": channel,
            "newOwnerId": new_owner,
            "newOwnerHomeUserId": home_user,
            "newOwnerHomeInstance": home_instance,
        }),
        None,
    )?;
    broadcast::to_dm_members(
        tx,
        out,
        channel,
        &json!({"type": "dm_message_created", "message": message}),
        None,
    )?;
    Ok(())
}

/// Убирает участника из группы: сам вышел или исключён (`removeDmMember`).
///
/// # Errors
/// Ошибка базы.
pub fn remove_member(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    config: &Config,
    ids: &Snowflake,
    channel: &str,
    actor: &str,
    target: &str,
    kicked: bool,
) -> DbResult<()> {
    let Some(row) = channel_row(tx, channel)? else {
        return Ok(());
    };
    let owner = row.opt_text("owner_id")?;
    let federated = row
        .opt_text("federated_id")?
        .filter(|id| !id.is_empty() && federation::relay_enabled(tx).unwrap_or(false));
    let targets = match &federated {
        Some(_) => federation::relay::dm_targets(tx, &federation::our_origin(config), channel)?,
        None => Vec::new(),
    };
    let content = system_content(&[
        ("event", json!("member_removed")),
        ("targetUserId", json!(target)),
        ("targetDisplayName", json!(display_name(tx, target)?)),
        ("reason", json!(if kicked { "kick" } else { "leave" })),
    ]);
    let message = system_message(tx, ids, channel, actor, &content, None, now_ms())?;
    broadcast::to_dm_members(
        tx,
        out,
        channel,
        &json!({"type": "dm_message_created", "message": message}),
        None,
    )?;
    tx.execute(
        "DELETE FROM dm_members WHERE dm_channel_id = ? AND user_id = ?",
        params![channel, target],
    )?;
    tx.execute(
        "DELETE FROM read_states WHERE user_id = ? AND channel_id = ?",
        params![target, channel],
    )?;
    let removed_at = now_ms();
    let removed = federation::relay::identity_value(tx, config, target)?;
    if let Some(federated) = &row.opt_text("federated_id")?.filter(|id| !id.is_empty()) {
        federation::relay::record_member_change(tx, config, federated, &removed, removed_at)?;
    }
    if let Some(federated) = &federated {
        let actor = federation::relay::identity_value(tx, config, actor)?;
        federation::relay::member_remove(
            tx,
            federation::relay::Relay { config, ids },
            channel,
            federated,
            &removed,
            target,
            &actor,
            if kicked { "kick" } else { "leave" },
            removed_at,
            &targets,
        )?;
    }
    let remaining: Vec<String> = tx
        .query(
            "SELECT user_id FROM dm_members WHERE dm_channel_id = ?",
            params![channel],
        )?
        .iter()
        .map(|row| row.text("user_id"))
        .collect::<DbResult<_>>()?;
    if remaining.is_empty() {
        if !kicked {
            tx.execute(
                "UPDATE dm_channels SET deleted_at = ? WHERE id = ?",
                params![now_ms(), channel],
            )?;
        }
        return Ok(());
    }
    if !kicked
        && owner.as_deref() == Some(actor)
        && let Some(next) = remaining.first()
    {
        transfer_ownership(tx, out, config, ids, channel, actor, next)?;
    }
    let event = json!({"type": "dm_member_removed", "dmChannelId": channel, "userId": target});
    for member in &remaining {
        out.to_user(member, &event);
    }
    Ok(())
}
