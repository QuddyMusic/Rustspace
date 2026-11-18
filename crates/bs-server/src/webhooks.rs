//! Вебхуки и карточки Rustspace (у оригинала их нет).
//!
//! Вебхук — адрес `POST /api/webhooks/:id/:token`, по которому внешняя
//! программа пишет в канал без входа. Тело — как у вебхуков Discord
//! (`content`, `username`, `avatar_url`, `embeds`), так что готовые
//! интеграции (CI, мониторинг, RSS) работают, если поменять адрес.
//!
//! Карточки (`embeds`) хранятся в таблице встраиваний оригинала как
//! `generic`: заголовок, текст, цвет полосы, ссылка и картинка. Поля, подпись
//! и автор Discord-карточки дописываются в текст разметкой, поэтому их видит
//! и веб-клиент оригинала.

use serde_json::{Value, json};

use crate::broadcast;
use crate::db::{DbResult, Tx};
use crate::error::{ApiError, ApiResult};
use crate::hub::Outbox;
use crate::messages::{MAX_MESSAGE_LENGTH, Shape, message_json, message_row};
use crate::params;
use crate::state::{AppState, now_ms};
use crate::util::utf16_len;

/// Не больше карточек в одном сообщении (как у Discord).
const MAX_EMBEDS: usize = 10;
/// Не больше полей в карточке.
const MAX_FIELDS: usize = 25;
/// Длина текста карточки вместе с полями.
const MAX_DESCRIPTION: usize = 6000;

/// Карточка сообщения.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Card {
    /// Заголовок.
    pub title: Option<String>,
    /// Текст (markdown).
    pub description: Option<String>,
    /// Ссылка заголовка.
    pub url: Option<String>,
    /// Цвет полосы, `#rrggbb`.
    pub color: Option<String>,
    /// Картинка.
    pub image: Option<String>,
}

impl Card {
    fn is_empty(&self) -> bool {
        self.title.is_none() && self.description.is_none() && self.image.is_none()
    }
}

/// Строка из поля, обрезанная и непустая.
fn text(value: &Value, key: &str, max: usize) -> Option<String> {
    let raw = value.get(key)?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    Some(if raw.chars().count() > max {
        raw.chars().take(max - 1).chain(['…']).collect()
    } else {
        raw.to_owned()
    })
}

/// Только `http(s)` — без `javascript:` и локальных схем.
fn web_url(value: Option<&Value>) -> Option<String> {
    let url = value?.as_str()?.trim();
    ((url.starts_with("https://") || url.starts_with("http://"))
        && url.len() <= 2048
        && !url.contains(char::is_whitespace))
    .then(|| url.to_owned())
}

/// Цвет Discord (число `0xRRGGBB`) или строка `#rrggbb`.
fn color(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(number) = value.as_u64() {
        return (number <= 0xff_ff_ff).then(|| format!("#{number:06x}"));
    }
    let hex = value.as_str()?.trim().trim_start_matches('#');
    (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_ascii_lowercase()))
}

/// Карточка из объекта Discord-embed.
fn card(value: &Value) -> Card {
    let mut description = String::new();
    if let Some(author) = value.get("author").and_then(|a| text(a, "name", 256)) {
        description.push_str(&format!("-# {author}\n"));
    }
    if let Some(body) = text(value, "description", 4096) {
        description.push_str(&body);
    }
    let fields = value
        .get("fields")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for field in fields.iter().take(MAX_FIELDS) {
        let (Some(name), Some(body)) = (text(field, "name", 256), text(field, "value", 1024))
        else {
            continue;
        };
        if !description.is_empty() {
            description.push('\n');
        }
        description.push_str(&format!("**{name}**\n{body}"));
    }
    if let Some(footer) = value.get("footer").and_then(|f| text(f, "text", 2048)) {
        if !description.is_empty() {
            description.push('\n');
        }
        description.push_str(&format!("-# {footer}"));
    }
    let description = (!description.is_empty()).then(|| {
        if description.chars().count() > MAX_DESCRIPTION {
            description
                .chars()
                .take(MAX_DESCRIPTION - 1)
                .chain(['…'])
                .collect()
        } else {
            description
        }
    });
    let image = ["image", "thumbnail"]
        .iter()
        .find_map(|key| web_url(value.get(*key).and_then(|i| i.get("url"))));
    Card {
        title: text(value, "title", 256),
        description,
        url: web_url(value.get("url")),
        color: color(value.get("color")),
        image,
    }
}

/// Карточки из поля `embeds` (не больше десяти, пустые выбрасываются).
///
/// # Errors
/// `embeds` — не массив.
pub fn cards(body: &Value) -> ApiResult<Vec<Card>> {
    match body.get("embeds") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(list)) => Ok(list
            .iter()
            .take(MAX_EMBEDS)
            .map(card)
            .filter(|card| !card.is_empty())
            .collect()),
        Some(_) => Err(ApiError::bad_request("embeds_invalid")),
    }
}

/// Текст сообщения: обрезанный, не длиннее предела.
///
/// # Errors
/// Слишком длинный текст.
pub fn content(body: &Value) -> ApiResult<Option<String>> {
    let Some(raw) = body.get("content").and_then(Value::as_str) else {
        return Ok(None);
    };
    if utf16_len(raw) > MAX_MESSAGE_LENGTH {
        return Err(ApiError::bad_request("content_too_long").detail("max", MAX_MESSAGE_LENGTH));
    }
    let trimmed = raw.trim();
    Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
}

/// Записывает сообщение с карточками от `author` и рассылает его.
///
/// # Errors
/// Ошибка базы.
#[allow(clippy::too_many_arguments, reason = "одна запись: всё нужно сразу")]
pub fn post(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    state: &AppState,
    space: &str,
    channel: &str,
    author: &str,
    content: Option<&str>,
    cards: &[Card],
) -> ApiResult<Value> {
    let id = state.ids.next();
    let created = now_ms();
    tx.execute(
        "INSERT INTO messages (id, channel_id, user_id, content, created_at) VALUES (?, ?, ?, ?, ?)",
        params![&id, channel, author, content.map(str::to_owned), created],
    )?;
    for card in cards {
        tx.execute(
            "INSERT INTO embeds (id, message_id, dm_message_id, url, embed_type, provider, title, \
             description, image, embed_url, width, height, color, created_at) \
             VALUES (?, ?, NULL, ?, 'generic', NULL, ?, ?, ?, NULL, NULL, NULL, ?, ?)",
            params![
                state.ids.next(),
                &id,
                card.url.clone().unwrap_or_default(),
                &card.title,
                &card.description,
                &card.image,
                &card.color,
                created
            ],
        )?;
    }
    let row = message_row(tx, &id)?.ok_or_else(ApiError::internal)?;
    let message =
        message_json(tx, &row, Shape::Event, true, true)?.ok_or_else(ApiError::internal)?;
    broadcast::to_channel(
        tx,
        &state.hub,
        out,
        space,
        channel,
        &json!({"type": "message_created", "message": message}),
        None,
    )?;
    Ok(message)
}

/// Записывает сообщение с карточками в личную переписку и рассылает его.
///
/// Только для переписок, которые не уходят наружу: через федерацию и личные
/// мосты карточки не доходят, там клиент пишет их разметкой
/// (`cards_unsupported`).
///
/// # Errors
/// Не участник, переписка не годится, ошибка базы.
pub fn post_dm(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    state: &AppState,
    channel: &str,
    author: &str,
    content: Option<&str>,
    cards: &[Card],
) -> ApiResult<Value> {
    if !crate::dm::is_member(tx, channel, author)? {
        return Err(ApiError::forbidden("not_dm_member"));
    }
    if crate::dm::is_dead_one_on_one(tx, channel, author)? {
        return Err(ApiError::forbidden("recipient_deleted"));
    }
    let outside = tx
        .query_one(
            "SELECT 1 AS one FROM dm_channels WHERE id = ? AND federated_id IS NOT NULL",
            params![channel],
        )?
        .is_some()
        || tx
            .query_one(
                "SELECT 1 AS one FROM bs_personal_chats WHERE dm_channel_id = ?",
                params![channel],
            )?
            .is_some();
    if outside {
        return Err(ApiError::conflict("cards_unsupported"));
    }
    let id = state.ids.next();
    let created = now_ms();
    tx.execute(
        "INSERT INTO dm_messages (id, dm_channel_id, user_id, content, created_at) \
         VALUES (?, ?, ?, ?, ?)",
        params![&id, channel, author, content.map(str::to_owned), created],
    )?;
    for card in cards {
        tx.execute(
            "INSERT INTO embeds (id, message_id, dm_message_id, url, embed_type, provider, title, \
             description, image, embed_url, width, height, color, created_at) \
             VALUES (?, NULL, ?, ?, 'generic', NULL, ?, ?, ?, NULL, NULL, NULL, ?, ?)",
            params![
                state.ids.next(),
                &id,
                card.url.clone().unwrap_or_default(),
                &card.title,
                &card.description,
                &card.image,
                &card.color,
                created
            ],
        )?;
    }
    let row = crate::dm::message_row(tx, &id)?.ok_or_else(ApiError::internal)?;
    let message = crate::dm::message_json(tx, &row)?.ok_or_else(ApiError::internal)?;
    crate::dm::broadcast_message(tx, out, channel, &message)?;
    Ok(message)
}

/// Пользователь-марионетка вебхука с именем `name`: находит или заводит,
/// обновляя аватар. Под ним нельзя войти (пароль «!»).
///
/// # Errors
/// Ошибка базы.
pub fn puppet(
    tx: &mut dyn Tx,
    state: &AppState,
    hook: &str,
    name: &str,
    avatar: Option<&str>,
) -> DbResult<String> {
    let external = format!("{hook}:{name}");
    if let Some(row) = tx.query_one(
        "SELECT user_id FROM bs_bridge_puppets WHERE kind = 'webhook' AND external_id = ?",
        params![&external],
    )? {
        let id = row.text("user_id")?;
        tx.execute(
            "UPDATE users SET avatar = ? WHERE id = ?",
            params![avatar.map(str::to_owned), &id],
        )?;
        return Ok(id);
    }
    let id = state.ids.next();
    tx.execute(
        "INSERT INTO users (id, username, display_name, avatar, password_hash, discoverable, created_at) \
         VALUES (?, ?, ?, ?, '!', 0, ?)",
        params![&id, format!("hook_{id}"), name, avatar.map(str::to_owned), now_ms()],
    )?;
    tx.execute(
        "INSERT INTO bs_bridge_puppets (user_id, kind, external_id) VALUES (?, 'webhook', ?)",
        params![&id, &external],
    )?;
    Ok(id)
}

/// Случайный секрет вебхука: 48 шестнадцатеричных знаков.
#[must_use]
pub fn new_token() -> String {
    let bytes: [u8; 24] = rand::random();
    bytes.iter().fold(String::with_capacity(48), |mut out, b| {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// Описание вебхука для его владельцев.
///
/// # Errors
/// Ошибка базы.
pub fn hook_json(row: &crate::db::Row, origin: &str) -> DbResult<Value> {
    let id = row.text("id")?;
    let token = row.text("token")?;
    Ok(json!({
        "id": id,
        "channelId": row.text("channel_id")?,
        "name": row.text("name")?,
        "avatar": row.opt_text("avatar")?,
        "token": token,
        "url": format!("{origin}/api/webhooks/{id}/{token}"),
        "createdAt": row.int("created_at")?,
    }))
}

#[cfg(test)]
mod tests {
    use super::{card, cards, color};
    use serde_json::json;

    #[test]
    fn discord_embed_becomes_a_card() {
        let card = card(&json!({
            "title": "Сборка прошла",
            "description": "main @ 1a2b3c",
            "url": "https://ci.example.org/42",
            "color": 0x2e_cc_71,
            "author": {"name": "CI"},
            "fields": [{"name": "Время", "value": "3 мин"}, {"name": "", "value": "пусто"}],
            "footer": {"text": "Rustspace CI"},
            "image": {"url": "javascript:alert(1)"},
            "thumbnail": {"url": "https://ci.example.org/ok.png"}
        }));
        assert_eq!(card.title.as_deref(), Some("Сборка прошла"));
        assert_eq!(card.color.as_deref(), Some("#2ecc71"));
        assert_eq!(card.image.as_deref(), Some("https://ci.example.org/ok.png"));
        assert_eq!(
            card.description.as_deref(),
            Some("-# CI\nmain @ 1a2b3c\n**Время**\n3 мин\n-# Rustspace CI")
        );
    }

    #[test]
    fn colors_and_limits() {
        assert_eq!(color(Some(&json!("#FFAA00"))).as_deref(), Some("#ffaa00"));
        assert_eq!(color(Some(&json!(0x1_00_00_00))), None);
        assert_eq!(color(Some(&json!("red"))), None);
        let many: Vec<_> = (0..15).map(|n| json!({"title": format!("{n}")})).collect();
        assert_eq!(
            cards(&json!({"embeds": many})).map(|c| c.len()).ok(),
            Some(10)
        );
        assert_eq!(
            cards(&json!({"embeds": [{}]})).map(|c| c.len()).ok(),
            Some(0)
        );
        assert!(cards(&json!({"embeds": "x"})).is_err());
    }
}
