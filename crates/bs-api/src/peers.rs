//! Федерация для администратора: пиры, запросы на одобрение, подключение.

use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::error::ApiError;

/// Пир (другой инстанс, с которым установлена федерация).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Peer {
    /// Идентификатор.
    pub id: String,
    /// Адрес инстанса.
    pub origin: String,
    /// Название, если известно.
    pub instance_name: Option<String>,
    /// `pending`, `active`, `unreachable`, `revoked`, `rejected`,
    /// `awaiting_approval`, `needs_attention`.
    pub status: String,
    /// Когда видели в последний раз (мс).
    pub last_seen_at: Option<i64>,
    /// Подряд неудач связи.
    pub consecutive_failures: u32,
    /// Подряд неудач аутентификации.
    pub consecutive_auth_failures: u32,
    /// Интервал автоматической ротации секрета, дней.
    pub auto_rotate_interval_days: u32,
    /// Идёт ли ротация.
    pub rotation_in_progress: bool,
    /// Когда добавлен (мс).
    pub created_at: i64,
    /// Причина статуса.
    pub status_reason: Option<String>,
}

/// Запрос на одобрение пиринга.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ApprovalRequest {
    /// Идентификатор.
    pub id: String,
    /// `inbound` или `outbound`.
    pub direction: String,
    /// Адрес инстанса.
    pub origin: String,
    /// Название, если известно.
    pub instance_name: Option<String>,
    /// Когда запрошено (мс).
    pub requested_at: i64,
    /// Когда истекает (мс).
    pub expires_at: i64,
}

#[derive(Deserialize)]
struct Peers {
    #[serde(default)]
    peers: Vec<Peer>,
}

#[derive(Deserialize)]
struct Requests {
    #[serde(default)]
    requests: Vec<ApprovalRequest>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Initiate<'a> {
    remote_origin: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Interval {
    auto_rotate_interval_days: u32,
}

/// Результат «перепроверить».
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Recheck {
    /// Связь восстановилась.
    pub recovered: bool,
    /// Новый статус.
    pub status: String,
}

fn segment(id: &str) -> String {
    url::form_urlencoded::byte_serialize(id.as_bytes()).collect()
}

impl ApiClient {
    /// `GET /api/federation/peers`.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn peers(&self) -> Result<Vec<Peer>, ApiError> {
        Ok(self.get_json::<Peers>("/api/federation/peers").await?.peers)
    }

    /// `POST /api/federation/peer/initiate` — начать пиринг с инстансом.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn initiate_peering(&self, origin: &str) -> Result<serde_json::Value, ApiError> {
        self.post_json(
            "/api/federation/peer/initiate",
            &Initiate {
                remote_origin: origin,
            },
        )
        .await
    }

    /// `DELETE /api/federation/peers/:id` — отозвать.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn revoke_peer(&self, id: &str) -> Result<serde_json::Value, ApiError> {
        self.delete_json(&format!("/api/federation/peers/{}", segment(id)))
            .await
    }

    /// `DELETE /api/federation/peers/:id/permanent` — удалить навсегда.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn delete_peer_permanently(&self, id: &str) -> Result<serde_json::Value, ApiError> {
        self.delete_json(&format!("/api/federation/peers/{}/permanent", segment(id)))
            .await
    }

    /// `POST /api/federation/peers/:id/reset` — сбросить пира.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn reset_peer(&self, id: &str) -> Result<serde_json::Value, ApiError> {
        self.post_json(
            &format!("/api/federation/peers/{}/reset", segment(id)),
            &serde_json::json!({}),
        )
        .await
    }

    /// `POST /api/federation/peers/:id/recheck` — перепроверить связь.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn recheck_peer(&self, id: &str) -> Result<Recheck, ApiError> {
        self.post_json(
            &format!("/api/federation/peers/{}/recheck", segment(id)),
            &serde_json::json!({}),
        )
        .await
    }

    /// `POST /api/federation/peers/:id/rotate` — сменить общий секрет.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn rotate_peer_secret(&self, id: &str) -> Result<serde_json::Value, ApiError> {
        self.post_json(
            &format!("/api/federation/peers/{}/rotate", segment(id)),
            &serde_json::json!({}),
        )
        .await
    }

    /// `PATCH /api/federation/peers/:id` — интервал автоматической ротации.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn set_peer_interval(
        &self,
        id: &str,
        days: u32,
    ) -> Result<serde_json::Value, ApiError> {
        self.patch_json(
            &format!("/api/federation/peers/{}", segment(id)),
            &Interval {
                auto_rotate_interval_days: days,
            },
        )
        .await
    }

    /// `GET /api/federation/approval-requests`.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn approval_requests(&self) -> Result<Vec<ApprovalRequest>, ApiError> {
        Ok(self
            .get_json::<Requests>("/api/federation/approval-requests")
            .await?
            .requests)
    }

    /// `POST /api/federation/approval-requests/:id/approve` или `/deny`.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn answer_approval(
        &self,
        id: &str,
        approve: bool,
    ) -> Result<serde_json::Value, ApiError> {
        let action = if approve { "approve" } else { "deny" };
        self.post_json(
            &format!("/api/federation/approval-requests/{}/{action}", segment(id)),
            &serde_json::json!({}),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::{Peer, Peers};

    #[test]
    fn peers_tolerate_missing_fields() {
        let parsed: Peers = serde_json::from_str(
            r#"{"peers":[{"id":"p1","origin":"https://a.example","status":"active",
                "lastSeenAt":null,"autoRotateIntervalDays":30,"extra":1}]}"#,
        )
        .unwrap_or(Peers { peers: Vec::new() });
        let peer: &Peer = &parsed.peers[0];
        assert_eq!(peer.status, "active");
        assert_eq!(peer.auto_rotate_interval_days, 30);
        assert_eq!(peer.consecutive_failures, 0);
    }
}
