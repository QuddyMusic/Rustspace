//! Инвайт-ссылки администратора: `/api/admin/invites`.

use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::error::ApiError;

/// Инвайт-ссылка.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Invite {
    /// Идентификатор.
    pub id: String,
    /// Название.
    pub name: String,
    /// `active`, `expired`, `exhausted`, `revoked`.
    pub status: String,
    /// Предел использований; `None` — без предела.
    pub max_uses: Option<u32>,
    /// Сколько раз использована.
    pub used_count: u32,
    /// Срок действия (мс); `None` — бессрочно.
    pub expires_at: Option<i64>,
    /// Когда отозвана (мс).
    pub revoked_at: Option<i64>,
    /// Имя создателя.
    pub created_by_username: Option<String>,
    /// Последнее использование (мс).
    pub last_redeemed_at: Option<i64>,
    /// Готовая ссылка (собирает сервер).
    pub url: String,
}

/// Кто воспользовался ссылкой.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Redemption {
    /// Имя при регистрации.
    pub registrant_username: String,
    /// Текущее имя.
    pub current_username: Option<String>,
    /// Аккаунт удалён.
    pub is_deleted: bool,
    /// Когда (мс).
    pub redeemed_at: i64,
}

/// Новая ссылка.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewInvite {
    /// Название.
    pub name: String,
    /// Предел использований.
    pub max_uses: Option<u32>,
    /// Срок действия (мс).
    pub expires_at: Option<i64>,
}

/// Правка ссылки: `Some(None)` сбрасывает значение.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvitePatch {
    /// Название.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Предел использований.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<Option<u32>>,
    /// Срок действия.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Option<i64>>,
}

#[derive(Deserialize)]
struct Invites {
    #[serde(default)]
    invites: Vec<Invite>,
}

#[derive(Deserialize)]
struct Redemptions {
    #[serde(default)]
    redemptions: Vec<Redemption>,
}

fn segment(id: &str) -> String {
    url::form_urlencoded::byte_serialize(id.as_bytes()).collect()
}

impl ApiClient {
    /// `GET /api/admin/invites?status=active|archived`.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn invites(&self, archived: bool) -> Result<Vec<Invite>, ApiError> {
        let status = if archived { "archived" } else { "active" };
        Ok(self
            .get_json::<Invites>(&format!("/api/admin/invites?status={status}"))
            .await?
            .invites)
    }

    /// `POST /api/admin/invites`.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn create_invite(&self, invite: &NewInvite) -> Result<Invite, ApiError> {
        self.post_json("/api/admin/invites", invite).await
    }

    /// `PATCH /api/admin/invites/:id`.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn update_invite(&self, id: &str, patch: &InvitePatch) -> Result<Invite, ApiError> {
        self.patch_json(&format!("/api/admin/invites/{}", segment(id)), patch)
            .await
    }

    /// `POST /api/admin/invites/:id/revoke`.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn revoke_invite(&self, id: &str) -> Result<serde_json::Value, ApiError> {
        self.post_json(
            &format!("/api/admin/invites/{}/revoke", segment(id)),
            &serde_json::json!({}),
        )
        .await
    }

    /// `POST /api/admin/invites/:id/reinstate`.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn reinstate_invite(
        &self,
        id: &str,
        patch: &InvitePatch,
    ) -> Result<serde_json::Value, ApiError> {
        self.post_json(
            &format!("/api/admin/invites/{}/reinstate", segment(id)),
            patch,
        )
        .await
    }

    /// `DELETE /api/admin/invites/:id`.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn delete_invite(&self, id: &str) -> Result<serde_json::Value, ApiError> {
        self.delete_json(&format!("/api/admin/invites/{}", segment(id)))
            .await
    }

    /// `GET /api/admin/invites/:id/redemptions`.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn invite_redemptions(&self, id: &str) -> Result<Vec<Redemption>, ApiError> {
        Ok(self
            .get_json::<Redemptions>(&format!("/api/admin/invites/{}/redemptions", segment(id)))
            .await?
            .redemptions)
    }
}

#[cfg(test)]
mod tests {
    use super::{InvitePatch, NewInvite};

    #[test]
    fn patch_distinguishes_clear_from_untouched() {
        let patch = InvitePatch {
            max_uses: Some(None),
            ..InvitePatch::default()
        };
        assert_eq!(
            serde_json::to_string(&patch).unwrap_or_default(),
            r#"{"maxUses":null}"#
        );
        assert_eq!(
            serde_json::to_string(&InvitePatch::default()).unwrap_or_default(),
            "{}"
        );
    }

    #[test]
    fn new_invite_sends_nulls() {
        let body = NewInvite {
            name: "друзья".to_owned(),
            max_uses: None,
            expires_at: Some(5),
        };
        assert_eq!(
            serde_json::to_string(&body).unwrap_or_default(),
            r#"{"name":"друзья","maxUses":null,"expiresAt":5}"#
        );
    }
}
