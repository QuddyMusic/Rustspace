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
    /// Канал «18+» (`PUT /api/rustspace/channels/:id/nsfw`).
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
