//! Социальные типы REST-слоя: поиск пользователей и discover.
//!
//! `Friend` и `FriendRequest` живут в [`crate::events`], потому что приходят
//! и событиями WebSocket. Здесь — только ответы REST-эндпоинтов
//! `/api/social/*`, которых в потоке событий нет.

use serde::{Deserialize, Serialize};

use crate::id::UserId;
use crate::user::{AvatarColor, UserStatus};

/// Отношение текущего пользователя к найденному.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverRelationship {
    /// Ни дружбы, ни заявок.
    None,
    /// Уже друзья.
    Friends,
    /// Заявка отправлена текущим пользователем и ждёт ответа.
    OutboundPending,
    /// Заявка пришла от этого пользователя и ждёт ответа.
    InboundPending,
}

/// Пользователь в выдаче `GET /api/social/discover`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverUser {
    pub id: UserId,
    pub username: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub avatar_color: Option<AvatarColor>,
    #[serde(default)]
    pub bio: Option<String>,
    pub status: UserStatus,
    #[serde(default)]
    pub custom_status: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub home_instance: Option<String>,
    #[serde(default)]
    pub home_user_id: Option<String>,
    /// Сколько общих друзей.
    pub mutual_friend_count: i64,
    /// Сколько общих пространств.
    pub mutual_space_count: i64,
    /// Текущее отношение.
    pub relationship: DiscoverRelationship,
    /// id заявки, если `relationship` — `outbound_pending` или `inbound_pending`.
    #[serde(default)]
    pub request_id: Option<String>,
}

/// Ответ `GET /api/social/discover`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverResponse {
    pub users: Vec<DiscoverUser>,
    pub total: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_user_round_trips() {
        let json = serde_json::json!({
            "id": "u2",
            "username": "bob",
            "displayName": null,
            "status": "offline",
            "createdAt": 1,
            "mutualFriendCount": 3,
            "mutualSpaceCount": 1,
            "relationship": "outbound_pending",
            "requestId": "r1"
        });
        let user: DiscoverUser = serde_json::from_value(json).unwrap();
        assert_eq!(user.id, "u2");
        assert_eq!(user.relationship, DiscoverRelationship::OutboundPending);
        assert_eq!(user.request_id.as_deref(), Some("r1"));
        assert_eq!(user.mutual_friend_count, 3);
    }

    #[test]
    fn discover_response_round_trips() {
        let json = serde_json::json!({ "users": [], "total": 0 });
        let page: DiscoverResponse = serde_json::from_value(json).unwrap();
        assert!(page.users.is_empty());
        assert_eq!(page.total, 0);
    }

    #[test]
    fn relationship_accepts_all_variants() {
        for (text, expected) in [
            ("\"none\"", DiscoverRelationship::None),
            ("\"friends\"", DiscoverRelationship::Friends),
            ("\"inbound_pending\"", DiscoverRelationship::InboundPending),
        ] {
            let parsed: DiscoverRelationship = serde_json::from_str(text).unwrap();
            assert_eq!(parsed, expected);
        }
    }
}
