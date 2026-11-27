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

mod import;
pub mod matrix;
pub mod mtproxy;
pub mod sync;
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
    /// Номер сообщения на той стороне (для правок, удалений и реакций).
    pub(crate) id: Option<String>,
    /// Когда написано, мс (история); `None` — сейчас.
    pub(crate) date: Option<i64>,
    /// Вложения.
    pub(crate) files: Vec<RemoteFile>,
    /// На какое сообщение той стороны ответ.
    pub(crate) reply_to: Option<String>,
}

/// Файл из чата на той стороне.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteFile {
    /// Имя файла.
    pub(crate) name: String,
    /// Содержимое.
    pub(crate) bytes: Vec<u8>,
}

/// Профиль собеседника на той стороне.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RemoteProfile {
    /// Фото: (стабильный id, байты JPEG); `None` — фото нет.
    pub(crate) photo: Option<(String, Vec<u8>)>,
    /// «О себе».
    pub(crate) bio: Option<String>,
    /// В сети ли сейчас (`None` — не знаем: группа, канал).
    pub(crate) online: Option<bool>,
}

/// Сообщение отсюда — наружу.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outgoing {
    /// Текст.
    pub(crate) text: String,
    /// Вложения: (имя, путь на диске, MIME).
    pub(crate) files: Vec<(String, std::path::PathBuf, String)>,
    /// Ссылки на вложения — для сетей, куда файлы не уходят.
    pub(crate) links: Vec<String>,
    /// Ответ на сообщение той стороны.
    pub(crate) reply_to: Option<String>,
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
    /// Что это: `person`, `bot`, `group` или `channel` — для разбивки в
    /// выборе чатов.
    pub(crate) kind: &'static str,
    /// Имя пользователя или чата (`@…`), если есть — для поиска.
    pub(crate) username: Option<String>,
}

/// Работающее подключение.
pub(crate) enum Link {
    /// Matrix.
    Matrix(matrix::Account),
    /// Telegram.
    Telegram(telegram::Account),
}

impl Link {
    /// Отправляет; номер нового сообщения той стороны, если он известен.
    async fn send(&self, chat: &str, message: &Outgoing) -> Result<Option<String>, String> {
        match self {
            Self::Matrix(account) => {
                let mut text = message.text.clone();
                for link in &message.links {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(link);
                }
                account.send(chat, &text).await.map(|()| None)
            }
            Self::Telegram(account) => Box::pin(account.send_full(chat, message)).await,
        }
    }

    /// Своя реакция на сообщение той стороны (`None` — снять).
    async fn react(&self, chat: &str, remote_id: &str, emoji: Option<&str>) -> Result<(), String> {
        match self {
            Self::Matrix(_) => Ok(()),
            Self::Telegram(account) => account.react(chat, remote_id, emoji).await,
        }
    }

    async fn edit(&self, chat: &str, remote_id: &str, text: &str) -> Result<(), String> {
        match self {
            Self::Matrix(_) => Ok(()),
            Self::Telegram(account) => account.edit(chat, remote_id, text).await,
        }
    }

    async fn delete(&self, chat: &str, remote_id: &str) -> Result<(), String> {
        match self {
            Self::Matrix(_) => Ok(()),
            Self::Telegram(account) => account.delete(chat, remote_id).await,
        }
    }

    /// Страница истории старее сообщения `before` (без него — самая новая);
    /// сообщения из `known` уже перенесены и не скачиваются.
    async fn history_page(
        &self,
        account: &str,
        chat: &str,
        before: Option<&str>,
        known: &std::collections::HashSet<String>,
    ) -> Result<import::HistoryPage, String> {
        match self {
            Self::Matrix(_) => Ok(import::HistoryPage {
                messages: Vec::new(),
                oldest: None,
                end: true,
            }),
            Self::Telegram(link) => link.history_page(account, chat, before, known).await,
        }
    }

    /// Кто из собеседников (личные чаты) сейчас в сети.
    async fn presence(&self, chats: &[String]) -> Vec<(String, bool)> {
        match self {
            Self::Matrix(_) => Vec::new(),
            Self::Telegram(link) => link.presence(chats).await,
        }
    }

    /// Профиль собеседника; `photo_known` — id уже загруженного фото.
    async fn profile(&self, chat: &str, photo_known: Option<&str>) -> Option<RemoteProfile> {
        match self {
            Self::Matrix(_) => None,
            Self::Telegram(account) => account.profile(chat, photo_known).await,
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
                Ok(link) => {
                    app.personal.insert(&id, link);
                    // Фото и «о себе» собеседников могли смениться, пока
                    // сервер стоял.
                    sync::connected(&app, &id);
                }
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
            import::forget(tx, &id, None)?;
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
                "kind": chat.kind,
                "username": chat.username,
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
    let (account, chat) = (id.clone(), remote_chat.clone());
    let linked = state
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
        .await?;
    // Новая связь: история чата и профиль собеседника — в фоне.
    if linked.get("id").is_some() {
        sync::after_link(state, &account, &chat);
    }
    Ok(linked)
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
            if let Some(row) = tx.query_one(
                "SELECT remote_chat FROM bs_personal_chats WHERE account_id = ? AND id = ?",
                params![&id, &chat],
            )? {
                import::forget(tx, &id, Some(&row.text("remote_chat")?))?;
            }
            tx.execute(
                "DELETE FROM bs_personal_chats WHERE account_id = ? AND id = ?",
                params![&id, &chat],
            )?;
            Ok(())
        })
        .await
}

/// Удаляет связанную с мостом переписку у себя целиком: связь с чатом,
/// историю и файлы. В самой сети (Telegram и т. п.) чат не трогается.
/// `false` — это не мостовая переписка этого пользователя.
///
/// # Errors
/// Ошибка базы.
pub(crate) async fn delete_bridged_dm(state: &AppState, user: &str, dm: &str) -> ApiResult<bool> {
    let (user, dm) = (user.to_owned(), dm.to_owned());
    let files = state
        .run(move |tx, out| {
            let Some(row) = tx.query_one(
                "SELECT c.account_id, c.remote_chat FROM bs_personal_chats c \
                 JOIN bs_personal_accounts a ON a.id = c.account_id \
                 WHERE c.dm_channel_id = ? AND a.user_id = ?",
                params![&dm, &user],
            )?
            else {
                return Ok(None);
            };
            let (account, chat) = (row.text("account_id")?, row.text("remote_chat")?);
            let mut files = Vec::new();
            for row in tx.query(
                "SELECT a.filename, a.thumbnail_filename FROM attachments a \
                 JOIN dm_messages m ON m.id = a.dm_message_id WHERE m.dm_channel_id = ?",
                params![&dm],
            )? {
                files.push(row.text("filename")?);
                files.extend(row.opt_text("thumbnail_filename")?);
            }
            tx.execute(
                "DELETE FROM bs_personal_messages WHERE account_id = ? AND remote_chat = ?",
                params![&account, &chat],
            )?;
            import::forget(tx, &account, Some(&chat))?;
            tx.execute(
                "DELETE FROM bs_personal_chats WHERE account_id = ? AND remote_chat = ?",
                params![&account, &chat],
            )?;
            // Сообщения, реакции и вложения уходят каскадом.
            tx.execute("DELETE FROM dm_channels WHERE id = ?", params![&dm])?;
            out.to_user(
                &user,
                &serde_json::json!({"type": "dm_channel_closed", "dmChannelId": dm}),
            );
            Ok(Some(files))
        })
        .await?;
    let Some(files) = files else {
        return Ok(false);
    };
    for name in files {
        crate::files::delete_upload_file(&state.config, &name).await;
    }
    Ok(true)
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

pub(crate) use sync::{deliver, notify_delete, notify_dm, notify_edit, notify_reaction};

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
