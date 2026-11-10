//! Чистая логика социального экрана GUI: друзья, заявки, поиск, личные каналы.
//!
//! Модуль ничего не знает про `egui` и сеть: только раскладка данных из
//! [`bs_core::Cache`] и результатов REST в строки и предикаты. Так его легко
//! покрыть тестами, а экраны `home` и `chat` остаются тонкими обёртками.

use bs_types::{DmChannel, DmChannelLastMessage, Friend, FriendRequest, User, UserStatus};

/// Подпись статуса присутствия.
pub(crate) const fn status_label(status: UserStatus) -> &'static str {
    match status {
        UserStatus::Online => "В сети",
        UserStatus::Idle => "Нет на месте",
        UserStatus::DoNotDisturb => "Не беспокоить",
        UserStatus::Offline => "Не в сети",
    }
}

/// Время последнего сообщения личного канала (мс), если оно есть.
pub(crate) fn dm_last_at(channel: &DmChannel) -> Option<i64> {
    match channel.last_message.as_ref()? {
        DmChannelLastMessage::Preview(preview) => Some(preview.created_at),
        DmChannelLastMessage::Full(message) => Some(message.message.created_at),
    }
}

/// Имя пользователя для показа: `display_name`, иначе `username`.
///
/// Пустая строка (после `trim`) считается отсутствием display name.
pub(crate) fn user_display_name(display_name: Option<&str>, username: &str) -> String {
    display_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(username)
        .to_owned()
}

/// Имя друга для показа.
pub(crate) fn friend_display_name(friend: &Friend) -> String {
    user_display_name(friend.display_name.as_deref(), &friend.username)
}

/// Однострочный статус друга: свой статус или присутствие.
pub(crate) fn friend_status_text(friend: &Friend) -> String {
    friend
        .custom_status
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map_or_else(|| status_label(friend.status).to_owned(), str::to_owned)
}

/// Имя собеседника в заявке (display name, иначе id).
pub(crate) fn request_name(request: &FriendRequest) -> String {
    request.user.as_ref().map_or_else(
        || request.id.clone(),
        |user| user_display_name(user.display_name.as_deref(), &user.username),
    )
}

/// Входящая ли заявка для пользователя `me`.
pub(crate) fn request_is_incoming(request: &FriendRequest, me: &str) -> bool {
    request.to_id == me
}

/// Имя личного канала: явное название группы или имена собеседников.
pub(crate) fn dm_display_name(channel: &DmChannel, me: &str) -> String {
    if let Some(name) = channel
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        return name.to_owned();
    }
    let others: Vec<String> = channel
        .members
        .iter()
        .filter(|member| member.id != me)
        .map(|member| user_display_name(member.display_name.as_deref(), &member.username))
        .collect();
    if others.is_empty() {
        channel.id.clone()
    } else {
        others.join(", ")
    }
}

/// Последнее сообщение личного канала, если есть.
///
/// Системное сообщение показывается текстом события, а не сырым JSON
/// (`formatDmSidebarPreview` оригинала).
pub(crate) fn dm_last_text(channel: &DmChannel) -> Option<String> {
    let (content, kind, author) = match channel.last_message.as_ref()? {
        DmChannelLastMessage::Preview(preview) => (
            preview.content.clone(),
            preview.message_type,
            preview.user_id.as_str(),
        ),
        DmChannelLastMessage::Full(message) => (
            message.message.content.clone(),
            message.message.message_type,
            message.message.user_id.as_str(),
        ),
    };
    if kind != Some(bs_types::MessageType::System) {
        return content;
    }
    let actor = channel
        .members
        .iter()
        .find(|member| member.id == author)
        .map_or_else(String::new, |member| {
            user_display_name(member.display_name.as_deref(), &member.username)
        });
    Some(
        content
            .as_deref()
            .and_then(bs_types::DmSystemEvent::parse)
            .map_or_else(
                || bs_types::GENERIC_SYSTEM_TEXT.to_owned(),
                |event| event.text(&actor, false),
            ),
    )
}

/// Уже ли пользователь `id` в друзьях.
pub(crate) fn is_friend(friends: &[Friend], id: &str) -> bool {
    friends.iter().any(|friend| friend.id == id)
}

/// Имя пользователя из результатов поиска.
pub(crate) fn search_user_name(user: &User) -> String {
    user_display_name(user.display_name.as_deref(), &user.username)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::needless_pass_by_value)]
mod tests {
    use super::{
        dm_display_name, dm_last_text, is_friend, request_is_incoming, request_name,
        search_user_name, user_display_name,
    };
    use bs_types::{DmChannel, FriendRequest, User};

    /// Минимальный пользователь; `display` — значение `displayName`.
    fn user(id: &str, username: &str, display: Option<&str>) -> User {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "username": username,
            "displayName": display,
            "avatar": null,
            "banner": null,
            "accentColor": null,
            "avatarColor": null,
            "bio": null,
            "status": "online",
            "customStatus": null,
            "isAdmin": false,
            "createdAt": 0,
            "homeInstance": null,
            "homeUserId": null,
            "replicatedInstances": []
        }))
        .expect("пользователь должен разбираться")
    }

    fn dm(
        id: &str,
        name: Option<&str>,
        members: Vec<User>,
        last: Option<serde_json::Value>,
    ) -> DmChannel {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "federatedId": null,
            "ownerId": null,
            "ownerHomeUserId": null,
            "ownerHomeInstance": null,
            "createdAt": 0,
            "members": members,
            "lastMessage": last,
            "name": name,
            "icon": null,
            "metadataUpdatedAt": 0
        }))
        .expect("личный канал должен разбираться")
    }

    #[test]
    fn display_name_prefers_non_empty_display() {
        assert_eq!(user_display_name(Some("Alice"), "alice42"), "Alice");
        assert_eq!(user_display_name(None, "alice42"), "alice42");
        assert_eq!(user_display_name(Some("   "), "alice42"), "alice42");
    }

    fn request(to_id: &str, user: Option<User>) -> FriendRequest {
        serde_json::from_value(serde_json::json!({
            "id": "r1",
            "fromId": "u2",
            "toId": to_id,
            "status": "pending",
            "createdAt": 0,
            "user": user
        }))
        .expect("заявка должна разбираться")
    }

    #[test]
    fn request_incoming_follows_to_id() {
        let req = request("me", None);
        assert!(request_is_incoming(&req, "me"));
        assert!(!request_is_incoming(&req, "u2"));
    }

    #[test]
    fn request_name_falls_back_to_id() {
        assert_eq!(request_name(&request("me", None)), "r1");
    }

    #[test]
    fn dm_name_uses_group_name() {
        let channel = dm("d1", Some("Команда"), vec![user("u2", "bob", None)], None);
        assert_eq!(dm_display_name(&channel, "me"), "Команда");
    }

    #[test]
    fn dm_name_joins_other_members() {
        let channel = dm(
            "d1",
            None,
            vec![user("me", "me", None), user("u2", "bob", Some("Bob"))],
            None,
        );
        assert_eq!(dm_display_name(&channel, "me"), "Bob");
    }

    #[test]
    fn dm_name_falls_back_to_id() {
        let channel = dm("d1", None, vec![user("me", "me", None)], None);
        assert_eq!(dm_display_name(&channel, "me"), "d1");
    }

    #[test]
    fn dm_last_text_from_preview() {
        let channel = dm(
            "d1",
            None,
            vec![],
            Some(serde_json::json!({
                "id": "m1",
                "dmChannelId": "d1",
                "userId": "u2",
                "content": "привет",
                "createdAt": 0
            })),
        );
        assert_eq!(dm_last_text(&channel).as_deref(), Some("привет"));
    }

    #[test]
    fn dm_last_text_from_full() {
        let channel = dm(
            "d1",
            None,
            vec![],
            Some(serde_json::json!({
                "id": "m1",
                "dmChannelId": "d1",
                "userId": "u2",
                "content": "текст",
                "createdAt": 0,
                "user": user("u2", "bob", None),
                "attachments": [],
                "embeds": [],
                "reactions": []
            })),
        );
        assert_eq!(dm_last_text(&channel).as_deref(), Some("текст"));
    }

    #[test]
    fn search_user_name_works() {
        let user = user("u2", "bob", Some("Bob"));
        assert_eq!(search_user_name(&user), "Bob");
    }

    #[test]
    fn is_friend_checks_id() {
        assert!(!is_friend(&[], "u2"));
    }
}
