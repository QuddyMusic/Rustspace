//! Боты (расширение Rustspace).
//!
//! Бот — обычный пользователь инстанса, которым управляет его владелец:
//! он входит токеном `rsb_<id>_<секрет>` (заголовок `Authorization: Bot …`
//! или `Bearer …`, а для WebSocket — то же поле `token` в `auth`) и
//! пользуется тем же API и теми же событиями, что и клиенты. Пароля у бота
//! нет; в базе лежит только SHA-256 секрета. Часть API ботам закрыта:
//! вход и учётная запись, друзья, двухфакторная защита, свои боты и мосты,
//! администрирование инстанса.

use std::collections::HashSet;
use std::sync::{LazyLock, RwLock};

use rand::RngCore as _;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;

use crate::auth::AuthUser;
use crate::db::{DbResult, Tx};
use crate::error::{ApiError, ApiResult};
use crate::params;
use crate::state::{AppState, now_ms};

/// Приставка токена бота.
pub const TOKEN_PREFIX: &str = "rsb_";
/// Ботов у одного владельца не больше.
pub const MAX_BOTS: i64 = 25;

/// Кто из пользователей — бот (для пометки `bot: true` в ответах).
static BOTS: LazyLock<RwLock<HashSet<String>>> = LazyLock::new(RwLock::default);

/// Бот ли этот пользователь.
#[must_use]
pub fn is_bot(user_id: &str) -> bool {
    BOTS.read().is_ok_and(|bots| bots.contains(user_id))
}

fn remember(user_id: &str, bot: bool) {
    if let Ok(mut bots) = BOTS.write() {
        if bot {
            bots.insert(user_id.to_owned());
        } else {
            bots.remove(user_id);
        }
    }
}

/// Загружает список ботов при старте.
pub fn load(state: &AppState) {
    let app = state.clone();
    tokio::spawn(async move {
        let ids = app
            .run(|tx, _| {
                Ok(tx
                    .query("SELECT user_id FROM bs_bots", params![])?
                    .iter()
                    .filter_map(|row| row.text("user_id").ok())
                    .collect::<Vec<_>>())
            })
            .await
            .unwrap_or_default();
        for id in ids {
            remember(&id, true);
        }
    });
}

fn hash(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// Новый токен для бота: (токен целиком, хеш секрета).
fn new_token(user_id: &str) -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let secret = hex::encode(bytes);
    (format!("{TOKEN_PREFIX}{user_id}_{secret}"), hash(&secret))
}

/// Похоже ли на токен бота.
#[must_use]
pub fn looks_like_token(token: &str) -> bool {
    token.starts_with(TOKEN_PREFIX)
}

/// Проверяет токен бота.
///
/// # Errors
/// Токен неверен или бот удалён.
pub fn verify(tx: &mut dyn Tx, token: &str) -> ApiResult<AuthUser> {
    let invalid = || ApiError::unauthorized("unauthorized").text("Invalid bot token");
    let rest = token.strip_prefix(TOKEN_PREFIX).ok_or_else(invalid)?;
    let (id, secret) = rest.split_once('_').ok_or_else(invalid)?;
    let row = tx
        .query_one(
            "SELECT b.token_hash, u.username, u.is_deleted FROM bs_bots b \
             JOIN users u ON u.id = b.user_id WHERE b.user_id = ?",
            params![id],
        )?
        .ok_or_else(invalid)?;
    let stored = row.text("token_hash")?;
    if !bool::from(hash(secret).as_bytes().ct_eq(stored.as_bytes())) {
        return Err(invalid());
    }
    if row.opt_flag("is_deleted")? == Some(true) {
        return Err(ApiError::unauthorized("account_deleted"));
    }
    Ok(AuthUser {
        id: id.to_owned(),
        username: row.text("username")?,
        home_instance: None,
        bot: true,
    })
}

/// Части API, закрытые для ботов.
const FORBIDDEN: &[&str] = &[
    "/api/auth/",
    "/api/social/",
    "/api/users/@me/verify-password",
    "/api/users/@me/change-password",
    "/api/users/@me/reattach",
    "/api/users/@me/federation",
    "/api/rustspace/bots",
    "/api/rustspace/personal-bridges",
    "/api/admin",
    "/api/rustspace/admin",
    "/api/federation",
];

/// Можно ли боту этот запрос. Удалить учётную запись бот тоже не может —
/// это делает владелец.
#[must_use]
pub fn allowed(method: &axum::http::Method, path: &str) -> bool {
    if method == axum::http::Method::DELETE && path.trim_end_matches('/') == "/api/users/@me" {
        return false;
    }
    !FORBIDDEN.iter().any(|prefix| path.starts_with(prefix))
}

// ─── Управление своими ботами ─────────────────────────────────────────────────

fn bot_json(tx: &mut dyn Tx, id: &str) -> DbResult<Option<Value>> {
    let Some(row) = tx.query_one(
        "SELECT b.user_id, b.description, b.created_at, u.username, u.display_name, \
         u.avatar, u.avatar_color FROM bs_bots b JOIN users u ON u.id = b.user_id \
         WHERE b.user_id = ? AND COALESCE(u.is_deleted, 0) = 0",
        params![id],
    )?
    else {
        return Ok(None);
    };
    let mut spaces = Vec::new();
    for space in tx.query(
        "SELECT s.id, s.name FROM space_members m JOIN spaces s ON s.id = m.space_id \
         WHERE m.user_id = ? ORDER BY m.joined_at",
        params![id],
    )? {
        spaces.push(json!({"id": space.text("id")?, "name": space.text("name")?}));
    }
    Ok(Some(json!({
        "id": row.text("user_id")?,
        "username": row.text("username")?,
        "displayName": row.opt_text("display_name")?,
        "avatar": row.opt_text("avatar")?,
        "avatarColor": row.opt_text("avatar_color")?,
        "description": row.opt_text("description")?,
        "createdAt": row.int("created_at")?,
        "spaces": spaces,
    })))
}

fn own_bot(tx: &mut dyn Tx, owner: &str, id: &str) -> ApiResult<()> {
    let row = tx
        .query_one(
            "SELECT owner_id FROM bs_bots WHERE user_id = ?",
            params![id],
        )?
        .ok_or_else(|| ApiError::not_found("bot_not_found"))?;
    if row.text("owner_id")? != owner {
        return Err(ApiError::not_found("bot_not_found"));
    }
    Ok(())
}

/// Свои боты.
///
/// # Errors
/// Ошибка базы.
pub fn list(tx: &mut dyn Tx, owner: &str) -> ApiResult<Vec<Value>> {
    let ids: Vec<String> = tx
        .query(
            "SELECT user_id FROM bs_bots WHERE owner_id = ? ORDER BY created_at",
            params![owner],
        )?
        .iter()
        .filter_map(|row| row.text("user_id").ok())
        .collect();
    let mut out = Vec::new();
    for id in ids {
        if let Some(bot) = bot_json(tx, &id)? {
            out.push(bot);
        }
    }
    Ok(out)
}

/// Имя пользователя бота: 2–32 символа из латиницы, цифр, `_`, `.`, `-`.
fn valid_username(name: &str) -> bool {
    (2..=32).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Новый бот: возвращает его вид и токен (показывается один раз).
///
/// # Errors
/// Имя занято или неверно, предел ботов.
pub fn create(
    tx: &mut dyn Tx,
    state: &AppState,
    owner: &str,
    username: &str,
    display: Option<&str>,
    description: Option<&str>,
) -> ApiResult<(Value, String)> {
    let username = username.trim().to_lowercase();
    if !valid_username(&username) {
        return Err(ApiError::bad_request("bot_username_invalid"));
    }
    let count = tx
        .query_one(
            "SELECT count(*) AS n FROM bs_bots WHERE owner_id = ?",
            params![owner],
        )?
        .map_or(Ok(0), |row| row.int("n"))?;
    if count >= MAX_BOTS {
        return Err(ApiError::conflict("bot_limit").detail("max", MAX_BOTS));
    }
    let taken = tx
        .query_one(
            "SELECT 1 AS one FROM users WHERE lower(username) = ?",
            params![&username],
        )?
        .is_some();
    if taken {
        return Err(ApiError::conflict("username_taken"));
    }
    let id = state.ids.next();
    let now = now_ms();
    let display = display
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| d.chars().take(32).collect::<String>());
    let description = description
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| d.chars().take(400).collect::<String>());
    tx.execute(
        // Пароль «!» не проходит ни одну проверку bcrypt: войти можно только токеном.
        "INSERT INTO users (id, username, display_name, password_hash, discoverable, created_at) \
         VALUES (?, ?, ?, '!', 0, ?)",
        params![&id, &username, &display, now],
    )?;
    let (token, token_hash) = new_token(&id);
    tx.execute(
        "INSERT INTO bs_bots (user_id, owner_id, token_hash, description, created_at) \
         VALUES (?, ?, ?, ?, ?)",
        params![&id, owner, &token_hash, &description, now],
    )?;
    remember(&id, true);
    let bot = bot_json(tx, &id)?.ok_or_else(ApiError::internal)?;
    Ok((bot, token))
}

/// Меняет имя для показа и описание.
///
/// # Errors
/// Бот не свой.
pub fn update(
    tx: &mut dyn Tx,
    owner: &str,
    id: &str,
    display: Option<Option<String>>,
    description: Option<Option<String>>,
) -> ApiResult<Value> {
    own_bot(tx, owner, id)?;
    if let Some(display) = display {
        let display = display
            .map(|d| d.trim().chars().take(32).collect::<String>())
            .filter(|d| !d.is_empty());
        tx.execute(
            "UPDATE users SET display_name = ?, profile_updated_at = ? WHERE id = ?",
            params![&display, now_ms(), id],
        )?;
    }
    if let Some(description) = description {
        let description = description
            .map(|d| d.trim().chars().take(400).collect::<String>())
            .filter(|d| !d.is_empty());
        tx.execute(
            "UPDATE bs_bots SET description = ? WHERE user_id = ?",
            params![&description, id],
        )?;
    }
    bot_json(tx, id)?.ok_or_else(ApiError::internal)
}

/// Новый токен; старый перестаёт работать (открытые соединения бота
/// закрываются при следующей проверке).
///
/// # Errors
/// Бот не свой.
pub fn regenerate(tx: &mut dyn Tx, owner: &str, id: &str) -> ApiResult<String> {
    own_bot(tx, owner, id)?;
    let (token, token_hash) = new_token(id);
    tx.execute(
        "UPDATE bs_bots SET token_hash = ? WHERE user_id = ?",
        params![&token_hash, id],
    )?;
    Ok(token)
}

/// Удаляет бота: он выходит из пространств, учётная запись помечается
/// удалённой (его сообщения остаются от «удалённого пользователя»).
///
/// # Errors
/// Бот не свой.
pub fn delete(
    tx: &mut dyn Tx,
    out: &mut crate::hub::Outbox,
    owner: &str,
    id: &str,
) -> ApiResult<()> {
    own_bot(tx, owner, id)?;
    let spaces: Vec<String> = tx
        .query(
            "SELECT space_id FROM space_members WHERE user_id = ?",
            params![id],
        )?
        .iter()
        .filter_map(|row| row.text("space_id").ok())
        .collect();
    for space in spaces {
        crate::routes::spaces::remove_member(tx, &space, id)?;
        out.to_space(
            &space,
            &json!({"type": "member_left", "spaceId": space, "userId": id}),
        );
        out.leave_space(id, &space);
    }
    tx.execute("DELETE FROM bs_bots WHERE user_id = ?", params![id])?;
    tx.execute(
        "UPDATE users SET is_deleted = 1, display_name = NULL, avatar = NULL WHERE id = ?",
        params![id],
    )?;
    remember(id, false);
    Ok(())
}

/// Добавляет своего бота в пространство (нужно право «Управлять
/// пространством»).
///
/// # Errors
/// Нет права, бот не свой, уже участник, бан.
pub fn add_to_space(
    tx: &mut dyn Tx,
    out: &mut crate::hub::Outbox,
    owner: &str,
    space: &str,
    bot: &str,
) -> ApiResult<()> {
    own_bot(tx, owner, bot)?;
    if !crate::perms::is_member(tx, space, owner)? {
        return Err(ApiError::not_found("space_not_found"));
    }
    if !crate::perms::has_permission(tx, owner, space, bs_permissions::Permissions::MANAGE_SPACE)? {
        return Err(crate::perms::missing("MANAGE_SPACE"));
    }
    if crate::perms::is_banned(tx, space, bot)? {
        return Err(ApiError::forbidden("user_banned"));
    }
    if crate::perms::is_member(tx, space, bot)? {
        return Err(ApiError::conflict("already_member"));
    }
    crate::routes::spaces::admit(tx, out, space, bot)?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    #[test]
    fn tokens_carry_the_id_and_hash_the_secret() {
        let (token, stored) = new_token("123");
        let secret = token.strip_prefix("rsb_123_").expect("приставка");
        assert_eq!(secret.len(), 64);
        assert_eq!(hash(secret), stored);
        assert!(looks_like_token(&token));
    }

    #[test]
    fn bots_stay_out_of_account_routes() {
        let get = axum::http::Method::GET;
        assert!(allowed(&get, "/api/channels/1/messages"));
        assert!(allowed(&axum::http::Method::PATCH, "/api/users/@me"));
        assert!(!allowed(&axum::http::Method::DELETE, "/api/users/@me"));
        assert!(!allowed(&get, "/api/social/friends"));
        assert!(!allowed(&get, "/api/auth/2fa"));
        assert!(!allowed(&get, "/api/rustspace/bots"));
    }

    #[test]
    fn usernames_are_plain() {
        assert!(valid_username("echo_bot"));
        assert!(!valid_username("a"));
        assert!(!valid_username("бот"));
    }
}
