//! Сквозное шифрование личных сообщений.
//!
//! Шифрует клиент, расшифровывает клиент собеседника; сервер видит только
//! строку `bs-e2ee:1:…` в обычном поле текста сообщения. Поэтому переписка
//! проходит через любой сервер, в том числе оригинальный и федеративный:
//! они хранят и пересылают её как обычный текст, а клиент без поддержки
//! шифрования просто покажет эту строку.
//!
//! Схема (версия 1):
//! - у каждого пользователя пара ключей X25519;
//! - общий ключ пары собеседников: `HKDF-SHA256(X25519(свой секретный, чужой
//!   открытый))`, в `info` — оба открытых ключа по порядку. Он одинаков у
//!   обоих, так что отправитель читает и свои сообщения из истории;
//! - сообщение: XChaCha20-Poly1305 со случайным 192-битным nonce; открытые
//!   ключи обоих входят в подтверждаемые данные.
//!
//! Чего схема не даёт. Ключи долговременные: тот, кто получит секретный ключ,
//! прочтёт и прошлую переписку (прямой секретности нет). Сервер по-прежнему
//! видит, кто, кому и когда пишет, и длину сообщений; может не доставить,
//! повторить или переставить сообщения. Подмену открытого ключа сервером
//! обнаруживает только сверка контрольного числа ([`safety_number`]) по
//! другому каналу.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Начало зашифрованного сообщения.
pub const SEALED_PREFIX: &str = "bs-e2ee:1:";
/// Начало сообщения с открытым ключом (обмен ключами прямо в переписке).
pub const KEY_PREFIX: &str = "bs-e2ee:key:1:";
/// Сколько байт открытого текста помещается в сообщение: зашифрованная
/// строка должна уложиться в предел сервера (4000 знаков).
pub const MAX_PLAINTEXT_BYTES: usize = 2900;
const NONCE_BYTES: usize = 24;

/// Почему сообщение не открылось.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    /// Это не зашифрованное сообщение этой версии.
    #[error("не зашифрованное сообщение")]
    NotSealed,
    /// Строка повреждена.
    #[error("сообщение повреждено")]
    Malformed,
    /// Ключ не подходит или сообщение изменено по дороге.
    #[error("не удалось расшифровать: другой ключ или сообщение изменено")]
    Rejected,
}

/// Почему сообщение не зашифровалось.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SealError {
    /// Текст длиннее [`MAX_PLAINTEXT_BYTES`].
    #[error("сообщение слишком длинное для шифрования")]
    TooLong,
    /// Открытый ключ собеседника негоден (точка малого порядка).
    #[error("открытый ключ собеседника негоден")]
    BadPeerKey,
}

/// Начало переносимой записи ключа, защищённой паролем.
pub const EXPORT_PREFIX: &str = "bs-e2ee-key:1:";
/// Самый короткий пароль для переноса ключа.
pub const MIN_PASSPHRASE_CHARS: usize = 8;
const SALT_BYTES: usize = 16;

/// Почему ключ не выгрузился или не загрузился.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransferError {
    /// Пароль короче [`MIN_PASSPHRASE_CHARS`].
    #[error("пароль слишком короткий")]
    WeakPassphrase,
    /// Это не запись ключа этой версии или она повреждена.
    #[error("запись ключа повреждена")]
    Malformed,
    /// Пароль не подходит или запись изменена.
    #[error("пароль не подходит")]
    Rejected,
}

/// Ключ шифрования записи из пароля: Argon2id (19 МиБ, 2 прохода).
fn passphrase_key(
    passphrase: &str,
    salt: &[u8],
) -> Result<zeroize::Zeroizing<[u8; 32]>, TransferError> {
    let params =
        argon2::Params::new(19 * 1024, 2, 1, Some(32)).map_err(|_| TransferError::Malformed)?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = zeroize::Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, key.as_mut())
        .map_err(|_| TransferError::Malformed)?;
    Ok(key)
}

/// Запись секретного ключа для переноса на другое устройство, защищённая
/// паролем: `bs-e2ee-key:1:` и base64url(соль, nonce, шифр).
///
/// # Errors
/// Пароль слишком короткий.
pub fn export_key(key: &SecretKey, passphrase: &str) -> Result<String, TransferError> {
    if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(TransferError::WeakPassphrase);
    }
    let salt: [u8; SALT_BYTES] = rand::random();
    let nonce: [u8; NONCE_BYTES] = rand::random();
    let wrapping = passphrase_key(passphrase, &salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(wrapping.as_ref())
        .map_err(|_| TransferError::Malformed)?;
    let sealed = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: &key.0,
                aad: EXPORT_PREFIX.as_bytes(),
            },
        )
        .map_err(|_| TransferError::Malformed)?;
    let mut packet = Vec::with_capacity(SALT_BYTES + NONCE_BYTES + sealed.len());
    packet.extend_from_slice(&salt);
    packet.extend_from_slice(&nonce);
    packet.extend_from_slice(&sealed);
    Ok(format!("{EXPORT_PREFIX}{}", URL_SAFE_NO_PAD.encode(packet)))
}

/// Секретный ключ из записи [`export_key`].
///
/// # Errors
/// Запись повреждена или пароль не подходит.
pub fn import_key(text: &str, passphrase: &str) -> Result<SecretKey, TransferError> {
    let body = text
        .trim()
        .strip_prefix(EXPORT_PREFIX)
        .ok_or(TransferError::Malformed)?;
    let packet = URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|_| TransferError::Malformed)?;
    let (salt, rest) = packet
        .split_at_checked(SALT_BYTES)
        .ok_or(TransferError::Malformed)?;
    let (nonce, sealed) = rest
        .split_at_checked(NONCE_BYTES)
        .ok_or(TransferError::Malformed)?;
    let wrapping = passphrase_key(passphrase, salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(wrapping.as_ref())
        .map_err(|_| TransferError::Malformed)?;
    let mut plain = cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: sealed,
                aad: EXPORT_PREFIX.as_bytes(),
            },
        )
        .map_err(|_| TransferError::Rejected)?;
    let key = <[u8; 32]>::try_from(plain.as_slice())
        .map(SecretKey)
        .map_err(|_| TransferError::Malformed);
    plain.zeroize();
    key
}

/// Открытый ключ пользователя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    /// Запись для передачи: base64url без дополнения (43 знака).
    #[must_use]
    pub fn to_wire(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }

    /// Разбор записи [`PublicKey::to_wire`].
    #[must_use]
    pub fn from_wire(text: &str) -> Option<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(text.trim()).ok()?;
        Some(Self(bytes.try_into().ok()?))
    }

    /// Байты ключа.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Секретный ключ пользователя. Затирается в памяти при уничтожении; в
/// отладочный вывод не попадает.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SecretKey([u8; 32]);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(<redacted>)")
    }
}

impl SecretKey {
    /// Новый случайный ключ.
    #[must_use]
    pub fn generate() -> Self {
        Self(rand::random())
    }

    /// Открытый ключ пары.
    #[must_use]
    pub fn public(&self) -> PublicKey {
        PublicKey(
            x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(self.0)).to_bytes(),
        )
    }

    /// Запись для хранения и переноса на другое устройство (base64).
    /// Это секрет: показывать её можно только владельцу.
    #[must_use]
    pub fn to_backup(&self) -> zeroize::Zeroizing<String> {
        zeroize::Zeroizing::new(STANDARD.encode(self.0))
    }

    /// Разбор записи [`SecretKey::to_backup`].
    #[must_use]
    pub fn from_backup(text: &str) -> Option<Self> {
        let mut bytes = STANDARD.decode(text.trim()).ok()?;
        let key = <[u8; 32]>::try_from(bytes.as_slice()).ok().map(Self);
        bytes.zeroize();
        key
    }
}

/// Оба открытых ключа в одном порядке для обоих собеседников.
fn ordered(a: &PublicKey, b: &PublicKey) -> [u8; 64] {
    let (first, second) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    let mut out = [0u8; 64];
    out[..32].copy_from_slice(&first.0);
    out[32..].copy_from_slice(&second.0);
    out
}

fn cipher(mine: &SecretKey, peer: &PublicKey) -> Option<(XChaCha20Poly1305, [u8; 64])> {
    let shared = x25519_dalek::StaticSecret::from(mine.0)
        .diffie_hellman(&x25519_dalek::PublicKey::from(peer.0));
    // Точка малого порядка дала бы общий «секрет», известный всем.
    if !shared.was_contributory() {
        return None;
    }
    let pair = ordered(&mine.public(), peer);
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(b"bs-e2ee v1"), shared.as_bytes())
        .expand(&pair, &mut key)
        .ok()?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key).ok();
    key.zeroize();
    Some((cipher?, pair))
}

/// Зашифровано ли сообщение этой схемой.
#[must_use]
pub fn is_sealed(content: &str) -> bool {
    content.starts_with(SEALED_PREFIX)
}

/// Шифрует текст для собеседника. Результат кладётся в поле текста сообщения.
///
/// # Errors
/// Текст слишком длинный или ключ собеседника негоден.
pub fn seal(mine: &SecretKey, peer: &PublicKey, plaintext: &str) -> Result<String, SealError> {
    if plaintext.len() > MAX_PLAINTEXT_BYTES {
        return Err(SealError::TooLong);
    }
    let (cipher, pair) = cipher(mine, peer).ok_or(SealError::BadPeerKey)?;
    let nonce: [u8; NONCE_BYTES] = rand::random();
    let sealed = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext.as_bytes(),
                aad: &pair,
            },
        )
        .map_err(|_| SealError::TooLong)?;
    let mut packet = Vec::with_capacity(NONCE_BYTES + sealed.len());
    packet.extend_from_slice(&nonce);
    packet.extend_from_slice(&sealed);
    Ok(format!("{SEALED_PREFIX}{}", URL_SAFE_NO_PAD.encode(packet)))
}

/// Расшифровывает сообщение собеседника или своё собственное из истории.
///
/// # Errors
/// Строка не зашифрована этой схемой, повреждена или не открывается ключом.
pub fn open(mine: &SecretKey, peer: &PublicKey, content: &str) -> Result<String, OpenError> {
    let body = content
        .strip_prefix(SEALED_PREFIX)
        .ok_or(OpenError::NotSealed)?;
    let packet = URL_SAFE_NO_PAD
        .decode(body.trim())
        .map_err(|_| OpenError::Malformed)?;
    let (nonce, sealed) = packet
        .split_at_checked(NONCE_BYTES)
        .ok_or(OpenError::Malformed)?;
    let (cipher, pair) = cipher(mine, peer).ok_or(OpenError::Rejected)?;
    let plain = cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: sealed,
                aad: &pair,
            },
        )
        .map_err(|_| OpenError::Rejected)?;
    String::from_utf8(plain).map_err(|_| OpenError::Malformed)
}

/// Сообщение, которым клиент сообщает собеседнику свой открытый ключ, когда
/// у сервера нет справочника ключей.
#[must_use]
pub fn key_announcement(key: &PublicKey) -> String {
    format!("{KEY_PREFIX}{}", key.to_wire())
}

/// Открытый ключ из сообщения [`key_announcement`].
#[must_use]
pub fn parse_key_announcement(content: &str) -> Option<PublicKey> {
    PublicKey::from_wire(content.strip_prefix(KEY_PREFIX)?)
}

/// Контрольное число пары ключей: шесть групп по пять цифр, одинаковые у
/// обоих собеседников. Его сверяют голосом или при встрече: совпало —
/// между собеседниками нет подменённого ключа.
#[must_use]
pub fn safety_number(a: &PublicKey, b: &PublicKey) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"bs-e2ee safety number v1");
    hasher.update(ordered(a, b));
    let digest = hasher.finalize();
    digest
        .chunks(5)
        .take(6)
        .map(|chunk| {
            let value = chunk
                .iter()
                .fold(0u64, |sum, byte| (sum << 8) | u64::from(*byte));
            format!("{:05}", value % 100_000)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    fn pair() -> (SecretKey, SecretKey) {
        (SecretKey::generate(), SecretKey::generate())
    }

    #[test]
    fn both_sides_read_the_message() {
        let (ann, bob) = pair();
        let sealed = seal(&ann, &bob.public(), "привет, Боб 🔒").unwrap_or_default();
        assert!(is_sealed(&sealed) && !sealed.contains("привет"));
        assert_eq!(
            open(&bob, &ann.public(), &sealed).as_deref(),
            Ok("привет, Боб 🔒")
        );
        // Отправитель читает своё сообщение из истории тем же ключом.
        assert_eq!(
            open(&ann, &bob.public(), &sealed).as_deref(),
            Ok("привет, Боб 🔒")
        );
        assert_ne!(
            seal(&ann, &bob.public(), "привет, Боб 🔒").unwrap_or_default(),
            sealed,
            "каждый раз новый nonce"
        );
    }

    #[test]
    fn outsiders_and_tampering_are_rejected() {
        let (ann, bob) = pair();
        let eve = SecretKey::generate();
        let sealed = seal(&ann, &bob.public(), "секрет").unwrap_or_default();
        assert_eq!(open(&eve, &ann.public(), &sealed), Err(OpenError::Rejected));
        assert_eq!(open(&bob, &eve.public(), &sealed), Err(OpenError::Rejected));
        let mut bytes = URL_SAFE_NO_PAD
            .decode(&sealed[SEALED_PREFIX.len()..])
            .unwrap_or_default();
        for at in [0, NONCE_BYTES, bytes.len() - 1] {
            bytes[at] ^= 1;
            let changed = format!("{SEALED_PREFIX}{}", URL_SAFE_NO_PAD.encode(&bytes));
            assert_eq!(
                open(&bob, &ann.public(), &changed),
                Err(OpenError::Rejected),
                "байт {at}"
            );
            bytes[at] ^= 1;
        }
        assert_eq!(
            open(&bob, &ann.public(), "обычный текст"),
            Err(OpenError::NotSealed)
        );
        assert_eq!(
            open(&bob, &ann.public(), "bs-e2ee:1:%%%"),
            Err(OpenError::Malformed)
        );
        assert_eq!(
            open(&bob, &ann.public(), "bs-e2ee:1:AAAA"),
            Err(OpenError::Malformed)
        );
    }

    #[test]
    fn weak_peer_keys_and_long_texts_are_refused() {
        let ann = SecretKey::generate();
        let zero = PublicKey([0; 32]);
        assert_eq!(seal(&ann, &zero, "x"), Err(SealError::BadPeerKey));
        let bob = SecretKey::generate();
        let long = "я".repeat(MAX_PLAINTEXT_BYTES / 2 + 1);
        assert_eq!(seal(&ann, &bob.public(), &long), Err(SealError::TooLong));
        let longest = "a".repeat(MAX_PLAINTEXT_BYTES);
        let sealed = seal(&ann, &bob.public(), &longest).unwrap_or_default();
        assert!(sealed.len() <= 4000, "{}", sealed.len());
    }

    #[test]
    fn keys_travel_as_text() {
        let key = SecretKey::generate();
        let public = key.public();
        assert_eq!(public.to_wire().len(), 43);
        assert_eq!(PublicKey::from_wire(&public.to_wire()), Some(public));
        assert_eq!(PublicKey::from_wire("коротко"), None);
        assert_eq!(
            parse_key_announcement(&key_announcement(&public)),
            Some(public)
        );
        assert_eq!(parse_key_announcement("bs-e2ee:1:abc"), None);
        let restored = SecretKey::from_backup(&key.to_backup()).map(|key| key.public());
        assert_eq!(restored, Some(public));
        assert!(SecretKey::from_backup("AAAA").is_none());
        assert_eq!(format!("{key:?}"), "SecretKey(<redacted>)");
    }

    #[test]
    fn safety_number_is_shared_and_key_specific() {
        let (ann, bob) = pair();
        let number = safety_number(&ann.public(), &bob.public());
        assert_eq!(number, safety_number(&bob.public(), &ann.public()));
        assert_eq!(number.len(), 35);
        assert!(
            number
                .split(' ')
                .all(|group| group.len() == 5 && group.bytes().all(|b| b.is_ascii_digit()))
        );
        assert_ne!(
            number,
            safety_number(&ann.public(), &SecretKey::generate().public())
        );
    }

    /// Известный ответ X25519 из RFC 7748 (раздел 6.1) проверяет, что открытый
    /// ключ считается стандартным образом и другой клиент получит тот же.
    #[test]
    fn public_key_matches_rfc_7748() {
        let hex = |text: &str| -> [u8; 32] {
            let mut out = [0u8; 32];
            for (index, byte) in out.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap_or_default();
            }
            out
        };
        let alice = SecretKey(hex(
            "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a",
        ));
        assert_eq!(
            alice.public().0,
            hex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a")
        );
    }

    #[test]
    fn key_transfer_with_passphrase() {
        let key = SecretKey::generate();
        let text = export_key(&key, "долгий пароль").expect("выгрузка");
        assert!(text.starts_with(EXPORT_PREFIX));
        let back = import_key(&text, "долгий пароль").expect("загрузка");
        assert_eq!(back.public(), key.public());
        assert_eq!(
            import_key(&text, "не тот пароль").err(),
            Some(TransferError::Rejected)
        );
        assert_eq!(
            export_key(&key, "коротко").err(),
            Some(TransferError::WeakPassphrase)
        );
        assert_eq!(
            import_key("bs-e2ee-key:1:AAAA", "долгий пароль").err(),
            Some(TransferError::Malformed)
        );
        assert_eq!(
            import_key(&key.to_backup(), "долгий пароль").err(),
            Some(TransferError::Malformed)
        );
        // Каждая выгрузка — со своей солью.
        assert_ne!(text, export_key(&key, "долгий пароль").expect("выгрузка"));
    }
}
