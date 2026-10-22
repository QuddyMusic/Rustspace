//! Права Backspace: битовые флаги, иерархия ролей, override-логика.
//!
//! Контракт: `@backspace/shared/permissions.ts`. Ключевые правила:
//!
//! - Права передаются по проводу как **decimal string**, не как JSON-число
//!   (JS теряет точность выше 2^53, а у нас биты вплоть до 1<<26).
//! - `ADMINISTRATOR` обходит все проверки.
//! - Роль может менять только те биты, которые есть у неё самой (held-bits rule).
//! - Версии прав/оверрайдов — FNV-1a 64-bit hex; `'none'` — для отсутствующего
//!   оверрайда.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use bitflags::bitflags;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

bitflags! {
    /// Биты прав. Порядок соответствует `PermissionBits` из TS-версии.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct Permissions: u64 {
        const ADMINISTRATOR        = 1 << 0;
        const VIEW_CHANNEL         = 1 << 1;
        const MANAGE_CHANNELS      = 1 << 2;
        const MANAGE_ROLES         = 1 << 3;
        const MANAGE_SPACE         = 1 << 4;
        const CREATE_INVITE        = 1 << 5;
        const KICK_MEMBERS         = 1 << 6;
        const BAN_MEMBERS          = 1 << 7;
        const SEND_MESSAGES        = 1 << 10;
        const MANAGE_MESSAGES      = 1 << 11;
        const ATTACH_FILES         = 1 << 12;
        const READ_MESSAGE_HISTORY = 1 << 13;
        const ADD_REACTIONS        = 1 << 14;
        const MENTION_EVERYONE     = 1 << 15;
        const CONNECT              = 1 << 20;
        const SPEAK                = 1 << 21;
        const MUTE_MEMBERS         = 1 << 22;
        const DEAFEN_MEMBERS       = 1 << 23;
        const MOVE_MEMBERS         = 1 << 24;
        const STREAM               = 1 << 25;
        const DISCONNECT_MEMBERS   = 1 << 26;
    }
}

/// Права по умолчанию для роли `@everyone`.
pub const DEFAULT_EVERYONE_PERMISSIONS: Permissions = Permissions::VIEW_CHANNEL
    .union(Permissions::SEND_MESSAGES)
    .union(Permissions::READ_MESSAGE_HISTORY)
    .union(Permissions::ADD_REACTIONS)
    .union(Permissions::CREATE_INVITE)
    .union(Permissions::ATTACH_FILES)
    .union(Permissions::CONNECT)
    .union(Permissions::SPEAK)
    .union(Permissions::STREAM);

/// Специальная строка версии «оверрайда нет».
pub const NO_OVERRIDE_VERSION: &str = "none";

/// Ошибки разбора строки прав.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PermissionParseError {
    /// Строка не является каноническим decimal-числом.
    #[error("некорректная строка прав: {0:?}")]
    InvalidString(String),
    /// Число не влезает в u64.
    #[error("права не влезают в u64: {0:?}")]
    Overflow(String),
}

/// Каноническая проверка: `0` либо `[1-9][0-9]*`.
fn is_canonical_decimal(s: &str) -> bool {
    let Some(first) = s.as_bytes().first() else {
        return false;
    };
    if *first == b'0' {
        return s.len() == 1;
    }
    s.bytes().all(|b| b.is_ascii_digit())
}

/// Проверить, есть ли конкретный бит (с обходом через ADMINISTRATOR).
pub fn has_permission_bit(bits: Permissions, flag: Permissions) -> bool {
    bits.contains(Permissions::ADMINISTRATOR) || bits.contains(flag)
}

/// Сериализовать права в decimal string.
pub fn permissions_to_string(bits: Permissions) -> String {
    bits.bits().to_string()
}

/// Разобрать decimal string в права.
///
/// Поддерживает fallback на legacy JSON-массив строк (как `stringToPermissions`
/// в TS-версии).
///
/// # Errors
///
/// Возвращает [`PermissionParseError`], если строка не является ни decimal, ни
/// JSON-массивом известных имён флагов.
pub fn parse_permission_string(raw: &str) -> Result<Permissions, PermissionParseError> {
    let trimmed = raw.trim();
    if is_canonical_decimal(trimmed) {
        let n: u64 = trimmed
            .parse()
            .map_err(|_| PermissionParseError::Overflow(trimmed.to_owned()))?;
        return Ok(Permissions::from_bits_truncate(n));
    }
    if let Ok(names) = serde_json::from_str::<Vec<String>>(trimmed) {
        let mut acc = Permissions::empty();
        for name in names {
            if let Some(flag) = Permissions::from_name(&name) {
                acc |= flag;
            }
        }
        return Ok(acc);
    }
    Err(PermissionParseError::InvalidString(trimmed.to_owned()))
}

/// Канонический decimal-string (идемпотентно).
///
/// # Errors
///
/// Возвращает [`PermissionParseError`], если вход не разбирается.
pub fn canonical_permission_string(raw: &str) -> Result<String, PermissionParseError> {
    parse_permission_string(raw).map(permissions_to_string)
}

/// Вычислить FNV-1a 64-bit hex для набора прав.
///
/// Используется для optimistic locking: сервер присылает `permissionsVersion`,
/// клиент сравнивает с локально посчитанным.
pub fn permissions_version(bits: Permissions) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bits.bits().to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Версия оверрайда: для отсутствующего — `'none'`.
pub fn override_version(allow: Permissions, deny: Permissions) -> String {
    if allow.is_empty() && deny.is_empty() {
        NO_OVERRIDE_VERSION.to_owned()
    } else {
        let a = permissions_version(allow);
        let d = permissions_version(deny);
        format!("{a}:{d}")
    }
}

/// Проверить, может ли `actor` действовать на участника с учётом позиции роли.
pub fn can_act_on_member(actor_top: i64, target_top: i64, actor: Permissions) -> bool {
    actor.contains(Permissions::ADMINISTRATOR) || actor_top > target_top
}

/// Может ли роль на позиции `actor_pos` управлять ролью на позиции `target_pos`.
pub fn can_manage_role_at(actor_pos: i64, target_pos: i64, actor: Permissions) -> bool {
    actor.contains(Permissions::ADMINISTRATOR) || actor_pos > target_pos
}

/// Причина отказа при изменении битов роли (held-bits rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleBitsChangeRefusal {
    /// Актёр пытается выдать биты, которых у него самого нет.
    GrantsUnheldBits,
    /// Актёр пытается снять биты, которых у него самого нет.
    RevokesUnheldBits,
}

/// Проверить, может ли актёр изменить набор битов роли.
pub fn role_bits_change_refusal(
    actor: Permissions,
    old: Permissions,
    new: Permissions,
) -> Option<RoleBitsChangeRefusal> {
    if actor.contains(Permissions::ADMINISTRATOR) {
        return None;
    }
    let granted = new & !old;
    let revoked = old & !new;
    if !actor.contains(granted) {
        return Some(RoleBitsChangeRefusal::GrantsUnheldBits);
    }
    if !actor.contains(revoked) {
        return Some(RoleBitsChangeRefusal::RevokesUnheldBits);
    }
    None
}

/// Причина отказа при изменении оверрайда (held-bits rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideChangeRefusal {
    /// Актёр пытается изменить биты, которых у него нет.
    UnheldBits,
}

/// Проверить, может ли актёр изменить allow/deny оверрайда.
pub fn override_change_refusal(
    actor: Permissions,
    old_allow: Permissions,
    old_deny: Permissions,
    new_allow: Permissions,
    new_deny: Permissions,
) -> Option<OverrideChangeRefusal> {
    if actor.contains(Permissions::ADMINISTRATOR) {
        return None;
    }
    let touched = (old_allow ^ new_allow) | (old_deny ^ new_deny);
    if actor.contains(touched) {
        None
    } else {
        Some(OverrideChangeRefusal::UnheldBits)
    }
}

/// Приватный канал — скрыт от `@everyone` (нет `VIEW_CHANNEL`).
pub fn is_hidden_from_everyone(everyone: Permissions) -> bool {
    !everyone.contains(Permissions::VIEW_CHANNEL)
}

/// Отфильтровать ID каналов, скрытых от `@everyone`.
pub fn ids_hidden_from_everyone<'a>(
    everyone: Permissions,
    channels: impl IntoIterator<Item = (&'a str, Permissions)>,
) -> Vec<&'a str> {
    if !is_hidden_from_everyone(everyone) {
        return Vec::new();
    }
    channels
        .into_iter()
        .filter(|(_, perms)| !perms.contains(Permissions::VIEW_CHANNEL))
        .map(|(id, _)| id)
        .collect()
}

// --- serde: права как decimal string ---

impl Serialize for Permissions {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&permissions_to_string(*self))
    }
}

impl<'de> Deserialize<'de> for Permissions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        parse_permission_string(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_roundtrip() {
        let bits = Permissions::ADMINISTRATOR | Permissions::SEND_MESSAGES;
        let s = permissions_to_string(bits);
        assert_eq!(parse_permission_string(&s).unwrap(), bits);
    }

    #[test]
    fn rejects_leading_zero() {
        assert!(matches!(
            parse_permission_string("007"),
            Err(PermissionParseError::InvalidString(_))
        ));
    }

    #[test]
    fn accepts_zero() {
        assert_eq!(parse_permission_string("0").unwrap(), Permissions::empty());
    }

    #[test]
    fn legacy_array_fallback() {
        let bits = parse_permission_string(r#"["VIEW_CHANNEL","SEND_MESSAGES"]"#).unwrap();
        assert!(bits.contains(Permissions::VIEW_CHANNEL));
        assert!(bits.contains(Permissions::SEND_MESSAGES));
    }

    #[test]
    fn serde_is_string() {
        let json = serde_json::to_string(&Permissions::VIEW_CHANNEL).unwrap();
        assert_eq!(json, "\"2\"");
    }

    #[test]
    fn serde_roundtrip() {
        let bits = Permissions::SPEAK | Permissions::STREAM;
        let json = serde_json::to_string(&bits).unwrap();
        let back: Permissions = serde_json::from_str(&json).unwrap();
        assert_eq!(back, bits);
    }

    #[test]
    fn administrator_bypasses() {
        let admin = Permissions::ADMINISTRATOR;
        assert!(has_permission_bit(admin, Permissions::BAN_MEMBERS));
    }

    #[test]
    fn version_is_stable() {
        let a = Permissions::VIEW_CHANNEL;
        assert_eq!(permissions_version(a), permissions_version(a));
        assert_ne!(
            permissions_version(a),
            permissions_version(Permissions::SPEAK)
        );
    }

    #[test]
    fn no_override_version_for_empty() {
        assert_eq!(
            override_version(Permissions::empty(), Permissions::empty()),
            NO_OVERRIDE_VERSION
        );
    }

    #[test]
    fn held_bits_grants_unheld() {
        let actor = Permissions::SEND_MESSAGES;
        assert_eq!(
            role_bits_change_refusal(actor, Permissions::empty(), Permissions::BAN_MEMBERS),
            Some(RoleBitsChangeRefusal::GrantsUnheldBits)
        );
    }

    #[test]
    fn held_bits_ok() {
        let actor = Permissions::SEND_MESSAGES | Permissions::VIEW_CHANNEL;
        assert!(
            role_bits_change_refusal(actor, Permissions::empty(), Permissions::SEND_MESSAGES)
                .is_none()
        );
    }

    #[test]
    fn hidden_from_everyone() {
        assert!(is_hidden_from_everyone(Permissions::SEND_MESSAGES));
        assert!(!is_hidden_from_everyone(Permissions::VIEW_CHANNEL));
    }

    #[test]
    fn ids_hidden_filter() {
        // everyone без VIEW_CHANNEL → каналы без VIEW_CHANNEL скрыты.
        let everyone = Permissions::empty();
        let chans = [
            ("a", Permissions::VIEW_CHANNEL),
            ("b", Permissions::empty()),
        ];
        assert_eq!(ids_hidden_from_everyone(everyone, chans), vec!["b"]);
        // everyone с VIEW_CHANNEL → скрытых нет.
        assert!(ids_hidden_from_everyone(Permissions::VIEW_CHANNEL, chans).is_empty());
    }
}
