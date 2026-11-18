//! Мосты в другие сети: Telegram, Discord и Matrix.
//!
//! Канал Rustspace связывается с внешним чатом (группой Telegram, каналом
//! Discord, комнатой Matrix). Сообщения ходят в обе стороны:
//!
//! - **внутрь** — бот сети получает сообщение, мост пишет его в связанные
//!   каналы от имени «марионетки» — служебного пользователя с именем вида
//!   «Олег · Telegram» (войти под ним нельзя);
//! - **наружу** — новое сообщение канала уходит в связанные чаты через бота
//!   (в Discord — через вебхук, чтобы было видно имя автора).
//!
//! Сообщения марионеток наружу не пересылаются, а свои сообщения боты
//! пропускают — так мост не зацикливается. Токены ботов задаёт
//! администратор сервера в переменных окружения, связи — тот, кто может
//! управлять каналами пространства (`/api/rustspace/channels/:id/bridges`).

mod discord;
mod matrix;
pub mod personal;
mod telegram;

use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::db::{DbResult, Tx};
use crate::messages::{Shape, message_json, message_row};
use crate::state::{AppState, now_ms};
use crate::{broadcast, params};

/// Сеть на той стороне моста.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Telegram (Bot API).
    Telegram,
    /// Discord (бот и вебхуки).
    Discord,
    /// Matrix (клиентский API).
    Matrix,
}

impl Kind {
    /// Все сети по порядку.
    pub const ALL: [Self; 3] = [Self::Telegram, Self::Discord, Self::Matrix];

    /// Имя в API и базе.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Discord => "discord",
            Self::Matrix => "matrix",
        }
    }

    /// Сеть по имени.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.code() == text)
    }

    /// Подпись сети в имени марионетки.
    const fn title(self) -> &'static str {
        match self {
            Self::Telegram => "Telegram",
            Self::Discord => "Discord",
            Self::Matrix => "Matrix",
        }
    }

    /// Приставка имени пользователя марионетки.
    const fn prefix(self) -> &'static str {
        match self {
            Self::Telegram => "tg",
            Self::Discord => "dc",
            Self::Matrix => "mx",
        }
    }

    /// Похож ли идентификатор чата на настоящий для этой сети.
    #[must_use]
    pub fn valid_chat(self, id: &str) -> bool {
        match self {
            // -1001234567890 или 123456789.
            Self::Telegram => {
                let digits = id.strip_prefix('-').unwrap_or(id);
                !digits.is_empty()
                    && digits.len() <= 20
                    && digits.bytes().all(|b| b.is_ascii_digit())
            }
            // Снежинка Discord.
            Self::Discord => {
                !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
            }
            // !opaque:server.example.
            Self::Matrix => {
                id.len() <= 255
                    && id.starts_with('!')
                    && id.contains(':')
                    && !id.contains(char::is_whitespace)
            }
        }
    }
}

/// Настройки мостов из окружения. `Debug` прячет токены.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct BridgeConfig {
    /// `TELEGRAM_BOT_TOKEN`.
    pub telegram_token: Option<String>,
    /// `TELEGRAM_API_URL` (для тестов; по умолчанию `https://api.telegram.org`).
    pub telegram_api: String,
    /// `DISCORD_BOT_TOKEN`.
    pub discord_token: Option<String>,
    /// `DISCORD_API_URL` (по умолчанию `https://discord.com/api/v10`).
    pub discord_api: String,
    /// `MATRIX_HOMESERVER`, например `https://matrix.example.org`.
    pub matrix_homeserver: Option<String>,
    /// `MATRIX_ACCESS_TOKEN` пользователя-бота.
    pub matrix_token: Option<String>,
    /// Личные мосты пользователей (`PERSONAL_BRIDGES`, по умолчанию включены).
    pub personal: bool,
    /// `TELEGRAM_API_ID` приложения с my.telegram.org — для личных мостов.
    pub telegram_api_id: Option<i32>,
    /// `TELEGRAM_API_HASH` к нему.
    pub telegram_api_hash: Option<String>,
}

impl std::fmt::Debug for BridgeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeConfig")
            .field("telegram", &self.telegram_token.is_some())
            .field("discord", &self.discord_token.is_some())
            .field("matrix", &self.matrix_homeserver)
            .field("personal", &self.personal)
            .field("telegram_app", &self.telegram_api_id)
            .finish_non_exhaustive()
    }
}

impl BridgeConfig {
    /// Сети, для которых заданы токены.
    #[must_use]
    pub fn available(&self) -> Vec<Kind> {
        let mut out = Vec::new();
        if self.telegram_token.is_some() {
            out.push(Kind::Telegram);
        }
        if self.discord_token.is_some() {
            out.push(Kind::Discord);
        }
        if self.matrix_homeserver.is_some() && self.matrix_token.is_some() {
            out.push(Kind::Matrix);
        }
        out
    }
}

/// Сообщение из внешней сети.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Incoming {
    kind: Kind,
    /// Чат на той стороне.
    chat: String,
    /// Автор на той стороне.
    user: String,
    /// Имя автора для показа.
    name: String,
    /// Текст (вложения — описанием и ссылками).
    text: String,
}

/// Что уходит наружу: автор и текст с адресами вложений.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outgoing {
    /// Имя автора.
    name: String,
    /// Текст сообщения.
    text: String,
}

/// Отправители по сетям; заполняются при запуске мостов.
#[derive(Default)]
struct Senders {
    telegram: Option<telegram::Sender>,
    discord: Option<discord::Sender>,
    matrix: Option<matrix::Sender>,
}

/// Состояние мостов в `AppState`: очередь исходящих.
#[derive(Clone, Default)]
pub struct Bridges {
    queue: Arc<OnceLock<mpsc::UnboundedSender<String>>>,
}

impl std::fmt::Debug for Bridges {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridges")
            .field("running", &self.queue.get().is_some())
            .finish()
    }
}

/// Сообщение канала создано: если канал связан, оно уйдёт наружу.
/// Ничего не делает, пока мосты не запущены.
pub fn notify(state: &AppState, message_id: &str) {
    if let Some(queue) = state.bridges.queue.get() {
        let _ = queue.send(message_id.to_owned());
    }
}

/// Запускает мосты, для которых заданы токены.
pub fn spawn(state: &AppState) {
    let config = state.config.bridges.clone();
    let available = config.available();
    if available.is_empty() {
        return;
    }
    let (queue, mut messages) = mpsc::unbounded_channel::<String>();
    if state.bridges.queue.set(queue).is_err() {
        return;
    }
    let (incoming_tx, mut incoming) = mpsc::unbounded_channel::<Incoming>();
    let mut senders = Senders::default();
    if let Some(token) = config.telegram_token.clone() {
        senders.telegram = Some(telegram::start(
            &config.telegram_api,
            token,
            incoming_tx.clone(),
        ));
    }
    if let Some(token) = config.discord_token.clone() {
        senders.discord = Some(discord::start(
            &config.discord_api,
            token,
            incoming_tx.clone(),
        ));
    }
    if let (Some(homeserver), Some(token)) = (&config.matrix_homeserver, config.matrix_token) {
        senders.matrix = Some(matrix::start(homeserver, token, incoming_tx));
    }
    let names: Vec<_> = available.iter().map(|kind| kind.code()).collect();
    tracing::info!(bridges = ?names, "мосты запущены");

    let inbound = state.clone();
    tokio::spawn(async move {
        while let Some(message) = incoming.recv().await {
            if let Err(err) = deliver(&inbound, message).await {
                tracing::warn!(?err, "мост: не удалось записать сообщение в канал");
            }
        }
    });
    let outbound = state.clone();
    tokio::spawn(async move {
        // Одна очередь — порядок сообщений сохраняется.
        while let Some(message_id) = messages.recv().await {
            relay(&outbound, &senders, message_id).await;
        }
    });
}

/// Пересылает сообщение канала во все связанные чаты.
async fn relay(state: &AppState, senders: &Senders, message_id: String) {
    let origin = crate::federation::our_origin(&state.config);
    let prepared = state
        .run(move |tx, _| Ok(outgoing(tx, &origin, &message_id)?))
        .await;
    let (message, links) = match prepared {
        Ok(Some(found)) => found,
        Ok(None) => return,
        Err(err) => {
            tracing::warn!(?err, "мост: не удалось прочитать сообщение");
            return;
        }
    };
    for (kind, chat) in links {
        let sent = match kind {
            Kind::Telegram => match &senders.telegram {
                Some(sender) => sender.send(&chat, &message).await,
                None => continue,
            },
            Kind::Discord => match &senders.discord {
                Some(sender) => sender.send(&chat, &message).await,
                None => continue,
            },
            Kind::Matrix => match &senders.matrix {
                Some(sender) => sender.send(&chat, &message).await,
                None => continue,
            },
        };
        if let Err(err) = sent {
            tracing::warn!(bridge = kind.code(), chat, %err, "мост: сообщение не отправлено");
        }
    }
}

/// Сообщение для отправки наружу и связанные чаты; `None` — канал не
/// связан или автор сам пришёл из моста.
fn outgoing(
    tx: &mut dyn Tx,
    origin: &str,
    message_id: &str,
) -> DbResult<Option<(Outgoing, Vec<(Kind, String)>)>> {
    let Some(row) = tx.query_one(
        "SELECT m.channel_id, m.user_id, m.content, m.type, m.edited_at, u.username, u.display_name \
         FROM messages m JOIN users u ON u.id = m.user_id WHERE m.id = ?",
        params![message_id],
    )?
    else {
        return Ok(None);
    };
    // Правка приходит тем же путём, что и новое сообщение: наружу не шлём,
    // иначе на той стороне появится дубль.
    if row.opt_int("edited_at")?.is_some() {
        return Ok(None);
    }
    if row.opt_text("type")?.is_some_and(|kind| kind == "system") {
        return Ok(None);
    }
    let channel = row.text("channel_id")?;
    let links: Vec<(Kind, String)> = tx
        .query(
            "SELECT kind, external_id FROM bs_bridge_links WHERE channel_id = ? ORDER BY created_at",
            params![&channel],
        )?
        .iter()
        .filter_map(|link| {
            let kind = Kind::parse(&link.text("kind").ok()?)?;
            Some((kind, link.text("external_id").ok()?))
        })
        .collect();
    if links.is_empty() {
        return Ok(None);
    }
    let author = row.text("user_id")?;
    if tx
        .query_one(
            "SELECT user_id FROM bs_bridge_puppets WHERE user_id = ?",
            params![&author],
        )?
        .is_some()
    {
        return Ok(None);
    }
    let mut text = row.opt_text("content")?.unwrap_or_default();
    for file in tx.query(
        "SELECT filename, original_name FROM attachments WHERE message_id = ? ORDER BY created_at",
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
    let name = row
        .opt_text("display_name")?
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(row.text("username")?);
    Ok(Some((Outgoing { name, text }, links)))
}

/// Пишет внешнее сообщение во все связанные каналы.
async fn deliver(state: &AppState, message: Incoming) -> crate::error::ApiResult<()> {
    let app = state.clone();
    let delivered: Vec<String> = state
        .run(move |tx, out| {
            let channels: Vec<String> = tx
                .query(
                    "SELECT channel_id FROM bs_bridge_links WHERE kind = ? AND external_id = ?",
                    params![message.kind.code(), &message.chat],
                )?
                .iter()
                .filter_map(|row| row.text("channel_id").ok())
                .collect();
            if channels.is_empty() {
                return Ok(Vec::new());
            }
            let puppet = puppet(tx, &app, &message)?;
            let mut created = Vec::new();
            for channel in channels {
                let Some(space) = crate::messages::channel_space(tx, &channel)? else {
                    continue;
                };
                let id = app.ids.next();
                tx.execute(
                    "INSERT INTO messages (id, channel_id, user_id, content, created_at) \
                     VALUES (?, ?, ?, ?, ?)",
                    params![&id, &channel, &puppet, &message.text, now_ms()],
                )?;
                if let Some(row) = message_row(tx, &id)?
                    && let Some(json) = message_json(tx, &row, Shape::Event, true, true)?
                {
                    broadcast::to_channel(
                        tx,
                        &app.hub,
                        out,
                        &space,
                        &channel,
                        &json!({"type": "message_created", "message": json}),
                        None,
                    )?;
                }
                created.push(id);
            }
            Ok(created)
        })
        .await?;
    if !delivered.is_empty() {
        tracing::debug!(count = delivered.len(), "мост: сообщение доставлено");
    }
    Ok(())
}

/// Марионетка автора: находит или заводит, обновляя имя.
fn puppet(tx: &mut dyn Tx, state: &AppState, message: &Incoming) -> DbResult<String> {
    let display = format!("{} · {}", clip(&message.name, 48), message.kind.title());
    if let Some(row) = tx.query_one(
        "SELECT user_id FROM bs_bridge_puppets WHERE kind = ? AND external_id = ?",
        params![message.kind.code(), &message.user],
    )? {
        let id = row.text("user_id")?;
        tx.execute(
            "UPDATE users SET display_name = ? WHERE id = ? AND (display_name IS NULL OR display_name <> ?)",
            params![&display, &id, &display],
        )?;
        return Ok(id);
    }
    let id = state.ids.next();
    let username = puppet_username(message.kind, &message.user);
    tx.execute(
        // Пароль «!» не проходит ни одну проверку bcrypt: войти нельзя.
        "INSERT INTO users (id, username, display_name, password_hash, discoverable, created_at) \
         VALUES (?, ?, ?, '!', 0, ?)",
        params![&id, &username, &display, now_ms()],
    )?;
    tx.execute(
        "INSERT INTO bs_bridge_puppets (user_id, kind, external_id) VALUES (?, ?, ?)",
        params![&id, message.kind.code(), &message.user],
    )?;
    Ok(id)
}

/// Имя пользователя марионетки: приставка сети и внешний идентификатор
/// (только латиница, цифры и `_`).
fn puppet_username(kind: Kind, external: &str) -> String {
    let safe: String = external
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    format!("{}_{safe}", kind.prefix())
}

/// Не длиннее `max` символов.
fn clip(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        trimmed.to_owned()
    } else {
        trimmed.chars().take(max - 1).chain(['…']).collect()
    }
}

/// Ответ на команду «какой у чата ID».
fn id_reply(chat: &str) -> String {
    format!(
        "Rustspace: ID этого чата — {chat}\n\
         Укажите его в настройках канала Rustspace → «Мосты». / Use it in Rustspace channel settings → Bridges."
    )
}

/// Похоже ли сообщение на команду «какой у чата ID».
fn asks_id(text: &str) -> bool {
    let first = text.split_whitespace().next().unwrap_or_default();
    let command = first.split('@').next().unwrap_or_default();
    matches!(command, "/rustspace_id" | "!rustspace_id")
}

/// HTTP-клиент мостов.
fn client(timeout: std::time::Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(concat!("rustspace-server/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}

/// Пауза перед новой попыткой: растёт до минуты.
fn backoff(attempt: u32) -> std::time::Duration {
    std::time::Duration::from_secs(u64::from(attempt.min(6)).pow(2).clamp(2, 60))
}

/// Конфигурация мостов из переменных окружения.
pub(crate) fn config_from(get: &dyn Fn(&str) -> Option<String>) -> BridgeConfig {
    let non_empty = |key: &str| {
        get(key)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    BridgeConfig {
        telegram_token: non_empty("TELEGRAM_BOT_TOKEN"),
        telegram_api: non_empty("TELEGRAM_API_URL")
            .unwrap_or_else(|| "https://api.telegram.org".to_owned())
            .trim_end_matches('/')
            .to_owned(),
        discord_token: non_empty("DISCORD_BOT_TOKEN"),
        discord_api: non_empty("DISCORD_API_URL")
            .unwrap_or_else(|| "https://discord.com/api/v10".to_owned())
            .trim_end_matches('/')
            .to_owned(),
        matrix_homeserver: non_empty("MATRIX_HOMESERVER")
            .map(|url| url.trim_end_matches('/').to_owned()),
        matrix_token: non_empty("MATRIX_ACCESS_TOKEN"),
        personal: non_empty("PERSONAL_BRIDGES").is_none_or(|v| {
            !matches!(
                v.to_ascii_lowercase().as_str(),
                "0" | "off" | "false" | "no"
            )
        }),
        telegram_api_id: non_empty("TELEGRAM_API_ID")
            .or_else(|| built_in(option_env!("RUSTSPACE_TELEGRAM_API_ID")))
            .and_then(|v| v.parse().ok()),
        telegram_api_hash: non_empty("TELEGRAM_API_HASH")
            .or_else(|| built_in(option_env!("RUSTSPACE_TELEGRAM_API_HASH"))),
    }
}

/// Ключи приложения Telegram, вшитые при сборке выпуска (секреты CI
/// `RUSTSPACE_TELEGRAM_API_ID`/`_HASH`): с ними личные мосты Telegram работают
/// на любом сервере без настройки. Переменные окружения сервера важнее.
fn built_in(value: Option<&'static str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

// ─── Связи (для маршрутов) ──────────────────────────────────────────────────

/// Связи канала в виде JSON.
///
/// # Errors
/// Ошибка базы.
pub fn links_json(tx: &mut dyn Tx, channel_id: &str) -> DbResult<Vec<Value>> {
    tx.query(
        "SELECT id, kind, external_id, created_at FROM bs_bridge_links \
         WHERE channel_id = ? ORDER BY created_at",
        params![channel_id],
    )?
    .iter()
    .map(|row| {
        Ok(json!({
            "id": row.text("id")?,
            "kind": row.text("kind")?,
            "externalId": row.text("external_id")?,
            "createdAt": row.int("created_at")?,
        }))
    })
    .collect()
}

/// Сведения о мостах для маршрутов.
#[must_use]
pub fn available_codes(config: &Config) -> Vec<&'static str> {
    config
        .bridges
        .available()
        .into_iter()
        .map(Kind::code)
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    #[test]
    fn chat_ids_are_checked_per_network() {
        assert!(Kind::Telegram.valid_chat("-1001234567890"));
        assert!(Kind::Telegram.valid_chat("42"));
        assert!(!Kind::Telegram.valid_chat("-"));
        assert!(!Kind::Telegram.valid_chat("@group"));
        assert!(Kind::Discord.valid_chat("1234567890123456789"));
        assert!(!Kind::Discord.valid_chat("-12"));
        assert!(Kind::Matrix.valid_chat("!abc:matrix.org"));
        assert!(!Kind::Matrix.valid_chat("#room:matrix.org"));
        assert!(!Kind::Matrix.valid_chat("!no spaces:x"));
    }

    #[test]
    fn puppet_names_are_safe_and_short() {
        assert_eq!(puppet_username(Kind::Telegram, "42"), "tg_42");
        assert_eq!(
            puppet_username(Kind::Matrix, "@Олег:matrix.org"),
            "mx_______matrix_org"
        );
        assert_eq!(clip("  Длинное имя  ", 5), "Длин…");
    }

    #[test]
    fn id_command_is_recognised() {
        assert!(asks_id("/rustspace_id"));
        assert!(asks_id("/rustspace_id@my_bot please"));
        assert!(asks_id("!rustspace_id"));
        assert!(!asks_id("what is /rustspace_id"));
    }

    #[test]
    fn tokens_stay_out_of_debug() {
        let config =
            config_from(&|key| (key == "TELEGRAM_BOT_TOKEN").then(|| "secret-token".to_owned()));
        assert_eq!(config.available(), vec![Kind::Telegram]);
        assert!(!format!("{config:?}").contains("secret-token"));
    }
}
