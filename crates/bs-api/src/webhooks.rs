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
    match (card.title.as_deref(), card.url.as_deref()) {
        (Some(title), Some(url)) => card_lines.push(format!("**[{title}]({url})**")),
        (Some(title), None) => card_lines.push(format!("**{title}**")),
        (None, Some(url)) => card_lines.push(url.to_owned()),
        (None, None) => {}
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
        assert_eq!(card_as_markdown(None, &card), "> **Правила**\n> Без ссылки");
        let card = CardDraft {
            title: Some("Сборка".into()),
            url: Some("https://ci.example/1".into()),
            description: Some("прошла\n\nза 3 мин".into()),
            ..CardDraft::default()
        };
        assert_eq!(
            card_as_markdown(Some("Готово"), &card),
            "Готово\n> **[Сборка](https://ci.example/1)**\n> прошла\n>\n> за 3 мин"
        );
    }
}
