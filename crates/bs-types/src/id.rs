//! Snowflake-идентификаторы.
//!
//! Сервер использует Snowflake ID (64-битные), но в JSON они передаются
//! **строками**, потому что JS теряет точность выше 2^53. Мы следуем этому
//! соглашению: все ID у нас — `String`.
//!
//! Позже можно заменить на newtype-ы поверх `String` с `Deref<Target = str>`,
//! чтобы компилятор не путал `UserId` и `ChannelId`.

/// ID пользователя (Snowflake как строка).
pub type UserId = String;
/// ID пространства (Space).
pub type SpaceId = String;
/// ID канала.
pub type ChannelId = String;
/// ID сообщения.
pub type MessageId = String;
/// ID категории каналов.
pub type CategoryId = String;
/// ID роли.
pub type RoleId = String;
/// ID приглашения.
pub type InviteCode = String;
/// ID эмодзи-реакции (может быть Unicode, не Snowflake).
pub type Emoji = String;
