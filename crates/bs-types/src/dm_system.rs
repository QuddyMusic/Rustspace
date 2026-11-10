//! Системные сообщения личных бесед (`type: "system"`).
//!
//! Содержимое такого сообщения — JSON-событие, как в
//! `shared/src/dmSystemEvents.ts` оригинала: участник добавлен или удалён,
//! сменился владелец, название или значок, приглашение в пространство.
//! Неизвестное или повреждённое событие — `None`; интерфейс показывает
//! нейтральное «Системное сообщение», а не сырой JSON.

use serde::{Deserialize, Serialize};

/// Как участник покинул группу.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemovalReason {
    /// Вышел сам.
    Leave,
    /// Удалил владелец.
    Kick,
}

/// Снимок пространства в приглашении (на момент отправки).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceInviteSnapshot {
    /// Название пространства.
    pub space_name: String,
    /// Иконка (имя файла или URL).
    #[serde(default)]
    pub icon: Option<String>,
    /// Цвет заглушки.
    #[serde(default)]
    pub avatar_color: Option<String>,
    /// Участников на момент отправки.
    #[serde(default)]
    pub member_count: u64,
    /// Описание.
    #[serde(default)]
    pub description: Option<String>,
    /// Название инстанса пространства.
    #[serde(default)]
    pub instance_name: String,
}

/// Приглашение в пространство, присланное в ЛС.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceInvite {
    /// Пространство на своём инстансе.
    pub space_id: String,
    /// Инстанс пространства (`https://…`).
    pub space_instance_origin: String,
    /// Код приглашения.
    pub invite_code: String,
    /// Что показать на карточке.
    pub snapshot: SpaceInviteSnapshot,
}

/// Системное событие беседы.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DmSystemEvent {
    /// Участника добавили.
    #[serde(rename_all = "camelCase")]
    MemberAdded {
        /// Кого.
        target_user_id: String,
        /// Его имя на момент события.
        target_display_name: String,
    },
    /// Участник ушёл или его удалили.
    #[serde(rename_all = "camelCase")]
    MemberRemoved {
        /// Кто.
        target_user_id: String,
        /// Его имя.
        target_display_name: String,
        /// Как.
        reason: RemovalReason,
    },
    /// Новый владелец.
    #[serde(rename_all = "camelCase")]
    OwnerChanged {
        /// Кто.
        new_owner_id: String,
        /// Его имя.
        new_owner_display_name: String,
    },
    /// Сменилось название (`None` — сброшено).
    #[serde(rename_all = "camelCase")]
    NameChanged {
        /// Было.
        #[serde(default)]
        old_name: Option<String>,
        /// Стало.
        #[serde(default)]
        new_name: Option<String>,
    },
    /// Сменился значок.
    IconChanged,
    /// Приглашение в пространство.
    SpaceInvite(SpaceInvite),
}

impl DmSystemEvent {
    /// Разбирает содержимое системного сообщения.
    #[must_use]
    pub fn parse(content: &str) -> Option<Self> {
        let event: Self = serde_json::from_str(content).ok()?;
        let valid = match &event {
            Self::MemberAdded {
                target_user_id,
                target_display_name,
            }
            | Self::MemberRemoved {
                target_user_id,
                target_display_name,
                ..
            } => !target_user_id.is_empty() && !target_display_name.is_empty(),
            Self::OwnerChanged {
                new_owner_id,
                new_owner_display_name,
            } => !new_owner_id.is_empty() && !new_owner_display_name.is_empty(),
            Self::SpaceInvite(invite) => {
                !invite.space_id.is_empty()
                    && !invite.invite_code.is_empty()
                    && !invite.snapshot.space_name.is_empty()
                    && is_http_origin(&invite.space_instance_origin)
            }
            Self::NameChanged { .. } | Self::IconChanged => true,
        };
        valid.then_some(event)
    }

    /// Значок строки ленты (`dmSystemIcon` оригинала).
    #[must_use]
    pub const fn glyph(&self) -> Option<&'static str> {
        match self {
            Self::MemberAdded { .. } => Some("→"),
            Self::MemberRemoved { .. } => Some("←"),
            Self::OwnerChanged { .. } => Some("♛"),
            Self::NameChanged { .. } => Some("✎"),
            Self::IconChanged => Some("🖼"),
            Self::SpaceInvite(_) => None,
        }
    }

    /// Текст события по-русски (`dm:system.*` оригинала); `actor` — автор.
    #[must_use]
    pub fn text(&self, actor: &str, timeline: bool) -> String {
        match self {
            Self::MemberAdded {
                target_display_name,
                ..
            } => format!("{actor} добавляет в группу: {target_display_name}"),
            Self::MemberRemoved {
                target_display_name,
                reason: RemovalReason::Leave,
                ..
            } => format!("{target_display_name} выходит из группы"),
            Self::MemberRemoved {
                target_display_name,
                ..
            } => format!("{actor} удаляет из группы: {target_display_name}"),
            Self::OwnerChanged {
                new_owner_display_name,
                ..
            } => format!("{new_owner_display_name} теперь владелец группы"),
            Self::NameChanged { new_name, .. } => {
                match new_name.as_deref().filter(|n| !n.is_empty()) {
                    None => format!("{actor} удаляет название группы"),
                    Some(name) if timeline => format!("{actor} меняет название группы на «{name}»"),
                    Some(_) => format!("{actor} меняет название группы"),
                }
            }
            Self::IconChanged => format!("{actor} меняет значок группы"),
            Self::SpaceInvite(invite) => format!("📨 Приглашение в {}", invite.snapshot.space_name),
        }
    }
}

/// Текст для неизвестного события.
pub const GENERIC_SYSTEM_TEXT: &str = "Системное сообщение";

/// `isHttpOrigin` оригинала: схема http(s) и только хост[:порт].
fn is_http_origin(value: &str) -> bool {
    let rest = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"));
    let Some(authority) = rest else {
        return false;
    };
    !authority.is_empty()
        && !authority.ends_with(':')
        && !authority.contains(['/', '?', '#', '@', '\\', '%'])
        && authority
            .chars()
            .all(|c| !c.is_control() && !c.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::{DmSystemEvent, RemovalReason};

    #[test]
    fn membership_events_parse_and_read_like_the_original() {
        let added = DmSystemEvent::parse(
            r#"{"event":"member_added","targetUserId":"2","targetDisplayName":"Maria"}"#,
        )
        .unwrap();
        assert_eq!(added.text("Тео", true), "Тео добавляет в группу: Maria");
        assert_eq!(added.glyph(), Some("→"));
        let left = DmSystemEvent::parse(
            r#"{"event":"member_removed","targetUserId":"2","targetDisplayName":"Oleg","reason":"leave"}"#,
        )
        .unwrap();
        assert!(matches!(
            left,
            DmSystemEvent::MemberRemoved {
                reason: RemovalReason::Leave,
                ..
            }
        ));
        assert_eq!(left.text("x", true), "Oleg выходит из группы");
        let renamed =
            DmSystemEvent::parse(r#"{"event":"name_changed","oldName":null,"newName":"Друзья"}"#)
                .unwrap();
        assert_eq!(
            renamed.text("А", true),
            "А меняет название группы на «Друзья»"
        );
        assert_eq!(renamed.text("А", false), "А меняет название группы");
        assert!(DmSystemEvent::parse(r#"{"event":"icon_changed"}"#).is_some());
    }

    #[test]
    fn broken_or_unknown_events_are_none() {
        assert!(DmSystemEvent::parse("not json").is_none());
        assert!(DmSystemEvent::parse(r#"{"event":"party"}"#).is_none());
        assert!(
            DmSystemEvent::parse(
                r#"{"event":"member_added","targetUserId":"","targetDisplayName":"x"}"#
            )
            .is_none()
        );
    }

    #[test]
    fn space_invite_requires_a_clean_origin() {
        let ok = r#"{"event":"space_invite","spaceId":"s1","spaceInstanceOrigin":"https://chat.example",
            "inviteCode":"abc","snapshot":{"spaceName":"Клуб","icon":null,"avatarColor":"teal",
            "memberCount":3,"description":null,"instanceName":"Chat"}}"#;
        let DmSystemEvent::SpaceInvite(invite) = DmSystemEvent::parse(ok).unwrap() else {
            panic!("ожидалось приглашение");
        };
        assert_eq!(invite.snapshot.member_count, 3);
        let bad = ok.replace("https://chat.example", "https://chat.example/path");
        assert!(DmSystemEvent::parse(&bad).is_none());
        let evil = ok.replace("https://chat.example", "javascript:alert(1)");
        assert!(DmSystemEvent::parse(&evil).is_none());
    }
}
