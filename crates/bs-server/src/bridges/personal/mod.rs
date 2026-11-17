//! Личные мосты: свои аккаунты в Matrix и Telegram в личных сообщениях.
//!
//! Пользователь подключает аккаунт (Matrix — токен доступа или логин и
//! пароль, Telegram — вход по номеру и коду через MTProto) и выбирает
//! чаты. Каждый выбранный чат становится личной перепиской с «марионеткой»:
//! служебным пользователем с именем вида «Анна · Telegram». Сообщения
//! собеседника приходят от неё; то, что пользователь пишет в эту переписку,
//! уходит в чат от его аккаунта. Свои сообщения, написанные с телефона,
//! тоже появляются — от самого пользователя.
//!
//! Секреты (токен Matrix, ключи сессии Telegram) хранятся в базе
//! зашифрованными ключом из `JWT_SECRET`. Discord сюда не входит: вход под
//! личным аккаунтом нарушает его правила.

pub mod matrix;
pub mod telegram;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use serde_json::{Value, json};
use sha2::Sha256;

use super::Kind;
use crate::db::{DbResult, Tx};
use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, now_ms};
use crate::{broadcast, dm, params};

/// Аккаунтов на пользователя не больше.
pub const MAX_ACCOUNTS: i64 = 10;
/// Связанных чатов на аккаунт не больше.
pub const MAX_CHATS: i64 = 100;

/// Сообщение из чата на той стороне.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Remote {
    /// Аккаунт Rustspace, через который пришло.
    pub(crate) account: String,
    /// Чат на той стороне.
    pub(crate) chat: String,
    /// Имя автора.
    pub(crate) sender: String,
    /// Текст.
    pub(crate) text: String,
    /// Написал сам владелец аккаунта (с другого устройства).
    pub(crate) mine: bool,
}

/// Чат на той стороне, который можно связать.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteChat {
    /// Идентификатор.
    pub(crate) id: String,
    /// Название или имя собеседника.
    pub(crate) title: String,
    /// Групповой чат: тогда к тексту добавляется имя автора.
    pub(crate) group: bool,
}

/// Работающее подключение.
pub(crate) enum Link {
    /// Matrix.
    Matrix(matrix::Account),
    /// Telegram.
    Telegram(telegram::Account),
}

impl Link {
    async fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        match self {
            Self::Matrix(account) => account.send(chat, text).await,
            Self::Telegram(account) => Box::pin(account.send(chat, text)).await,
        }
    }

    async fn chats(&self) -> Result<Vec<RemoteChat>, String> {
        match self {
            Self::Matrix(account) => account.chats().await,
            Self::Telegram(account) => account.chats().await,
        }
    }

    fn stop(&self) {
        match self {
            Self::Matrix(account) => account.stop(),
            Self::Telegram(account) => account.stop(),
        }
    }
}

/// Личные мосты в `AppState`: работающие подключения и незаконченные входы.
#[derive(Clone, Default)]
pub struct Personal {
    links: Arc<Mutex<HashMap<String, Arc<Link>>>>,
    pub(crate) logins: Arc<Mutex<HashMap<String, telegram::PendingLogin>>>,
}

impl std::fmt::Debug for Personal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Personal")
            .field("links", &self.links().len())
            .finish_non_exhaustive()
    }
}

impl Personal {
    fn links(&self) -> MutexGuard<'_, HashMap<String, Arc<Link>>> {
        self.links
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn logins(&self) -> MutexGuard<'_, HashMap<String, telegram::PendingLogin>> {
        self.logins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn link(&self, account: &str) -> Option<Arc<Link>> {
        self.links().get(account).cloned()
    }

    pub(crate) fn insert(&self, account: &str, link: Link) {
        let old = self.links().insert(account.to_owned(), Arc::new(link));
        if let Some(old) = old {
            old.stop();
        }
    }

    fn remove(&self, account: &str) {
        let old = self.links().remove(account);
        if let Some(old) = old {
            old.stop();
        }
    }
}

// ─── Секреты ──────────────────────────────────────────────────────────────────

fn cipher(server_secret: &str) -> Option<ChaCha20Poly1305> {
    let mut key = [0u8; 32];
    hkdf::Hkdf::<Sha256>::new(None, server_secret.as_bytes())
        .expand(b"rustspace personal bridges v1", &mut key)
        .ok()?;
    ChaCha20Poly1305::new_from_slice(&key).ok()
}

/// Шифрует секрет аккаунта для базы.
pub(crate) fn seal(state: &AppState, secret: &Value) -> Option<String> {
    let nonce: [u8; 12] = rand::random();
    let sealed = cipher(&state.config.jwt_secret)?
        .encrypt(Nonce::from_slice(&nonce), secret.to_string().as_bytes())
        .ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode([nonce.as_slice(), &sealed].concat()))
}

/// Расшифровывает секрет аккаунта.
pub(crate) fn open(state: &AppState, stored: &str) -> Option<Value> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(stored)
        .ok()?;
    let (nonce, sealed) = bytes.split_at_checked(12)?;
    let plain = cipher(&state.config.jwt_secret)?
        .decrypt(Nonce::from_slice(nonce), sealed)
        .ok()?;
    serde_json::from_slice(&plain).ok()
}

// ─── Запуск и состояние ───────────────────────────────────────────────────────

/// Включены ли личные мосты (`PERSONAL_BRIDGES=off` выключает).
#[must_use]
pub fn enabled(state: &AppState) -> bool {
    state.config.bridges.personal
}

/// Поднимает подключения всех аккаунтов при старте сервера.
pub fn spawn(state: &AppState) {
    if !enabled(state) {
        return;
    }
    let app = state.clone();
    tokio::spawn(async move {
        let accounts = app
            .run(|tx, _| {
                Ok(tx
                    .query(
                        "SELECT id, kind, secret FROM bs_personal_accounts WHERE state = 'active'",
                        params![],
                    )?
                    .iter()
                    .filter_map(|row| {
                        Some((
                            row.text("id").ok()?,
                            row.text("kind").ok()?,
                            row.text("secret").ok()?,
                        ))
                    })
                    .collect::<Vec<_>>())
            })
            .await
            .unwrap_or_default();
        for (id, kind, secret) in accounts {
            let Some(secret) = open(&app, &secret) else {
                set_error(
                    &app,
                    &id,
                    "не удалось расшифровать ключи (сменился JWT_SECRET?)",
                )
                .await;
                continue;
            };
            let started = match Kind::parse(&kind) {
                Some(Kind::Matrix) => matrix::resume(&app, &id, &secret).map(Link::Matrix),
                Some(Kind::Telegram) => telegram::resume(&app, &id, &secret)
                    .await
                    .map(Link::Telegram),
                _ => continue,
            };
            match started {
                Ok(link) => app.personal.insert(&id, link),
                Err(err) => set_error(&app, &id, &err).await,
            }
        }
    });
}

/// Помечает аккаунт ошибкой (вход отозван, ключи не подходят).
pub(crate) async fn set_error(state: &AppState, account: &str, error: &str) {
    tracing::warn!(account, error, "личный мост: ошибка аккаунта");
    let (account, error) = (account.to_owned(), error.to_owned());
    let _ = state
        .run(move |tx, _| {
            tx.execute(
                "UPDATE bs_personal_accounts SET state = 'error', error = ? WHERE id = ?",
                params![&error, &account],
            )?;
            Ok(())
        })
        .await;
}

/// Сохраняет новый секрет аккаунта (Telegram обновляет ключи сессии).
pub(crate) async fn save_secret(state: &AppState, account: &str, secret: &Value) {
    let Some(sealed) = seal(state, secret) else {
        return;
    };
    let account = account.to_owned();
    let _ = state
        .run(move |tx, _| {
            tx.execute(
                "UPDATE bs_personal_accounts SET secret = ? WHERE id = ?",
                params![&sealed, &account],
            )?;
            Ok(())
        })
        .await;
}

/// Заводит аккаунт после успешного входа (или обновляет ключи уже
/// подключённого); возвращает его идентификатор.
pub(crate) async fn add_account(
    state: &AppState,
    user: &str,
    kind: Kind,
    remote_id: &str,
    display: &str,
    secret: &Value,
) -> ApiResult<String> {
    let sealed = seal(state, secret).ok_or_else(ApiError::internal)?;
    let (app, user, remote_id, display) = (
        state.clone(),
        user.to_owned(),
        remote_id.to_owned(),
        display.to_owned(),
    );
    state
        .run(move |tx, _| {
            // Тот же аккаунт подключён снова — обновляем ключи.
            if let Some(row) = tx.query_one(
                "SELECT id FROM bs_personal_accounts WHERE user_id = ? AND kind = ? AND remote_id = ?",
                params![&user, kind.code(), &remote_id],
            )? {
                let id = row.text("id")?;
                tx.execute(
                    "UPDATE bs_personal_accounts SET secret = ?, display = ?, state = 'active', error = NULL WHERE id = ?",
                    params![&sealed, &display, &id],
                )?;
                return Ok(id);
            }
            let count = tx
                .query_one(
                    "SELECT count(*) AS n FROM bs_personal_accounts WHERE user_id = ?",
                    params![&user],
                )?
                .map_or(Ok(0), |row| row.int("n"))?;
            if count >= MAX_ACCOUNTS {
                return Err(ApiError::conflict("personal_bridge_limit").detail("max", MAX_ACCOUNTS));
            }
            let id = app.ids.next();
            tx.execute(
                "INSERT INTO bs_personal_accounts (id, user_id, kind, remote_id, display, secret, state, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, 'active', ?)",
                params![&id, &user, kind.code(), &remote_id, &display, &sealed, now_ms()],
            )?;
            Ok(id)
        })
        .await
}

/// Аккаунт пользователя в виде JSON.
pub(crate) async fn account_view(state: &AppState, user: &str, id: &str) -> ApiResult<Value> {
    let (app, user, id) = (state.clone(), user.to_owned(), id.to_owned());
    state
        .run(move |tx, _| {
            account_json(tx, &app, &user, &id)?
                .ok_or_else(|| ApiError::not_found("personal_bridge_not_found"))
        })
        .await
}

// ─── JSON ─────────────────────────────────────────────────────────────────────

/// Аккаунт с его чатами, если он принадлежит `viewer`.
pub(crate) fn account_json(
    tx: &mut dyn Tx,
    state: &AppState,
    viewer: &str,
    id: &str,
) -> ApiResult<Option<Value>> {
    let Some(row) = tx.query_one(
        "SELECT id, user_id, kind, remote_id, display, state, error, created_at \
         FROM bs_personal_accounts WHERE id = ?",
        params![id],
    )?
    else {
        return Ok(None);
    };
    if row.text("user_id")? != viewer {
        return Ok(None);
    }
    let mut chats = Vec::new();
    for chat in tx.query(
        "SELECT id, remote_chat, title, grouped, dm_channel_id, created_at \
         FROM bs_personal_chats WHERE account_id = ? ORDER BY created_at",
        params![id],
    )? {
        chats.push(json!({
            "id": chat.text("id")?,
            "remoteChat": chat.text("remote_chat")?,
            "title": chat.text("title")?,
            "group": chat.int("grouped")? != 0,
            "dmChannelId": chat.text("dm_channel_id")?,
            "createdAt": chat.int("created_at")?,
        }));
    }
    let connected = state.personal.link(id).is_some();
    Ok(Some(json!({
        "id": id,
        "kind": row.text("kind")?,
        "remoteId": row.text("remote_id")?,
        "display": row.text("display")?,
        "state": if row.text("state")? == "active" && !connected { "starting".to_owned() } else { row.text("state")? },
        "error": row.opt_text("error")?,
        "createdAt": row.int("created_at")?,
        "chats": chats,
    })))
}

/// Все аккаунты пользователя.
pub(crate) fn accounts_json(
    tx: &mut dyn Tx,
    state: &AppState,
    user: &str,
) -> ApiResult<Vec<Value>> {
    let ids: Vec<String> = tx
        .query(
            "SELECT id FROM bs_personal_accounts WHERE user_id = ? ORDER BY created_at",
            params![user],
        )?
        .iter()
        .filter_map(|row| row.text("id").ok())
        .collect();
    let mut out = Vec::new();
    for id in ids {
        if let Some(account) = account_json(tx, state, user, &id)? {
            out.push(account);
        }
    }
    Ok(out)
}

// ─── Действия ─────────────────────────────────────────────────────────────────

/// Проверяет, что аккаунт принадлежит пользователю; возвращает его сеть.
pub(crate) fn own_account(tx: &mut dyn Tx, user: &str, id: &str) -> ApiResult<Kind> {
    let row = tx
        .query_one(
            "SELECT user_id, kind FROM bs_personal_accounts WHERE id = ?",
            params![id],
        )?
        .ok_or_else(|| ApiError::not_found("personal_bridge_not_found"))?;
    if row.text("user_id")? != user {
        return Err(ApiError::not_found("personal_bridge_not_found"));
    }
    Kind::parse(&row.text("kind")?).ok_or_else(ApiError::internal)
}

/// Отключает и удаляет аккаунт; переписки остаются, но больше не связаны.
pub(crate) async fn remove_account(state: &AppState, user: &str, id: &str) -> ApiResult<()> {
    let (user, id) = (user.to_owned(), id.to_owned());
    let account = id.clone();
    state
        .run(move |tx, _| {
            own_account(tx, &user, &id)?;
            tx.execute(
                "DELETE FROM bs_personal_chats WHERE account_id = ?",
                params![&id],
            )?;
            tx.execute(
                "DELETE FROM bs_personal_accounts WHERE id = ?",
                params![&id],
            )?;
            Ok(())
        })
        .await?;
    if let Some(link) = state.personal.link(&account)
        && let Link::Telegram(telegram) = link.as_ref()
    {
        telegram.sign_out().await;
    }
    state.personal.remove(&account);
    Ok(())
}

/// Чаты на той стороне, отмеченные уже связанными.
pub(crate) async fn remote_chats(state: &AppState, user: &str, id: &str) -> ApiResult<Vec<Value>> {
    let (viewer, account) = (user.to_owned(), id.to_owned());
    let linked: Vec<String> = state
        .run(move |tx, _| {
            own_account(tx, &viewer, &account)?;
            Ok(tx
                .query(
                    "SELECT remote_chat FROM bs_personal_chats WHERE account_id = ?",
                    params![&account],
                )?
                .iter()
                .filter_map(|row| row.text("remote_chat").ok())
                .collect())
        })
        .await?;
    let link = state.personal.link(id).ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "personal_bridge_offline",
        )
    })?;
    let chats = link.chats().await.map_err(remote_error)?;
    Ok(chats
        .into_iter()
        .map(|chat| {
            json!({
                "id": chat.id,
                "title": chat.title,
                "group": chat.group,
                "linked": linked.contains(&chat.id),
            })
        })
        .collect())
}

/// Ошибка на той стороне — для клиента.
pub(crate) fn remote_error(message: String) -> ApiError {
    ApiError::new(
        axum::http::StatusCode::BAD_GATEWAY,
        "personal_bridge_remote_failed",
    )
    .detail("message", message)
}

/// Связывает чат: марионетка собеседника и личная переписка с ней.
pub(crate) async fn link_chat(
    state: &AppState,
    user: &str,
    id: &str,
    remote_chat: String,
    title: String,
    group: bool,
) -> ApiResult<Value> {
    let (app, user, id) = (state.clone(), user.to_owned(), id.to_owned());
    state
        .run(move |tx, out| {
            let kind = own_account(tx, &user, &id)?;
            if let Some(row) = tx.query_one(
                "SELECT dm_channel_id FROM bs_personal_chats WHERE account_id = ? AND remote_chat = ?",
                params![&id, &remote_chat],
            )? {
                return Ok(json!({"dmChannelId": row.text("dm_channel_id")?}));
            }
            let count = tx
                .query_one(
                    "SELECT count(*) AS n FROM bs_personal_chats WHERE account_id = ?",
                    params![&id],
                )?
                .map_or(Ok(0), |row| row.int("n"))?;
            if count >= MAX_CHATS {
                return Err(ApiError::conflict("personal_bridge_chat_limit").detail("max", MAX_CHATS));
            }
            let title = super::clip(&title, 48);
            let puppet = chat_puppet(tx, &app, kind, &id, &remote_chat, &title)?;
            let (channel, _) = dm::find_or_create_pair(tx, &app.ids, &user, &puppet, true)?;
            dm::set_closed(tx, &channel, &user, false)?;
            let chat_id = app.ids.next();
            tx.execute(
                "INSERT INTO bs_personal_chats (id, account_id, remote_chat, title, grouped, dm_channel_id, puppet_id, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                params![&chat_id, &id, &remote_chat, &title, i64::from(group), &channel, &puppet, now_ms()],
            )?;
            if let Some(wire) = dm::channel_wire(tx, &channel)? {
                broadcast::to_user(tx, out, &user, &json!({"type": "dm_channel_created", "dmChannel": wire}))?;
            }
            Ok(json!({"id": chat_id, "dmChannelId": channel}))
        })
        .await
}

/// Отвязывает чат (переписка остаётся).
pub(crate) async fn unlink_chat(
    state: &AppState,
    user: &str,
    id: &str,
    chat: &str,
) -> ApiResult<()> {
    let (user, id, chat) = (user.to_owned(), id.to_owned(), chat.to_owned());
    state
        .run(move |tx, _| {
            own_account(tx, &user, &id)?;
            tx.execute(
                "DELETE FROM bs_personal_chats WHERE account_id = ? AND id = ?",
                params![&id, &chat],
            )?;
            Ok(())
        })
        .await
}

/// Марионетка чата: одна на (аккаунт, чат), имя — «Название · Сеть».
fn chat_puppet(
    tx: &mut dyn Tx,
    state: &AppState,
    kind: Kind,
    account: &str,
    chat: &str,
    title: &str,
) -> DbResult<String> {
    let display = format!("{title} · {}", kind.title());
    let external = format!("p:{account}:{chat}");
    if let Some(row) = tx.query_one(
        "SELECT user_id FROM bs_bridge_puppets WHERE kind = ? AND external_id = ?",
        params![kind.code(), &external],
    )? {
        let id = row.text("user_id")?;
        tx.execute(
            "UPDATE users SET display_name = ? WHERE id = ?",
            params![&display, &id],
        )?;
        return Ok(id);
    }
    let id = state.ids.next();
    let username = format!("{}_p{id}", kind.prefix());
    tx.execute(
        // Пароль «!» не проходит ни одну проверку bcrypt: войти нельзя.
        "INSERT INTO users (id, username, display_name, password_hash, discoverable, created_at) \
         VALUES (?, ?, ?, '!', 0, ?)",
        params![&id, &username, &display, now_ms()],
    )?;
    tx.execute(
        "INSERT INTO bs_bridge_puppets (user_id, kind, external_id) VALUES (?, ?, ?)",
        params![&id, kind.code(), &external],
    )?;
    Ok(id)
}

// ─── Внутрь ───────────────────────────────────────────────────────────────────

/// Пишет сообщение с той стороны в связанную переписку.
pub(crate) async fn deliver(state: &AppState, message: Remote) {
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
            } else if row.int("grouped")? != 0 {
                (
                    row.text("puppet_id")?,
                    format!("**{}**: {}", super::clip(&message.sender, 48), message.text),
                )
            } else {
                (row.text("puppet_id")?, message.text.clone())
            };
            let content: String = content.chars().take(4000).collect();
            let new = dm::NewMessage {
                content: Some(content),
                attachments: Vec::new(),
                reply_to: None,
            };
            dm::create_message(tx, out, &app.config, &app.ids, &channel, &author, &new)?;
            Ok(())
        })
        .await;
    if let Err(err) = result {
        tracing::warn!(?err, "личный мост: не удалось записать сообщение");
    }
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
        let found = app
            .run(move |tx, _| Ok(outgoing(tx, &origin, &message_id)?))
            .await;
        let Ok(Some((account, chat, text))) = found else {
            return;
        };
        let Some(link) = app.personal.link(&account) else {
            tracing::warn!(
                account,
                "личный мост: аккаунт не подключён, сообщение не ушло"
            );
            return;
        };
        if let Err(err) = Box::pin(link.send(&chat, &text)).await {
            tracing::warn!(account, %err, "личный мост: сообщение не ушло");
        }
    });
}

/// Аккаунт, чат и текст для отправки наружу.
fn outgoing(
    tx: &mut dyn Tx,
    origin: &str,
    message_id: &str,
) -> DbResult<Option<(String, String, String)>> {
    let Some(row) = tx.query_one(
        "SELECT m.dm_channel_id, m.user_id, m.content, m.type, m.edited_at FROM dm_messages m WHERE m.id = ?",
        params![message_id],
    )?
    else {
        return Ok(None);
    };
    // Правки наружу не уходят (на той стороне это был бы дубль).
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
    let mut text = row.opt_text("content")?.unwrap_or_default();
    for file in tx.query(
        "SELECT filename, original_name FROM attachments WHERE dm_message_id = ? ORDER BY created_at",
        params![message_id],
    )? {
        let url = format!("{origin}/api/uploads/{}", file.text("filename")?);
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!("📎 {}: {url}", file.text("original_name")?));
    }
    if text.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some((
        link.text("account_id")?,
        link.text("remote_chat")?,
        text,
    )))
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    #[test]
    fn secrets_round_trip_and_need_the_key() {
        let key = "0123456789abcdef0123456789abcdef";
        let other = "fedcba9876543210fedcba9876543210";
        let sealed = {
            let nonce: [u8; 12] = rand::random();
            let sealed = cipher(key)
                .expect("ключ")
                .encrypt(Nonce::from_slice(&nonce), b"{\"token\":\"x\"}".as_slice())
                .expect("шифр");
            [nonce.as_slice(), &sealed].concat()
        };
        let (nonce, body) = sealed.split_at(12);
        assert!(
            cipher(key)
                .expect("ключ")
                .decrypt(Nonce::from_slice(nonce), body)
                .is_ok()
        );
        assert!(
            cipher(other)
                .expect("ключ")
                .decrypt(Nonce::from_slice(nonce), body)
                .is_err()
        );
    }
}
