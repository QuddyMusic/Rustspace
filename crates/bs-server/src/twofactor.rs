//! Двухфакторная аутентификация: одноразовые коды по времени (TOTP,
//! RFC 6238) и запасные коды.
//!
//! Это расширение сервера: у оригинала его нет. Клиент, который о нём не
//! знает, работает как раньше, пока владелец аккаунта не включит защиту.
//! Секрет хранится в базе зашифрованным ключом, производным от секрета
//! сервера: копия базы без конфигурации не даёт подделать коды.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::db::{DbResult, Tx};
use crate::params;
use crate::state::now_ms;

/// Шаг времени кода.
const STEP_SECONDS: i64 = 30;
/// Цифр в коде.
const DIGITS: u32 = 6;
/// Сколько запасных кодов выдаётся.
pub const RECOVERY_CODES: usize = 10;
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Запись base32 без дополнения (RFC 4648), как ждут приложения-аутентификаторы.
#[must_use]
pub fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(BASE32[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(BASE32[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// Разбор base32; пробелы, дефисы и регистр не важны.
#[must_use]
pub fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.chars().filter(|c| !matches!(c, ' ' | '-' | '=')) {
        let value = BASE32
            .iter()
            .position(|symbol| char::from(*symbol) == c.to_ascii_uppercase())?;
        buffer = (buffer << 5) | u32::try_from(value).ok()?;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// Код для шага времени `step` (HOTP, RFC 4226).
fn code_at(secret: &[u8], step: i64) -> Option<u32> {
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(secret).ok()?;
    mac.update(&u64::try_from(step).ok()?.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[19] & 0x0f);
    let binary = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    Some(binary % 10u32.pow(DIGITS))
}

/// Шаг времени, которому соответствует введённый код: текущий или соседний
/// (часы телефона могут отставать или спешить на полминуты). Шаг не раньше
/// `after` — использованный код второй раз не принимается.
#[must_use]
pub fn matching_step(secret: &[u8], code: &str, now_ms: i64, after: i64) -> Option<i64> {
    let code = code.trim();
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let entered: u32 = code.parse().ok()?;
    let current = now_ms.div_euclid(1000) / STEP_SECONDS;
    let mut found = None;
    // Проверяются все три шага без раннего выхода: время ответа не зависит
    // от того, какой подошёл.
    for step in [current - 1, current, current + 1] {
        let expected = code_at(secret, step)?;
        let equal: bool = expected.to_be_bytes().ct_eq(&entered.to_be_bytes()).into();
        if equal && step > after && found.is_none() {
            found = Some(step);
        }
    }
    found
}

fn cipher(server_secret: &str) -> Option<ChaCha20Poly1305> {
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(None, server_secret.as_bytes())
        .expand(b"bs-server totp secret v1", &mut key)
        .ok()?;
    ChaCha20Poly1305::new_from_slice(&key).ok()
}

/// Шифрует секрет для хранения.
#[must_use]
pub fn seal(server_secret: &str, secret: &[u8]) -> Option<String> {
    let nonce: [u8; 12] = rand::random();
    let sealed = cipher(server_secret)?
        .encrypt(Nonce::from_slice(&nonce), secret)
        .ok()?;
    Some(STANDARD.encode([nonce.as_slice(), &sealed].concat()))
}

/// Расшифровывает сохранённый секрет.
#[must_use]
pub fn open(server_secret: &str, stored: &str) -> Option<Vec<u8>> {
    let bytes = STANDARD.decode(stored).ok()?;
    let (nonce, sealed) = bytes.split_at_checked(12)?;
    cipher(server_secret)?
        .decrypt(Nonce::from_slice(nonce), sealed)
        .ok()
}

/// Новый секрет: 160 бит, как советует RFC 4226.
#[must_use]
pub fn new_secret() -> [u8; 20] {
    rand::random()
}

/// Ссылка для приложения-аутентификатора (её же кодируют в QR).
#[must_use]
pub fn otpauth_url(issuer: &str, username: &str, secret: &[u8]) -> String {
    let encode = crate::routes::uploads::encode_uri_component;
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}",
        encode(issuer),
        encode(username),
        base32_encode(secret),
        encode(issuer),
    )
}

/// Запасной код вида `abcd-efgh-jkmn` (60 бит случайности).
fn new_recovery_code() -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghjkmnpqrstuvwxyz234567891";
    let bytes: [u8; 12] = rand::random();
    let letters: Vec<char> = bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(byte & 31)]))
        .collect();
    letters
        .chunks(4)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

fn recovery_hash(code: &str) -> String {
    let normal: String = code
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    hex::encode(Sha256::digest(normal.as_bytes()))
}

/// Состояние защиты аккаунта.
#[derive(Debug, Clone)]
pub struct State {
    /// Зашифрованный секрет.
    pub sealed: String,
    /// Когда защита включена; `None` — настройка начата, но не подтверждена.
    pub enabled_at: Option<i64>,
    /// Последний принятый шаг времени.
    pub last_step: i64,
}

/// Читает состояние защиты пользователя.
///
/// # Errors
/// Ошибка базы.
pub fn load(tx: &mut dyn Tx, user_id: &str) -> DbResult<Option<State>> {
    tx.query_one(
        "SELECT secret, enabled_at, last_step FROM bs_totp WHERE user_id = ?",
        params![user_id],
    )?
    .map(|row| {
        Ok(State {
            sealed: row.text("secret")?,
            enabled_at: row.opt_int("enabled_at")?,
            last_step: row.int("last_step")?,
        })
    })
    .transpose()
}

/// Включена ли защита.
///
/// # Errors
/// Ошибка базы.
pub fn is_enabled(tx: &mut dyn Tx, user_id: &str) -> DbResult<bool> {
    Ok(load(tx, user_id)?.is_some_and(|state| state.enabled_at.is_some()))
}

/// Сохраняет новый, ещё не подтверждённый секрет.
///
/// # Errors
/// Ошибка базы.
pub fn begin(tx: &mut dyn Tx, user_id: &str, sealed: &str) -> DbResult<()> {
    tx.execute(
        "DELETE FROM bs_totp WHERE user_id = ? AND enabled_at IS NULL",
        params![user_id],
    )?;
    tx.execute(
        "INSERT INTO bs_totp (user_id, secret, enabled_at, last_step, created_at) VALUES (?, ?, NULL, 0, ?)",
        params![user_id, sealed, now_ms()],
    )?;
    Ok(())
}

/// Проверяет код по времени и запоминает шаг, чтобы код не сработал дважды.
///
/// # Errors
/// Ошибка базы.
pub fn check_code(
    tx: &mut dyn Tx,
    server_secret: &str,
    user_id: &str,
    state: &State,
    code: &str,
) -> DbResult<bool> {
    let Some(secret) = open(server_secret, &state.sealed) else {
        return Ok(false);
    };
    let Some(step) = matching_step(&secret, code, now_ms(), state.last_step) else {
        return Ok(false);
    };
    tx.execute(
        "UPDATE bs_totp SET last_step = ? WHERE user_id = ?",
        params![step, user_id],
    )?;
    Ok(true)
}

/// Гасит запасной код; `false` — такого кода нет или он уже использован.
///
/// # Errors
/// Ошибка базы.
pub fn use_recovery_code(tx: &mut dyn Tx, user_id: &str, code: &str) -> DbResult<bool> {
    let changed = tx.execute(
        "UPDATE bs_totp_recovery SET used_at = ? WHERE user_id = ? AND code_hash = ? AND used_at IS NULL",
        params![now_ms(), user_id, recovery_hash(code)],
    )?;
    Ok(changed == 1)
}

/// Выдаёт новый набор запасных кодов взамен прежнего.
///
/// # Errors
/// Ошибка базы.
pub fn issue_recovery_codes(tx: &mut dyn Tx, user_id: &str) -> DbResult<Vec<String>> {
    tx.execute(
        "DELETE FROM bs_totp_recovery WHERE user_id = ?",
        params![user_id],
    )?;
    let mut codes = Vec::with_capacity(RECOVERY_CODES);
    while codes.len() < RECOVERY_CODES {
        let code = new_recovery_code();
        let added = tx.execute(
            "INSERT INTO bs_totp_recovery (user_id, code_hash, used_at) VALUES (?, ?, NULL) ON CONFLICT DO NOTHING",
            params![user_id, recovery_hash(&code)],
        )?;
        if added == 1 {
            codes.push(code);
        }
    }
    Ok(codes)
}

/// Сколько запасных кодов осталось.
///
/// # Errors
/// Ошибка базы.
pub fn recovery_codes_left(tx: &mut dyn Tx, user_id: &str) -> DbResult<i64> {
    tx.query_one(
        "SELECT count(*) AS n FROM bs_totp_recovery WHERE user_id = ? AND used_at IS NULL",
        params![user_id],
    )?
    .map_or(Ok(0), |row| row.int("n"))
}

/// Снимает защиту с аккаунта.
///
/// # Errors
/// Ошибка базы.
pub fn remove(tx: &mut dyn Tx, user_id: &str) -> DbResult<()> {
    tx.execute("DELETE FROM bs_totp WHERE user_id = ?", params![user_id])?;
    tx.execute(
        "DELETE FROM bs_totp_recovery WHERE user_id = ?",
        params![user_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Проверочные значения из приложения B к RFC 6238 (SHA-1, секрет
    /// «12345678901234567890»), приведённые к шести цифрам.
    #[test]
    fn rfc_6238_vectors() {
        let secret = b"12345678901234567890";
        for (seconds, code) in [
            (59, 287_082),
            (1_111_111_109, 81_804),
            (1_111_111_111, 50_471),
            (1_234_567_890, 5_924),
            (2_000_000_000, 279_037),
            (20_000_000_000, 353_130),
        ] {
            assert_eq!(
                code_at(secret, seconds / STEP_SECONDS),
                Some(code),
                "t={seconds}"
            );
        }
    }

    #[test]
    fn neighbouring_steps_are_accepted_once() {
        let secret = b"12345678901234567890";
        let now = 1_111_111_111_000;
        let step = now / 1000 / STEP_SECONDS;
        assert_eq!(matching_step(secret, "050471", now, 0), Some(step));
        assert_eq!(matching_step(secret, " 050471 ", now, 0), Some(step));
        assert_eq!(matching_step(secret, "050471", now + 30_000, 0), Some(step));
        assert_eq!(matching_step(secret, "050471", now + 60_000, 0), None);
        assert_eq!(
            matching_step(secret, "050471", now, step),
            None,
            "повтор кода"
        );
        assert_eq!(matching_step(secret, "50471", now, 0), None);
        assert_eq!(matching_step(secret, "05047a", now, 0), None);
        assert_eq!(matching_step(secret, "000000", now, 0), None);
    }

    #[test]
    fn base32_round_trip() {
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert_eq!(
            base32_encode(b"12345678901234567890"),
            "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"
        );
        assert_eq!(
            base32_decode("mzxw 6ytb-oi==").as_deref(),
            Some(b"foobar".as_slice())
        );
        assert_eq!(base32_decode("1!"), None);
        let secret = new_secret();
        assert_eq!(
            base32_decode(&base32_encode(&secret)).as_deref(),
            Some(secret.as_slice())
        );
    }

    #[test]
    fn stored_secret_needs_the_server_key() {
        let sealed = seal("server-secret", b"totp-secret").unwrap_or_default();
        assert_eq!(
            open("server-secret", &sealed).as_deref(),
            Some(b"totp-secret".as_slice())
        );
        assert_eq!(open("another-secret", &sealed), None);
        assert_ne!(
            seal("server-secret", b"totp-secret"),
            Some(sealed.clone()),
            "каждый раз новый nonce"
        );
        let mut broken = STANDARD.decode(&sealed).unwrap_or_default();
        if let Some(last) = broken.last_mut() {
            *last ^= 1;
        }
        assert_eq!(open("server-secret", &STANDARD.encode(broken)), None);
        assert_eq!(open("server-secret", "AAAA"), None);
    }

    #[test]
    fn recovery_codes_ignore_formatting() {
        let code = new_recovery_code();
        assert_eq!(code.len(), 14);
        assert_eq!(
            recovery_hash(&code),
            recovery_hash(&code.replace('-', " ").to_uppercase())
        );
        assert_ne!(recovery_hash(&code), recovery_hash("aaaa-aaaa-aaaa"));
        assert!(otpauth_url("Мой сервер", "ann", b"12345678901234567890").starts_with(
            "otpauth://totp/%D0%9C%D0%BE%D0%B9%20%D1%81%D0%B5%D1%80%D0%B2%D0%B5%D1%80:ann?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer="
        ));
    }
}
