//! Вебхуки каналов и сообщения с карточками (`/api/rustspace/...`; у
//! оригинала их нет).

use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::error::ApiError;

/// Вебхук канала.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Webhook {
    /// Идентификатор.
    pub id: String,
    /// Имя, под которым пишет.
    pub name: String,
    /// Адрес для внешней программы (с секретом).
    pub url: String,
    /// Когда создан, мс.
    pub created_at: i64,
}

/// Карточка сообщения (как embed у Discord, без полей).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CardDraft {
    /// Заголовок.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Текст (markdown).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Ссылка заголовка.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Цвет полосы `#rrggbb`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Картинка.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<CardImage>,
}

/// Картинка карточки `{url}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardImage {
    /// Адрес.
    pub url: String,
}

#[derive(Serialize)]
struct NewHook<'a> {
    name: &'a str,
}

#[derive(Serialize)]
struct CardMessage<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
    embeds: &'a [CardDraft],
}

impl ApiClient {
    /// `GET /api/rustspace/channels/:id/webhooks`.
    ///
    /// # Errors
    /// Нет права управлять каналами; сервер без вебхуков (404).
    pub async fn webhooks(&self, channel_id: &str) -> Result<Vec<Webhook>, ApiError> {
        self.get_json(&format!("/api/rustspace/channels/{channel_id}/webhooks"))
            .await
    }

    /// `POST /api/rustspace/channels/:id/webhooks`.
    ///
    /// # Errors
    /// Неверное имя, слишком много вебхуков, нет права.
    pub async fn create_webhook(&self, channel_id: &str, name: &str) -> Result<Webhook, ApiError> {
        self.post_json(
            &format!("/api/rustspace/channels/{channel_id}/webhooks"),
            &NewHook { name },
        )
        .await
    }

    /// `DELETE /api/rustspace/webhooks/:id`.
    ///
    /// # Errors
    /// Вебхука нет или нет права.
    pub async fn delete_webhook(&self, hook_id: &str) -> Result<(), ApiError> {
        self.delete_json::<serde_json::Value>(&format!("/api/rustspace/webhooks/{hook_id}"))
            .await?;
        Ok(())
    }

    /// `POST /api/rustspace/channels/:id/cards` — сообщение с карточками.
    ///
    /// # Errors
    /// Нет права писать; сервер без карточек (404).
    pub async fn send_cards(
        &self,
        channel_id: &str,
        content: Option<&str>,
        cards: &[CardDraft],
    ) -> Result<(), ApiError> {
        self.post_json::<serde_json::Value, _>(
            &format!("/api/rustspace/channels/{channel_id}/cards"),
            &CardMessage {
                content,
                embeds: cards,
            },
        )
        .await?;
        Ok(())
    }

    /// `POST /api/rustspace/dm/:id/cards` — карточки в личной переписке.
    ///
    /// # Errors
    /// Не участник; переписка уходит наружу (409 `cards_unsupported`); сервер без
    /// карточек в ЛС (404).
    pub async fn send_dm_cards(
        &self,
        channel_id: &str,
        content: Option<&str>,
        cards: &[CardDraft],
    ) -> Result<(), ApiError> {
        self.post_json::<serde_json::Value, _>(
            &format!("/api/rustspace/dm/{channel_id}/cards"),
            &CardMessage {
                content,
                embeds: cards,
            },
        )
        .await?;
        Ok(())
    }
}

/// Битрейт голоса звонка (`GET /api/rustspace/voice-bitrate/:target`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VoiceBitrate {
    /// Битрейт инстанса, кбит/с.
    pub instance_kbps: u32,
    /// Пониженный для канала или переписки.
    pub override_kbps: Option<u32>,
    /// Действующий.
    pub effective_kbps: u32,
    /// Можно ли менять.
    pub can_edit: bool,
    /// Ниже нельзя.
    pub min_kbps: u32,
}

/// Битрейт голоса инстанса (`GET /api/rustspace/admin/voice-bitrate`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct InstanceVoiceBitrate {
    /// Битрейт, кбит/с.
    pub kbps: u32,
    /// Пределы.
    pub min: u32,
    /// Пределы.
    pub max: u32,
}

#[derive(Serialize)]
struct Kbps {
    kbps: Option<u32>,
}

impl ApiClient {
    /// Запрещено ли давать ссылку на профиль пользователя при пересылке
    /// (`GET /api/rustspace/users/:id/privacy`). Сервер без этой настройки
    /// (оригинал) — не запрещено.
    ///
    /// # Errors
    /// Ошибки запроса, кроме «нет такого маршрута».
    pub async fn hides_forward_author(&self, user_id: &str) -> Result<bool, ApiError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Privacy {
            hide_forward_author: bool,
        }
        match self
            .get_json::<Privacy>(&format!("/api/rustspace/users/{user_id}/privacy"))
            .await
        {
            Ok(privacy) => Ok(privacy.hide_forward_author),
            Err(ApiError::Server { status, .. }) if status == reqwest::StatusCode::NOT_FOUND => {
                Ok(false)
            }
            Err(err) => Err(err),
        }
    }

    /// Своя настройка: не давать ссылку на профиль при пересылке.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn privacy_hide_forward(&self) -> Result<bool, ApiError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Privacy {
            hide_forward_author: bool,
        }
        Ok(self
            .get_json::<Privacy>("/api/rustspace/privacy")
            .await?
            .hide_forward_author)
    }

    /// Меняет свою настройку пересылки.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn set_privacy_hide_forward(&self, hide: bool) -> Result<(), ApiError> {
        let _: serde_json::Value = self
            .put_json(
                "/api/rustspace/privacy",
                &serde_json::json!({ "hideForwardAuthor": hide }),
            )
            .await?;
        Ok(())
    }

    /// Битрейт голоса канала или переписки.
    ///
    /// # Errors
    /// Сервер без этой функции (404) или нет доступа.
    pub async fn voice_bitrate(&self, target: &str) -> Result<VoiceBitrate, ApiError> {
        self.get_json(&format!("/api/rustspace/voice-bitrate/{target}"))
            .await
    }

    /// Понизить битрейт (`Some`) или вернуть битрейт инстанса (`None`).
    ///
    /// # Errors
    /// Вне пределов или нет права.
    pub async fn set_voice_bitrate(
        &self,
        target: &str,
        kbps: Option<u32>,
    ) -> Result<VoiceBitrate, ApiError> {
        self.put_json(
            &format!("/api/rustspace/voice-bitrate/{target}"),
            &Kbps { kbps },
        )
        .await
    }

    /// Битрейт голоса инстанса.
    ///
    /// # Errors
    /// Сервер без этой функции.
    pub async fn instance_voice_bitrate(&self) -> Result<InstanceVoiceBitrate, ApiError> {
        self.get_json("/api/rustspace/admin/voice-bitrate").await
    }

    /// Задать битрейт голоса инстанса (только администратор).
    ///
    /// # Errors
    /// Вне пределов или не администратор.
    pub async fn set_instance_voice_bitrate(
        &self,
        kbps: u32,
    ) -> Result<InstanceVoiceBitrate, ApiError> {
        self.put_json(
            "/api/rustspace/admin/voice-bitrate",
            &Kbps { kbps: Some(kbps) },
        )
        .await
    }
}

#[derive(Serialize)]
struct Nsfw {
    nsfw: bool,
}

impl ApiClient {
    /// Канал с деликатным контентом (`PUT /api/rustspace/channels/:id/nsfw`).
    ///
    /// # Errors
    /// Нет права управлять каналом; сервер без этой функции (404).
    pub async fn set_channel_nsfw(&self, channel_id: &str, nsfw: bool) -> Result<(), ApiError> {
        self.put_json::<serde_json::Value, _>(
            &format!("/api/rustspace/channels/{channel_id}/nsfw"),
            &Nsfw { nsfw },
        )
        .await?;
        Ok(())
    }
}

/// Невидимая метка карточки, записанной разметкой: за ней — цвет полосы
/// символами-тегами Unicode (U+E0000 + ASCII), браузер их не показывает.
pub const CARD_MARK: char = '\u{2060}';

/// Метка карточки с цветом `#rrggbb`.
fn card_mark(color: Option<&str>) -> String {
    let hex = color
        .map(|c| c.trim_start_matches('#'))
        .filter(|c| c.len() == 6 && c.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or_default();
    std::iter::once(CARD_MARK)
        .chain(
            hex.chars()
                .filter_map(|c| char::from_u32(0xE0000 + u32::from(c))),
        )
        .collect()
}

/// Цвет и текст без метки: `Some(цвет)`, если строка — заголовок карточки,
/// записанной разметкой.
#[must_use]
pub fn split_card_mark(text: &str) -> Option<(Option<String>, String)> {
    let at = text.find(CARD_MARK)?;
    let mut hex = String::new();
    let mut rest = String::with_capacity(text.len());
    rest.push_str(&text[..at]);
    for c in text[at + CARD_MARK.len_utf8()..].chars() {
        match u32::from(c).checked_sub(0xE0000).and_then(char::from_u32) {
            Some(ascii) if ascii.is_ascii_hexdigit() && hex.len() < 6 => hex.push(ascii),
            _ => rest.push(c),
        }
    }
    Some(((hex.len() == 6).then(|| format!("#{hex}")), rest))
}

/// Невидимая метка пересланного сообщения (в начале цитаты).
pub const FORWARD_MARK: char = '\u{2063}';

/// Пересланное сообщение разметкой: цитата с подписью «Переслано от».
///
/// Автор — упоминанием (по нему открывается профиль) или, если он это
/// запретил, просто именем. Вложения — ссылками. Видно и в клиенте оригинала.
#[must_use]
pub fn forward_as_markdown(
    author_id: &str,
    author_name: &str,
    hide_author: bool,
    content: &str,
    attachments: &[String],
) -> String {
    let author = if hide_author {
        format!("**{}**", author_name.replace('*', ""))
    } else {
        format!("<@{author_id}>")
    };
    let mut lines = vec![format!("> {FORWARD_MARK}↪ {author}")];
    for line in content
        .lines()
        .chain(attachments.iter().map(String::as_str))
    {
        // Цитаты внутри цитаты разметка не поддерживает — знак цитаты снимается.
        let line = line.strip_prefix('>').map_or(line, str::trim_start);
        lines.push(if line.trim().is_empty() {
            ">".to_owned()
        } else {
            format!("> {line}")
        });
    }
    lines.join("\n")
}

/// Карточка как обычное сообщение с разметкой — для серверов без карточек
/// и для личных сообщений: заголовок (ссылкой, если есть), текст, картинка.
#[must_use]
pub fn card_as_markdown(content: Option<&str>, card: &CardDraft) -> String {
    let mut out: Vec<String> = Vec::new();
    if let Some(content) = content.map(str::trim).filter(|c| !c.is_empty()) {
        out.push(content.to_owned());
    }
    // Карточка — цитатой: так у неё остаётся полоса слева и в нашем клиенте,
    // и в веб-клиенте оригинала (цвет полосы разметкой не передать).
    let mut card_lines: Vec<String> = Vec::new();
    let mark = card_mark(card.color.as_deref());
    match (card.title.as_deref(), card.url.as_deref()) {
        (Some(title), Some(url)) => card_lines.push(format!("**[{title}]({url})**{mark}")),
        (Some(title), None) => card_lines.push(format!("**{title}**{mark}")),
        (None, Some(url)) => card_lines.push(format!("{url} {mark}")),
        (None, None) => card_lines.push(mark),
    }
    if let Some(description) = card.description.as_deref() {
        card_lines.extend(description.lines().map(str::to_owned));
    }
    if let Some(image) = &card.image {
        card_lines.push(image.url.clone());
    }
    out.extend(card_lines.into_iter().map(|line| {
        if line.trim().is_empty() {
            ">".to_owned()
        } else {
            format!("> {line}")
        }
    }));
    out.join("\n")
}

#[cfg(test)]
mod card_tests {
    use super::*;

    #[test]
    fn cards_fall_back_to_markdown() {
        let card = CardDraft {
            title: Some("Правила".into()),
            description: Some("Без ссылки".into()),
            ..CardDraft::default()
        };
        assert_eq!(
            card_as_markdown(None, &card),
            "> **Правила**\u{2060}\n> Без ссылки"
        );
        let card = CardDraft {
            title: Some("Сборка".into()),
            url: Some("https://ci.example/1".into()),
            description: Some("прошла\n\nза 3 мин".into()),
            ..CardDraft::default()
        };
        assert_eq!(
            card_as_markdown(Some("Готово"), &card),
            "Готово\n> **[Сборка](https://ci.example/1)**\u{2060}\n> прошла\n>\n> за 3 мин"
        );
    }

    #[test]
    fn forward_marks_author() {
        let text = forward_as_markdown(
            "42",
            "Ann",
            false,
            "привет\n\nмир",
            &["https://x/a.png".into()],
        );
        assert_eq!(
            text,
            "> \u{2063}↪ <@42>\n> привет\n>\n> мир\n> https://x/a.png"
        );
        let hidden = forward_as_markdown("42", "A*nn", true, "hi", &[]);
        assert_eq!(hidden, "> \u{2063}↪ **Ann**\n> hi");
        let quoted = forward_as_markdown("1", "B", false, "> **Т**\n> текст", &[]);
        assert_eq!(quoted, "> \u{2063}↪ <@1>\n> **Т**\n> текст");
    }

    #[test]
    fn card_mark_keeps_color() {
        let card = CardDraft {
            title: Some("Т".into()),
            color: Some("#e06c3a".into()),
            ..CardDraft::default()
        };
        let text = card_as_markdown(None, &card);
        let (color, rest) = split_card_mark(&text).unwrap_or_default();
        assert_eq!(color.as_deref(), Some("#e06c3a"));
        assert_eq!(rest, "> **Т**");
        assert!(split_card_mark("> обычная цитата").is_none());
    }
}
