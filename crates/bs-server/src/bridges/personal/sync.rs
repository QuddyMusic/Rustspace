//! Синхронизация личного моста: сообщения с вложениями в обе стороны,
//! история при связывании, правки, удаления, реакции, профиль и «в сети»
//! собеседника.
//!
//! Соответствие сообщений хранится в `bs_personal_messages`: номер
//! сообщения той стороны ↔ личное сообщение здесь.

use serde_json::json;

use super::{Outgoing, Remote, RemoteFile, RemoteProfile, enabled};
use crate::db::{DbResult, Tx};
use crate::routes::users::{USER_COLUMNS, sanitize_user};
use crate::state::{AppState, now_ms};
use crate::{broadcast, dm, params, ws};

/// Сколько последних сообщений переносимой истории клиенты видят сразу.
const LIVE_TAIL: usize = 50;
/// Как часто спрашивать «в сети» собеседников.
const PRESENCE_EVERY: std::time::Duration = std::time::Duration::from_mins(1);

/// Тяжёлая работа мостов (история, профили) идёт по одной: при связывании
/// сотни чатов разом параллельные загрузки съедали память сервера.
static HEAVY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

// ─── Файлы ────────────────────────────────────────────────────────────────────

/// Кладёт файлы в каталог загрузок как вложения (ещё без сообщения).
/// Возвращает id вложений; не уложившиеся в лимит пропускаются.
async fn store_files(state: &AppState, files: Vec<RemoteFile>) -> Vec<String> {
    let mut ids = Vec::new();
    for file in files {
        let limit = usize::try_from(state.config.max_upload_size).unwrap_or(usize::MAX);
        if file.bytes.is_empty() || file.bytes.len() > limit {
            tracing::info!(name = file.name, "личный мост: файл пропущен по размеру");
            continue;
        }
        match store_file(state, &file).await {
            Ok(id) => ids.push(id),
            Err(err) => tracing::warn!(%err, "личный мост: файл не сохранён"),
        }
    }
    ids
}

async fn store_file(state: &AppState, file: &RemoteFile) -> Result<String, String> {
    let id = state.ids.next();
    let original = crate::bridges::clip(&file.name, 120);
    let filename = format!("{id}{}", crate::routes::files::extension(&original));
    let staging = state.config.upload_dir.join(".tus");
    tokio::fs::create_dir_all(&staging)
        .await
        .map_err(|err| err.to_string())?;
    let source = staging.join(format!("bridge-{id}"));
    tokio::fs::write(&source, &file.bytes)
        .await
        .map_err(|err| err.to_string())?;
    let mimetype = crate::routes::files::mimetype_for(&original)
        .unwrap_or("application/octet-stream")
        .to_owned();
    let thumb = crate::files::thumb_filename(&filename);
    let done = crate::media::process(&source, &mimetype, &state.config.upload_dir, &thumb).await;
    let target = crate::files::upload_path(&state.config, &filename)
        .ok_or_else(|| "недопустимое имя файла".to_owned())?;
    tokio::fs::rename(&source, &target)
        .await
        .map_err(|err| err.to_string())?;
    let size = i64::try_from(file.bytes.len()).unwrap_or(i64::MAX);
    let row_id = id.clone();
    state
        .run(move |tx, _| {
            tx.execute(
                "INSERT INTO attachments (id, uploader_id, message_id, dm_message_id, filename, original_name, \
                 mimetype, size, thumbnail_filename, width, height, duration, playable, created_at) \
                 VALUES (?, NULL, NULL, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    &row_id,
                    &filename,
                    &original,
                    &mimetype,
                    size,
                    &done.thumbnail,
                    done.width,
                    done.height,
                    done.duration,
                    done.playable.map(i64::from),
                    now_ms()
                ],
            )?;
            Ok(())
        })
        .await
        .map_err(|err| format!("{err:?}"))?;
    Ok(id)
}

// ─── Соответствие сообщений ──────────────────────────────────────────────────

fn local_of(
    tx: &mut dyn Tx,
    account: &str,
    chat: Option<&str>,
    remote_id: &str,
) -> DbResult<Option<String>> {
    let row = match chat {
        Some(chat) => tx.query_one(
            "SELECT dm_message_id FROM bs_personal_messages \
             WHERE account_id = ? AND remote_chat = ? AND remote_id = ?",
            params![account, chat, remote_id],
        )?,
        // Удаления в личных чатах и группах Telegram приходят без чата:
        // номера сообщений там общие на аккаунт.
        None => tx.query_one(
            "SELECT dm_message_id FROM bs_personal_messages WHERE account_id = ? AND remote_id = ?",
            params![account, remote_id],
        )?,
    };
    row.map(|row| row.text("dm_message_id")).transpose()
}

fn remember(
    tx: &mut dyn Tx,
    account: &str,
    chat: &str,
    remote_id: &str,
    local: &str,
) -> DbResult<()> {
    tx.execute(
        "INSERT INTO bs_personal_messages (account_id, remote_chat, remote_id, dm_message_id) \
         VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING",
        params![account, chat, remote_id, local],
    )?;
    Ok(())
}

/// Связанный чат и номер той стороны для сообщения отсюда.
fn remote_of(tx: &mut dyn Tx, message_id: &str) -> DbResult<Option<(String, String, String)>> {
    let row = tx.query_one(
        "SELECT account_id, remote_chat, remote_id FROM bs_personal_messages WHERE dm_message_id = ?",
        params![message_id],
    )?;
    row.map(|row| {
        Ok((
            row.text("account_id")?,
            row.text("remote_chat")?,
            row.text("remote_id")?,
        ))
    })
    .transpose()
}

// ─── Внутрь ───────────────────────────────────────────────────────────────────

/// Пишет сообщение с той стороны в связанную переписку.
pub(crate) async fn deliver(state: &AppState, message: Remote) {
    deliver_as(state, message, true).await;
}

/// `live == false` — старое сообщение из истории: пишется молча, без
/// событий клиентам (иначе перенос тысяч сообщений засыпал бы их событиями);
/// клиент увидит его, когда подгрузит историю.
async fn deliver_as(state: &AppState, message: Remote, live: bool) {
    if let Some(remote_id) = message.id.clone() {
        let (account, chat) = (message.account.clone(), message.chat.clone());
        let known = state
            .run(move |tx, _| Ok(local_of(tx, &account, Some(&chat), &remote_id)?))
            .await;
        if matches!(known, Ok(Some(_))) {
            return;
        }
    }
    let attachments = store_files(state, message.files.clone()).await;
    let app = state.clone();
    let result = state
        .run(move |tx, out| {
            let Some(row) = tx.query_one(
                "SELECT c.dm_channel_id, c.puppet_id, c.grouped, a.user_id \
                 FROM bs_personal_chats c JOIN bs_personal_accounts a ON a.id = c.account_id \
                 WHERE c.account_id = ? AND c.remote_chat = ?",
                params![&message.account, &message.chat],
            )?
            else {
                return Ok(());
            };
            let channel = row.text("dm_channel_id")?;
            let (author, content) = if message.mine {
                (row.text("user_id")?, message.text.clone())
            } else if row.int("grouped")? != 0 && !message.text.is_empty() {
                (
                    row.text("puppet_id")?,
                    format!(
                        "**{}**: {}",
                        crate::bridges::clip(&message.sender, 48),
                        message.text
                    ),
                )
            } else {
                (row.text("puppet_id")?, message.text.clone())
            };
            let content: String = content.chars().take(4000).collect();
            if content.trim().is_empty() && attachments.is_empty() {
                return Ok(());
            }
            let reply_to = match &message.reply_to {
                Some(remote) => local_of(tx, &message.account, Some(&message.chat), remote)?,
                None => None,
            };
            let new = dm::NewMessage {
                content: (!content.trim().is_empty()).then_some(content),
                attachments: attachments.clone(),
                reply_to,
            };
            let mut quiet = crate::hub::Outbox::new();
            let created = dm::create_message_at(
                tx,
                if live { out } else { &mut quiet },
                &app.config,
                &app.ids,
                &channel,
                &author,
                &new,
                message.date.unwrap_or_else(now_ms),
            )?;
            if let (Some(created), Some(remote_id)) = (created, &message.id)
                && let Some(local) = created["id"].as_str()
            {
                remember(tx, &message.account, &message.chat, remote_id, local)?;
            }
            Ok(())
        })
        .await;
    if let Err(err) = result {
        tracing::warn!(?err, "личный мост: не удалось записать сообщение");
    }
}

/// Сообщение изменили на той стороне.
pub(crate) async fn remote_edit(
    state: &AppState,
    account: &str,
    chat: &str,
    remote_id: &str,
    text: &str,
) {
    let (app, account, chat, remote_id) = (
        state.clone(),
        account.to_owned(),
        chat.to_owned(),
        remote_id.to_owned(),
    );
    let text: String = text.trim().chars().take(4000).collect();
    let result = state
        .run(move |tx, out| {
            let Some(local) = local_of(tx, &account, Some(&chat), &remote_id)? else {
                return Ok(());
            };
            let Some(row) = dm::message_row(tx, &local)? else {
                return Ok(());
            };
            let old = row.opt_text("content")?.unwrap_or_default();
            // В группе к тексту приписано имя автора: сравниваем хвост.
            if text.is_empty() || old == text || old.ends_with(&format!(": {text}")) {
                return Ok(());
            }
            let content = match old.split_once("**: ") {
                Some((author, _)) if old.starts_with("**") => format!("{author}**: {text}"),
                _ => text,
            };
            dm::edit_message(tx, out, &app.config, &app.ids, &row, &content)?;
            Ok(())
        })
        .await;
    if let Err(err) = result {
        tracing::warn!(?err, "личный мост: правка не записана");
    }
}

/// Сообщения удалили на той стороне.
pub(crate) async fn remote_delete(
    state: &AppState,
    account: &str,
    chat: Option<&str>,
    remote_ids: &[String],
) {
    let (app, account, chat, remote_ids) = (
        state.clone(),
        account.to_owned(),
        chat.map(str::to_owned),
        remote_ids.to_vec(),
    );
    let stale = state
        .run(move |tx, out| {
            let mut stale = Vec::new();
            for remote_id in &remote_ids {
                let Some(local) = local_of(tx, &account, chat.as_deref(), remote_id)? else {
                    continue;
                };
                tx.execute(
                    "DELETE FROM bs_personal_messages WHERE dm_message_id = ?",
                    params![&local],
                )?;
                let Some(row) = dm::message_row(tx, &local)? else {
                    continue;
                };
                let author = row.text("user_id")?;
                stale.extend(dm::delete_message(
                    tx,
                    out,
                    &app.config,
                    &app.ids,
                    &row,
                    &author,
                )?);
            }
            Ok(stale)
        })
        .await;
    match stale {
        Ok(files) => {
            for name in files {
                crate::files::delete_upload_file(&state.config, &name).await;
            }
        }
        Err(err) => tracing::warn!(?err, "личный мост: удаление не записано"),
    }
}

/// Реакции сообщения на той стороне: у собеседника и у владельца аккаунта.
pub(crate) async fn remote_reactions(
    state: &AppState,
    account: &str,
    chat: &str,
    remote_id: &str,
    theirs: Vec<String>,
    mine: Vec<String>,
) {
    let (app, account, chat, remote_id) = (
        state.clone(),
        account.to_owned(),
        chat.to_owned(),
        remote_id.to_owned(),
    );
    let result = state
        .run(move |tx, out| {
            let Some(local) = local_of(tx, &account, Some(&chat), &remote_id)? else {
                return Ok(());
            };
            let Some(row) = tx.query_one(
                "SELECT c.dm_channel_id, c.puppet_id, a.user_id FROM bs_personal_chats c \
                 JOIN bs_personal_accounts a ON a.id = c.account_id \
                 WHERE c.account_id = ? AND c.remote_chat = ?",
                params![&account, &chat],
            )?
            else {
                return Ok(());
            };
            let channel = row.text("dm_channel_id")?;
            for (user, wanted) in [
                (row.text("puppet_id")?, &theirs),
                (row.text("user_id")?, &mine),
            ] {
                set_reactions(tx, out, &app, &channel, &local, &user, wanted)?;
            }
            Ok(())
        })
        .await;
    if let Err(err) = result {
        tracing::warn!(?err, "личный мост: реакции не записаны");
    }
}

/// Приводит реакции пользователя на сообщение к `wanted`.
fn set_reactions(
    tx: &mut dyn Tx,
    out: &mut crate::hub::Outbox,
    app: &AppState,
    channel: &str,
    message_id: &str,
    user: &str,
    wanted: &[String],
) -> DbResult<()> {
    let have: Vec<String> = tx
        .query(
            "SELECT emoji FROM dm_reactions WHERE dm_message_id = ? AND user_id = ?",
            params![message_id, user],
        )?
        .iter()
        .map(|row| row.text("emoji"))
        .collect::<DbResult<_>>()?;
    for emoji in have.iter().filter(|emoji| !wanted.contains(emoji)) {
        tx.execute(
            "DELETE FROM dm_reactions WHERE dm_message_id = ? AND user_id = ? AND emoji = ?",
            params![message_id, user, emoji],
        )?;
        broadcast::to_dm_members(
            tx,
            out,
            channel,
            &json!({"type": "reaction_removed", "messageId": message_id, "userId": user, "emoji": emoji}),
            None,
        )?;
    }
    for emoji in wanted.iter().filter(|emoji| !have.contains(emoji)) {
        let id = app.ids.next();
        let now = now_ms();
        tx.execute(
            "INSERT INTO dm_reactions (id, dm_message_id, user_id, emoji, created_at) \
             VALUES (?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
            params![&id, message_id, user, emoji, now],
        )?;
        let mut reaction = json!({
            "id": id, "messageId": message_id, "userId": user, "emoji": emoji, "createdAt": now,
        });
        if let Some(row) = tx.query_one(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"),
            params![user],
        )? {
            reaction["user"] = sanitize_user(&row, false)?;
        }
        broadcast::to_dm_members(
            tx,
            out,
            channel,
            &json!({"type": "reaction_added", "messageId": message_id, "reaction": reaction}),
            None,
        )?;
    }
    Ok(())
}

/// «В сети» собеседника (личный чат с пользователем Telegram).
pub(crate) async fn remote_presence(state: &AppState, account: &str, chat: &str, online: bool) {
    let (account, chat) = (account.to_owned(), chat.to_owned());
    let status = if online { "online" } else { "offline" };
    let result = state
        .run(move |tx, out| {
            for row in tx.query(
                "SELECT puppet_id FROM bs_personal_chats WHERE account_id = ? AND remote_chat = ?",
                params![&account, &chat],
            )? {
                let puppet = row.text("puppet_id")?;
                let changed = tx.execute(
                    "UPDATE users SET status = ? WHERE id = ? AND (status IS NULL OR status <> ?)",
                    params![status, &puppet, status],
                )?;
                if changed > 0 {
                    let payload = ws::presence_update(tx, &puppet, status, None)?;
                    ws::to_profile_audience(tx, out, &puppet, &payload)?;
                }
            }
            Ok(())
        })
        .await;
    if let Err(err) = result {
        tracing::warn!(?err, "личный мост: статус не записан");
    }
}

/// Фото, «о себе» и «в сети» собеседника — в марионетку.
async fn apply_profile(state: &AppState, account: &str, chat: &str, profile: RemoteProfile) {
    let mut avatar = None;
    if let Some((photo_id, bytes)) = &profile.photo {
        let name = format!("{}.jpg", state.ids.next());
        if let Some(path) = crate::files::upload_path(&state.config, &name)
            && tokio::fs::write(&path, bytes).await.is_ok()
        {
            avatar = Some((photo_id.clone(), name));
        }
    }
    let (account_id, chat_id) = (account.to_owned(), chat.to_owned());
    let bio = profile.bio.clone();
    let replaced = state
        .run(move |tx, out| {
            let mut replaced = Vec::new();
            for row in tx.query(
                "SELECT puppet_id FROM bs_personal_chats WHERE account_id = ? AND remote_chat = ?",
                params![&account_id, &chat_id],
            )? {
                let puppet = row.text("puppet_id")?;
                if let Some((photo_id, name)) = &avatar {
                    if let Some(old) = tx
                        .query_one("SELECT avatar FROM users WHERE id = ?", params![&puppet])?
                        .and_then(|row| row.opt_text("avatar").ok().flatten())
                    {
                        replaced.push(old);
                    }
                    tx.execute(
                        "UPDATE users SET avatar = ? WHERE id = ?",
                        params![name, &puppet],
                    )?;
                    tx.execute(
                        "UPDATE bs_personal_chats SET photo_id = ? WHERE account_id = ? AND remote_chat = ?",
                        params![photo_id, &account_id, &chat_id],
                    )?;
                }
                if let Some(bio) = &bio {
                    let bio: String = bio.chars().take(190).collect();
                    tx.execute(
                        "UPDATE users SET bio = ? WHERE id = ?",
                        params![&bio, &puppet],
                    )?;
                }
                tx.execute(
                    "UPDATE users SET profile_updated_at = ? WHERE id = ?",
                    params![now_ms(), &puppet],
                )?;
                if let Some(user) = tx.query_one(
                    &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"),
                    params![&puppet],
                )? {
                    crate::federation::identity::announce_user_updated(tx, out, &user)?;
                }
            }
            Ok(replaced)
        })
        .await;
    match replaced {
        Ok(old) => {
            for name in old {
                crate::files::delete_upload_file(&state.config, &name).await;
            }
        }
        Err(err) => tracing::warn!(?err, "личный мост: профиль не записан"),
    }
    if let Some(online) = profile.online {
        remote_presence(state, account, chat, online).await;
    }
}

/// После связывания чата: профиль собеседника и последние сообщения.
pub(crate) fn after_link(state: &AppState, account: &str, chat: &str) {
    let (app, account, chat) = (state.clone(), account.to_owned(), chat.to_owned());
    tokio::spawn(async move {
        let Ok(_permit) = HEAVY.acquire().await else {
            return;
        };
        let Some(link) = app.personal.link(&account) else {
            return;
        };
        if let Some(profile) = link.profile(&chat, None).await {
            apply_profile(&app, &account, &chat, profile).await;
        }
        import_history(&app, &link, &account, &chat).await;
    });
}

/// Переносит всю историю чата. Уже перенесённые сообщения пропускаются
/// (`deliver` знает их номера), так что повторный вызов только дополняет.
async fn import_history(app: &AppState, link: &super::Link, account: &str, chat: &str) {
    let (sink, mut incoming) = tokio::sync::mpsc::channel(4);
    let consume = async {
        let mut batch = Vec::new();
        while let Some(message) = incoming.recv().await {
            batch.push(message);
            // Последние сообщения клиенты получат сразу, остальные молча.
            if batch.len() > LIVE_TAIL {
                deliver_as(app, batch.remove(0), false).await;
            }
        }
        for message in batch {
            deliver_as(app, message, true).await;
        }
    };
    tokio::join!(link.history(account, chat, sink), consume);
}

/// Перечитать историю связанного чата (по id переписки у себя).
/// `false` — это не мостовая переписка пользователя или мост не подключён.
pub(crate) async fn resync_dm(
    state: &AppState,
    user: &str,
    dm: &str,
) -> crate::error::ApiResult<bool> {
    let (owner, dm) = (user.to_owned(), dm.to_owned());
    let found = state
        .run(move |tx, _| {
            Ok(tx
                .query_one(
                    "SELECT c.account_id, c.remote_chat FROM bs_personal_chats c \
                     JOIN bs_personal_accounts a ON a.id = c.account_id \
                     WHERE c.dm_channel_id = ? AND a.user_id = ?",
                    params![&dm, &owner],
                )?
                .map(|row| {
                    Ok::<_, crate::db::DbError>((row.text("account_id")?, row.text("remote_chat")?))
                })
                .transpose()?)
        })
        .await?;
    let Some((account, chat)) = found else {
        return Ok(false);
    };
    if state.personal.link(&account).is_none() {
        return Ok(false);
    }
    after_link(state, &account, &chat);
    Ok(true)
}

/// Аккаунт подключился: обновить профили и следить за «в сети».
pub(crate) fn connected(state: &AppState, account: &str) {
    refresh_profiles(state, account);
    poll_presence(state, account);
}

/// Telegram шлёт «в сети» только сеансам, которые сами в сети, а мост
/// невидим — поэтому статусы собеседников спрашиваем раз в минуту.
fn poll_presence(state: &AppState, account: &str) {
    let (app, account) = (state.clone(), account.to_owned());
    let Some(link) = app.personal.link(&account) else {
        return;
    };
    tokio::spawn(async move {
        loop {
            // Аккаунт отключили или переподключили — этот опрос больше не нужен.
            if !app
                .personal
                .link(&account)
                .is_some_and(|now| std::sync::Arc::ptr_eq(&now, &link))
            {
                return;
            }
            let lookup = account.clone();
            let chats = app
                .run(move |tx, _| {
                    tx.query(
                        "SELECT remote_chat FROM bs_personal_chats WHERE account_id = ? AND grouped = 0",
                        params![&lookup],
                    )?
                    .iter()
                    .map(|row| row.text("remote_chat"))
                    .collect::<DbResult<Vec<_>>>()
                    .map_err(Into::into)
                })
                .await
                .unwrap_or_default();
            for (chat, online) in link.presence(&chats).await {
                remote_presence(&app, &account, &chat, online).await;
            }
            tokio::time::sleep(PRESENCE_EVERY).await;
        }
    });
}

/// После подключения аккаунта: профили всех связанных чатов (фото — только
/// если поменялось).
pub(crate) fn refresh_profiles(state: &AppState, account: &str) {
    let (app, account) = (state.clone(), account.to_owned());
    tokio::spawn(async move {
        let Ok(_permit) = HEAVY.acquire().await else {
            return;
        };
        let lookup = account.clone();
        let Ok(chats) = app
            .run(move |tx, _| {
                Ok(tx
                    .query(
                        "SELECT remote_chat, photo_id FROM bs_personal_chats WHERE account_id = ?",
                        params![&lookup],
                    )?
                    .iter()
                    .map(|row| Ok((row.text("remote_chat")?, row.opt_text("photo_id")?)))
                    .collect::<DbResult<Vec<_>>>()?)
            })
            .await
        else {
            return;
        };
        for (chat, photo) in chats {
            let Some(link) = app.personal.link(&account) else {
                return;
            };
            if let Some(profile) = link.profile(&chat, photo.as_deref()).await {
                apply_profile(&app, &account, &chat, profile).await;
            }
        }
    });
}

// ─── Наружу ───────────────────────────────────────────────────────────────────

/// Новое личное сообщение: если переписка связана и написал владелец
/// аккаунта, оно уходит в чат на той стороне.
pub fn notify_dm(state: &AppState, message_id: &str) {
    if !enabled(state) {
        return;
    }
    let (app, message_id) = (state.clone(), message_id.to_owned());
    tokio::spawn(async move {
        let origin = crate::federation::our_origin(&app.config);
        let config = app.config.clone();
        let local = message_id.clone();
        let found = app
            .run(move |tx, _| Ok(outgoing(tx, &config, &origin, &local)?))
            .await;
        let Ok(Some((account, chat, message))) = found else {
            return;
        };
        let Some(link) = app.personal.link(&account) else {
            tracing::warn!(
                account,
                "личный мост: аккаунт не подключён, сообщение не ушло"
            );
            return;
        };
        match Box::pin(link.send(&chat, &message)).await {
            Ok(Some(remote_id)) => {
                let _ = app
                    .run(move |tx, _| Ok(remember(tx, &account, &chat, &remote_id, &message_id)?))
                    .await;
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(account, %err, "личный мост: сообщение не ушло"),
        }
    });
}

/// Аккаунт, чат и сообщение для отправки наружу.
fn outgoing(
    tx: &mut dyn Tx,
    config: &crate::config::Config,
    origin: &str,
    message_id: &str,
) -> DbResult<Option<(String, String, Outgoing)>> {
    let Some(row) = tx.query_one(
        "SELECT m.dm_channel_id, m.user_id, m.content, m.type, m.edited_at, m.reply_to_id \
         FROM dm_messages m WHERE m.id = ?",
        params![message_id],
    )?
    else {
        return Ok(None);
    };
    // Правки уходят отдельно (notify_edit).
    if row.opt_int("edited_at")?.is_some() {
        return Ok(None);
    }
    if row.opt_text("type")?.is_some_and(|kind| kind == "system") {
        return Ok(None);
    }
    let Some(link) = tx.query_one(
        "SELECT c.account_id, c.remote_chat FROM bs_personal_chats c \
         JOIN bs_personal_accounts a ON a.id = c.account_id \
         WHERE c.dm_channel_id = ? AND a.user_id = ?",
        params![&row.text("dm_channel_id")?, &row.text("user_id")?],
    )?
    else {
        return Ok(None);
    };
    let mut message = Outgoing {
        text: row.opt_text("content")?.unwrap_or_default(),
        files: Vec::new(),
        links: Vec::new(),
        reply_to: None,
    };
    for file in tx.query(
        "SELECT filename, original_name, mimetype FROM attachments WHERE dm_message_id = ? ORDER BY created_at",
        params![message_id],
    )? {
        let filename = file.text("filename")?;
        let name = file.text("original_name")?;
        message
            .links
            .push(format!("📎 {name}: {origin}/api/uploads/{filename}"));
        if let Some(path) = crate::files::upload_path(config, &filename) {
            message.files.push((
                name,
                path,
                file.opt_text("mimetype")?
                    .unwrap_or_else(|| "application/octet-stream".to_owned()),
            ));
        }
    }
    if let Some(reply) = row.opt_text("reply_to_id")? {
        message.reply_to = remote_of(tx, &reply)?.map(|(_, _, remote)| remote);
    }
    if message.text.trim().is_empty() && message.files.is_empty() {
        return Ok(None);
    }
    Ok(Some((
        link.text("account_id")?,
        link.text("remote_chat")?,
        message,
    )))
}

/// Владелец аккаунта связанной переписки — тот, кто действует.
fn owned_remote(
    tx: &mut dyn Tx,
    message_id: &str,
    actor: &str,
) -> DbResult<Option<(String, String, String)>> {
    let Some((account, chat, remote)) = remote_of(tx, message_id)? else {
        return Ok(None);
    };
    let owner = tx
        .query_one(
            "SELECT user_id FROM bs_personal_accounts WHERE id = ?",
            params![&account],
        )?
        .map(|row| row.text("user_id"))
        .transpose()?;
    Ok((owner.as_deref() == Some(actor)).then_some((account, chat, remote)))
}

/// Своя реакция здесь — на ту сторону. Telegram держит одну реакцию
/// обычного аккаунта: уходит последняя оставшаяся.
pub fn notify_reaction(state: &AppState, message_id: &str, user: &str) {
    if !enabled(state) {
        return;
    }
    let (app, message_id, user) = (state.clone(), message_id.to_owned(), user.to_owned());
    tokio::spawn(async move {
        // Вызов — изнутри транзакции реакции: дать ей закончиться.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let found = app
            .run(move |tx, _| {
                let Some(target) = owned_remote(tx, &message_id, &user)? else {
                    return Ok(None);
                };
                let last = tx
                    .query_one(
                        "SELECT emoji FROM dm_reactions WHERE dm_message_id = ? AND user_id = ? \
                         ORDER BY created_at DESC LIMIT 1",
                        params![&message_id, &user],
                    )?
                    .map(|row| row.text("emoji"))
                    .transpose()?;
                Ok(Some((target, last)))
            })
            .await;
        let Ok(Some(((account, chat, remote), emoji))) = found else {
            return;
        };
        if let Some(link) = app.personal.link(&account)
            && let Err(err) = link.react(&chat, &remote, emoji.as_deref()).await
        {
            tracing::warn!(account, %err, "личный мост: реакция не ушла");
        }
    });
}

/// Правка своего сообщения — на ту сторону (новые сообщения не трогает:
/// у них ещё нет пары на той стороне или нет `edited_at`).
pub fn notify_edit(state: &AppState, message_id: &str) {
    if !enabled(state) {
        return;
    }
    let (app, message_id) = (state.clone(), message_id.to_owned());
    tokio::spawn(async move {
        let found = app
            .run(move |tx, _| {
                let Some(row) = dm::message_row(tx, &message_id)? else {
                    return Ok(None);
                };
                if row.opt_int("edited_at")?.is_none() {
                    return Ok(None);
                }
                let author = row.text("user_id")?;
                let Some(target) = owned_remote(tx, &message_id, &author)? else {
                    return Ok(None);
                };
                Ok(row.opt_text("content")?.map(|text| (target, text)))
            })
            .await;
        let Ok(Some(((account, chat, remote), text))) = found else {
            return;
        };
        if let Some(link) = app.personal.link(&account)
            && let Err(err) = link.edit(&chat, &remote, &text).await
        {
            tracing::warn!(account, %err, "личный мост: правка не ушла");
        }
    });
}

/// Удаление своего сообщения — на ту сторону. Вызывать до удаления строки
/// сообщения: соответствие читается сразу.
pub fn notify_delete(state: &AppState, tx: &mut dyn Tx, message_id: &str, user: &str) {
    if !enabled(state) {
        return;
    }
    let Ok(Some((account, chat, remote))) = owned_remote(tx, message_id, user) else {
        return;
    };
    let _ = tx.execute(
        "DELETE FROM bs_personal_messages WHERE dm_message_id = ?",
        params![message_id],
    );
    let app = state.clone();
    tokio::spawn(async move {
        if let Some(link) = app.personal.link(&account)
            && let Err(err) = link.delete(&chat, &remote).await
        {
            tracing::warn!(account, %err, "личный мост: удаление не ушло");
        }
    });
}
