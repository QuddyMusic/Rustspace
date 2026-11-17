//! Мосты Rustspace: связи каналов с чатами Telegram, Discord и Matrix
//! (`/api/rustspace/...`; у оригинала их нет).

use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::error::ApiError;

/// Связь канала с внешним чатом.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeLink {
    /// Идентификатор связи.
    pub id: String,
    /// Сеть: `telegram`, `discord`, `matrix`.
    pub kind: String,
    /// Чат на той стороне.
    pub external_id: String,
    /// Когда связали, мс.
    pub created_at: i64,
}

/// Мосты канала: какие сети настроены на сервере и связи канала.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ChannelBridges {
    /// Сети с токенами ботов на сервере.
    pub available: Vec<String>,
    /// Связи канала.
    pub links: Vec<BridgeLink>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NewLink<'a> {
    kind: &'a str,
    external_id: &'a str,
}

impl ApiClient {
    /// `GET /api/rustspace/channels/:id/bridges`.
    ///
    /// # Errors
    /// Нет права управлять каналами; сервер без мостов (404).
    pub async fn channel_bridges(&self, channel_id: &str) -> Result<ChannelBridges, ApiError> {
        self.get_json(&format!("/api/rustspace/channels/{channel_id}/bridges"))
            .await
    }

    /// `POST /api/rustspace/channels/:id/bridges` — связать канал с чатом.
    ///
    /// # Errors
    /// Сеть не настроена, неверный идентификатор чата, связь уже есть.
    pub async fn add_channel_bridge(
        &self,
        channel_id: &str,
        kind: &str,
        external_id: &str,
    ) -> Result<(), ApiError> {
        self.post_json::<serde_json::Value, _>(
            &format!("/api/rustspace/channels/{channel_id}/bridges"),
            &NewLink { kind, external_id },
        )
        .await?;
        Ok(())
    }

    /// `DELETE /api/rustspace/bridges/:id` — развязать.
    ///
    /// # Errors
    /// Связи нет или нет права.
    pub async fn remove_bridge(&self, link_id: &str) -> Result<(), ApiError> {
        self.delete_json::<serde_json::Value>(&format!("/api/rustspace/bridges/{link_id}"))
            .await?;
        Ok(())
    }
}
