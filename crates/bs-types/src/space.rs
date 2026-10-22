//! Пространства (Space), участники, роли, приглашения, раскладка сайдбара.
//!
//! Источник истины — `backspace/packages/shared/src/types.ts`.
//! Время — epoch-миллисекунды (`i64`), как отдаёт сервер.

use serde::{Deserialize, Serialize};

use crate::channel::{Channel, ChannelCategory};
use crate::id::{InviteCode, RoleId, SpaceId, UserId};
use crate::user::{AvatarColor, User};

/// Видимость пространства.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceVisibility {
    /// Публичное — любой может войти.
    Public,
    /// Вступление по запросу.
    Request,
    /// Приватное — только по приглашению.
    Private,
}

/// Пространство.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Space {
    pub id: SpaceId,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub avatar_color: Option<AvatarColor>,
    pub owner_id: UserId,
    #[serde(default)]
    pub owner_title: Option<String>,
    #[serde(default)]
    pub invite_code: Option<InviteCode>,
    pub visibility: SpaceVisibility,
    pub directory_listed: bool,
    #[serde(default)]
    pub description: Option<String>,
    pub created_at: i64,
}

/// Пространство с каналами, категориями, участниками и ролями.
///
/// В TS это `interface SpaceWithChannelsAndMembers extends Space`, поэтому
/// поля `Space` сериализуются «плоско» через `#[serde(flatten)]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceWithChannelsAndMembers {
    #[serde(flatten)]
    pub space: Space,
    #[serde(default)]
    pub channels: Vec<Channel>,
    #[serde(default)]
    pub categories: Vec<ChannelCategory>,
    #[serde(default)]
    pub members: Vec<MemberWithUser>,
    #[serde(default)]
    pub roles: Vec<Role>,
    /// Эффективные права текущего пользователя (decimal string).
    #[serde(default)]
    pub my_permissions: Option<String>,
}

/// Участник пространства.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub space_id: SpaceId,
    pub user_id: UserId,
    #[serde(default)]
    pub nickname: Option<String>,
    pub joined_at: i64,
}

/// Участник с вложенным пользователем и его ролями.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberWithUser {
    #[serde(flatten)]
    pub member: Member,
    pub user: User,
    #[serde(default)]
    pub roles: Vec<Role>,
}

/// Роль в пространстве.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Role {
    pub id: RoleId,
    pub space_id: SpaceId,
    pub name: String,
    /// Цвет в hex-строке (не палитра `AvatarColor`).
    pub color: String,
    pub position: i64,
    /// Права как decimal string; `None`, если не запрашивались.
    #[serde(default)]
    pub permissions: Option<String>,
    /// `true` для роли `@everyone`.
    #[serde(default)]
    pub is_everyone: Option<bool>,
    pub created_at: i64,
}

/// Папка пространств в сайдбаре пользователя (личная, не серверная).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceFolder {
    pub id: String,
    pub user_id: UserId,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    pub position: i64,
    #[serde(default)]
    pub space_ids: Vec<SpaceId>,
}

/// Элемент пользовательской раскладки сайдбара.
///
/// В TS это union `{ t: 's'; id } | { t: 'f'; id }` — тегированное объединение
/// по полю `t`, а не enum с отдельным полем `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum SpaceLayoutItem {
    /// Пространство (`'s'`).
    #[serde(rename = "s")]
    Space { id: SpaceId },
    /// Папка (`'f'`).
    #[serde(rename = "f")]
    Folder { id: String },
}

/// Статус запроса на вступление.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinRequestStatus {
    Pending,
    Accepted,
    Declined,
}

/// Запрос на вступление в пространство с `visibility = "request"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequest {
    pub id: String,
    pub space_id: SpaceId,
    pub user_id: UserId,
    #[serde(default)]
    pub message: Option<String>,
    pub status: JoinRequestStatus,
    #[serde(default)]
    pub decided_by: Option<UserId>,
    pub created_at: i64,
    #[serde(default)]
    pub decided_at: Option<i64>,
    #[serde(default)]
    pub user: Option<User>,
}

/// Превью приглашения (плоская структура, не вложенный `Space`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvitePreview {
    pub space_id: SpaceId,
    pub space_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub avatar_color: Option<AvatarColor>,
    pub member_count: u64,
    pub instance_name: String,
    #[serde(default)]
    pub visibility: Option<SpaceVisibility>,
}

/// Пространство в директории (Explore) — плоская структура.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreSpace {
    pub id: SpaceId,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub avatar_color: Option<AvatarColor>,
    #[serde(default)]
    pub description: Option<String>,
    pub visibility: SpaceVisibility,
    pub member_count: u64,
    pub created_at: i64,
    #[serde(default)]
    pub joined: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_space_payload() {
        let json = r#"{
            "id": "123456789012345678",
            "name": "Rustaceans",
            "icon": "https://cdn.example/i.png",
            "banner": null,
            "avatarColor": "lavender",
            "ownerId": "42",
            "ownerTitle": null,
            "inviteCode": "abc123",
            "visibility": "request",
            "directoryListed": true,
            "description": "Про Rust",
            "createdAt": 1755000000000
        }"#;
        let space: Space = serde_json::from_str(json).unwrap();
        assert_eq!(space.id, "123456789012345678");
        assert_eq!(space.avatar_color.as_deref(), Some("lavender"));
        assert_eq!(space.visibility, SpaceVisibility::Request);
        assert!(space.directory_listed);
        assert_eq!(space.created_at, 1_755_000_000_000);
        assert!(space.banner.is_none());
    }

    #[test]
    fn space_with_channels_flattens_space_fields() {
        let json = r#"{
            "id": "s1",
            "name": "Space",
            "ownerId": "u1",
            "visibility": "public",
            "directoryListed": false,
            "createdAt": 1,
            "channels": [],
            "categories": [],
            "members": [],
            "roles": [],
            "myPermissions": "32"
        }"#;
        let swc: SpaceWithChannelsAndMembers = serde_json::from_str(json).unwrap();
        assert_eq!(swc.space.name, "Space");
        assert_eq!(swc.my_permissions.as_deref(), Some("32"));
        assert!(swc.channels.is_empty());
    }

    #[test]
    fn member_with_user_parses_flattened() {
        let json = r##"{
            "spaceId": "s1",
            "userId": "u1",
            "nickname": null,
            "joinedAt": 1700000000000,
            "user": {
                "id": "u1",
                "username": "alice",
                "displayName": "Alice",
                "avatar": null,
                "banner": null,
                "accentColor": null,
                "avatarColor": "mint",
                "bio": null,
                "status": "online",
                "customStatus": null,
                "isAdmin": false,
                "createdAt": 1690000000000,
                "homeInstance": null,
                "homeUserId": null,
                "replicatedInstances": []
            },
            "roles": [{
                "id": "r1", "spaceId": "s1", "name": "@everyone",
                "color": "#99aab5", "position": 0, "isEveryone": true,
                "createdAt": 1
            }]
        }"##;
        let m: MemberWithUser = serde_json::from_str(json).unwrap();
        assert_eq!(m.member.joined_at, 1_700_000_000_000);
        assert_eq!(m.user.username, "alice");
        assert_eq!(m.roles.len(), 1);
        assert!(m.roles[0].permissions.is_none());
    }

    #[test]
    fn layout_item_is_tagged_union() {
        let s: SpaceLayoutItem = serde_json::from_str(r#"{"t":"s","id":"sp1"}"#).unwrap();
        assert_eq!(s, SpaceLayoutItem::Space { id: "sp1".into() });
        let f: SpaceLayoutItem = serde_json::from_str(r#"{"t":"f","id":"fold1"}"#).unwrap();
        assert_eq!(f, SpaceLayoutItem::Folder { id: "fold1".into() });
    }

    #[test]
    fn folder_has_color_and_user() {
        let json = r##"{
            "id": "f1", "userId": "u1", "name": null,
            "color": "#ff0000", "position": 2, "spaceIds": ["s1","s2"]
        }"##;
        let folder: SpaceFolder = serde_json::from_str(json).unwrap();
        assert!(folder.name.is_none());
        assert_eq!(folder.color.as_deref(), Some("#ff0000"));
        assert_eq!(folder.space_ids, vec!["s1".to_string(), "s2".to_string()]);
    }

    #[test]
    fn join_request_status_and_decision() {
        let json = r#"{
            "id": "jr1", "spaceId": "s1", "userId": "u1", "message": null,
            "status": "declined", "decidedBy": "admin1",
            "createdAt": 10, "decidedAt": 20
        }"#;
        let jr: JoinRequest = serde_json::from_str(json).unwrap();
        assert_eq!(jr.status, JoinRequestStatus::Declined);
        assert_eq!(jr.decided_by.as_deref(), Some("admin1"));
        assert_eq!(jr.decided_at, Some(20));
    }

    #[test]
    fn explore_space_is_flat() {
        let json = r#"{
            "id": "s1", "name": "Open", "icon": null, "banner": null,
            "avatarColor": null, "description": null, "visibility": "public",
            "memberCount": 7, "createdAt": 1, "joined": false
        }"#;
        let es: ExploreSpace = serde_json::from_str(json).unwrap();
        assert_eq!(es.member_count, 7);
        assert_eq!(es.joined, Some(false));
    }
}
