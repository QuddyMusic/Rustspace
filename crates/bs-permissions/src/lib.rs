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

fn fnv1a64(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for unit in text.encode_utf16() {
        hash ^= u64::from(unit);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Версия набора прав роли (`rolePermissionsVersion`).
///
/// FNV-1a 64 от строки `role:<права>`. Клиент присылает её с изменением прав,
/// сервер отказывает (`role_permissions_conflict`), если роль с тех пор
/// изменилась.
#[must_use]
pub fn permissions_version(bits: Permissions) -> String {
    fnv1a64(&format!("role:{}", permissions_to_string(bits)))
}

/// Версия переопределения (`overrideVersion`): для отсутствующего — `'none'`,
/// иначе FNV-1a 64 от строки `override:<allow>:<deny>`.
#[must_use]
pub fn override_version(row: Option<(Permissions, Permissions)>) -> String {
    row.map_or_else(
        || NO_OVERRIDE_VERSION.to_owned(),
        |(allow, deny)| {
            fnv1a64(&format!(
                "override:{}:{}",
                permissions_to_string(allow),
                permissions_to_string(deny)
            ))
        },
    )
}

/// Место участника в иерархии пространства (`HierarchyStanding`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Standing {
    /// Владелец пространства.
    pub is_owner: bool,
    /// Администратор инстанса.
    pub is_instance_admin: bool,
    /// Позиция высшей роли, не считая `@everyone` (без ролей — 0).
    pub top_position: i64,
}

/// Может ли `actor` действовать на участника `target`.
///
/// Исключить, забанить, менять роли и ник: владельца не трогает никто,
/// владелец и администратор инстанса могут всё, остальные — только строго
/// нижестоящих. Право `ADMINISTRATOR` иерархию не обходит.
#[must_use]
pub const fn can_act_on_member(actor: Standing, target: Standing) -> bool {
    if target.is_owner {
        return false;
    }
    if actor.is_owner || actor.is_instance_admin {
        return true;
    }
    actor.top_position > target.top_position
}

/// Может ли `actor` управлять ролью на позиции `role_position`: только
/// строго ниже своей высшей роли (владелец и администратор инстанса — любой).
#[must_use]
pub const fn can_manage_role_at(actor: Standing, role_position: i64) -> bool {
    if actor.is_owner || actor.is_instance_admin {
        return true;
    }
    role_position < actor.top_position
}

/// Отказ по правилу «менять можно только биты, которые есть у самого».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldBitsRefusal {
    /// Выдаёт бит, которого нет у действующего.
    Grant,
    /// Запрещает бит, которого нет у действующего (только переопределения).
    Deny,
    /// Снимает бит, которого нет у действующего.
    Change,
}

impl HeldBitsRefusal {
    /// Код ошибки сервера.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Grant => "cannot_grant_unowned_permissions",
            Self::Deny => "cannot_deny_unowned_permissions",
            Self::Change => "cannot_change_unowned_permissions",
        }
    }
}

/// Биты, которых у действующего нет (у обладателя всех прав таких нет).
fn unheld(held: Permissions) -> Permissions {
    if held.is_all() {
        Permissions::empty()
    } else {
        !held
    }
}

/// Можно ли изменить биты роли с `before` на `after`, имея `held`.
#[must_use]
pub fn role_bits_change_refusal(
    held: Permissions,
    before: Permissions,
    after: Permissions,
) -> Option<HeldBitsRefusal> {
    let unheld = unheld(held);
    if !(after & !before & unheld).is_empty() {
        return Some(HeldBitsRefusal::Grant);
    }
    if !(before & !after & unheld).is_empty() {
        return Some(HeldBitsRefusal::Change);
    }
    None
}

/// Можно ли изменить переопределение (пары `(allow, deny)`; `None` — его нет).
#[must_use]
pub fn override_change_refusal(
    held: Permissions,
    before: Option<(Permissions, Permissions)>,
    after: Option<(Permissions, Permissions)>,
) -> Option<HeldBitsRefusal> {
    let (old_allow, old_deny) = before.unwrap_or((Permissions::empty(), Permissions::empty()));
    let (new_allow, new_deny) = after.unwrap_or((Permissions::empty(), Permissions::empty()));
    let unheld = unheld(held);
    if !(new_allow & !old_allow & unheld).is_empty() {
        return Some(HeldBitsRefusal::Grant);
    }
    if !(new_deny & !old_deny & unheld).is_empty() {
        return Some(HeldBitsRefusal::Deny);
    }
    if !(((old_allow & !new_allow) | (old_deny & !new_deny)) & unheld).is_empty() {
        return Some(HeldBitsRefusal::Change);
    }
    None
}

/// Скрыт ли канал или категория от `@everyone`: запрет в переопределении
/// роли `@everyone` содержит `VIEW_CHANNEL`.
#[must_use]
pub const fn is_hidden_from_everyone(everyone_deny: Permissions) -> bool {
    everyone_deny.contains(Permissions::VIEW_CHANNEL)
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
    fn versions_match_the_server() {
        // Значения посчитаны оригинальным `rolePermissionsVersion` /
        // `overrideVersion` (FNV-1a 64 от текста).
        assert_eq!(permissions_version(Permissions::empty()), fnv1a64("role:0"));
        assert_eq!(fnv1a64(""), "cbf29ce484222325");
        assert_eq!(fnv1a64("a"), "af63dc4c8601ec8c");
        assert_eq!(override_version(None), NO_OVERRIDE_VERSION);
        assert_ne!(
            override_version(Some((Permissions::empty(), Permissions::empty()))),
            NO_OVERRIDE_VERSION
        );
    }

    #[test]
    fn held_bits_rules() {
        let actor = Permissions::SEND_MESSAGES | Permissions::VIEW_CHANNEL;
        assert_eq!(
            role_bits_change_refusal(actor, Permissions::empty(), Permissions::BAN_MEMBERS),
            Some(HeldBitsRefusal::Grant)
        );
        assert_eq!(
            role_bits_change_refusal(actor, Permissions::BAN_MEMBERS, Permissions::empty()),
            Some(HeldBitsRefusal::Change)
        );
        assert!(
            role_bits_change_refusal(actor, Permissions::empty(), Permissions::SEND_MESSAGES)
                .is_none()
        );
        // ADMINISTRATOR сам по себе чужих битов не даёт; все права — дают.
        assert!(
            role_bits_change_refusal(
                Permissions::ADMINISTRATOR,
                Permissions::empty(),
                Permissions::BAN_MEMBERS
            )
            .is_some()
        );
        assert!(
            role_bits_change_refusal(Permissions::all(), Permissions::empty(), Permissions::all())
                .is_none()
        );
        assert_eq!(
            override_change_refusal(
                actor,
                None,
                Some((Permissions::empty(), Permissions::SPEAK))
            ),
            Some(HeldBitsRefusal::Deny)
        );
    }

    #[test]
    fn hierarchy() {
        let owner = Standing {
            is_owner: true,
            ..Standing::default()
        };
        let high = Standing {
            top_position: 5,
            ..Standing::default()
        };
        let low = Standing {
            top_position: 2,
            ..Standing::default()
        };
        assert!(can_act_on_member(owner, high));
        assert!(!can_act_on_member(high, owner));
        assert!(can_act_on_member(high, low));
        assert!(!can_act_on_member(low, high));
        assert!(!can_act_on_member(high, high));
        assert!(can_manage_role_at(high, 4));
        assert!(!can_manage_role_at(high, 5));
        assert!(can_manage_role_at(owner, 99));
    }

    #[test]
    fn hidden_from_everyone() {
        assert!(is_hidden_from_everyone(Permissions::VIEW_CHANNEL));
        assert!(!is_hidden_from_everyone(Permissions::SEND_MESSAGES));
    }
}
