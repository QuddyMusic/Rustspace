//! Ключи доступа (passkey, WebAuthn): проверка регистрации и входа.
//!
//! Своя небольшая реализация на Rust: разбор CBOR из ответа аутентификатора,
//! ключ COSE и подпись. Подписи проверяет `ring` (он уже есть у сервера
//! через rustls). Аттестация не проверяется (`attestation: "none"`, как у
//! большинства сайтов с passkey): ключу доверяем потому, что регистрирует
//! его вошедший пользователь.
//!
//! Поддерживаются ES256 (−7), EdDSA (−8) и RS256 (−257) — этого хватает
//! Bitwarden, телефонам, Windows Hello и связке ключей Apple.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Алгоритмы ключей, которые просим у аутентификатора (по порядку).
pub const ALGORITHMS: [i64; 3] = [-7, -8, -257];

/// Почему ответ аутентификатора не принят.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PasskeyError {
    #[error("ответ аутентификатора повреждён")]
    Malformed,
    #[error("ответ не для этого запроса")]
    Challenge,
    #[error("ответ не с этого сайта")]
    Origin,
    #[error("пользователь не подтвердил действие")]
    Presence,
    #[error("алгоритм ключа не поддерживается")]
    Algorithm,
    #[error("подпись не сходится")]
    Signature,
}

type Result<T> = std::result::Result<T, PasskeyError>;

// ─── CBOR (только то, что бывает в WebAuthn) ─────────────────────────────

/// Значение CBOR.
#[derive(Debug, Clone, PartialEq)]
enum Cbor {
    Int(i64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Self>),
    Map(Vec<(Self, Self)>),
    Other,
}

impl Cbor {
    fn get(&self, key: &Self) -> Option<&Self> {
        match self {
            Self::Map(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn get_int(&self, key: i64) -> Option<&Self> {
        self.get(&Self::Int(key))
    }

    fn get_text(&self, key: &str) -> Option<&Self> {
        self.get(&Self::Text(key.to_owned()))
    }

    const fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            _ => None,
        }
    }

    fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(b) => Some(b),
            _ => None,
        }
    }
}

/// Разбирает одно значение CBOR; возвращает его и сколько байт занято.
fn cbor(data: &[u8], depth: u8) -> Result<(Cbor, usize)> {
    if depth > 8 {
        return Err(PasskeyError::Malformed);
    }
    let first = *data.first().ok_or(PasskeyError::Malformed)?;
    let (major, info) = (first >> 5, first & 0x1f);
    let (arg, mut used) = match info {
        0..=23 => (u64::from(info), 1),
        24..=27 => {
            let len = 1_usize << (info - 24);
            let bytes = data.get(1..=len).ok_or(PasskeyError::Malformed)?;
            (
                bytes
                    .iter()
                    .fold(0_u64, |acc, b| (acc << 8) | u64::from(*b)),
                1 + len,
            )
        }
        _ => return Err(PasskeyError::Malformed),
    };
    let len = usize::try_from(arg).map_err(|_| PasskeyError::Malformed)?;
    let value = match major {
        0 => Cbor::Int(i64::try_from(arg).map_err(|_| PasskeyError::Malformed)?),
        1 => Cbor::Int(-1 - i64::try_from(arg).map_err(|_| PasskeyError::Malformed)?),
        2 | 3 => {
            let end = used.checked_add(len).ok_or(PasskeyError::Malformed)?;
            let bytes = data.get(used..end).ok_or(PasskeyError::Malformed)?.to_vec();
            used = end;
            if major == 2 {
                Cbor::Bytes(bytes)
            } else {
                Cbor::Text(String::from_utf8(bytes).map_err(|_| PasskeyError::Malformed)?)
            }
        }
        4 => {
            let mut items = Vec::with_capacity(len.min(64));
            for _ in 0..len {
                let (item, n) = cbor(data.get(used..).ok_or(PasskeyError::Malformed)?, depth + 1)?;
                items.push(item);
                used += n;
            }
            Cbor::Array(items)
        }
        5 => {
            let mut pairs = Vec::with_capacity(len.min(64));
            for _ in 0..len {
                let (key, n) = cbor(data.get(used..).ok_or(PasskeyError::Malformed)?, depth + 1)?;
                used += n;
                let (value, n) = cbor(data.get(used..).ok_or(PasskeyError::Malformed)?, depth + 1)?;
                used += n;
                pairs.push((key, value));
            }
            Cbor::Map(pairs)
        }
        // Метки, простые значения (true/false/null).
        6 => {
            let (_, n) = cbor(data.get(used..).ok_or(PasskeyError::Malformed)?, depth + 1)?;
            used += n;
            Cbor::Other
        }
        _ => Cbor::Other,
    };
    Ok((value, used))
}

// ─── Общее ───────────────────────────────────────────────────────────────

fn b64(text: &str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .map_err(|_| PasskeyError::Malformed)
}

/// Проверяет clientDataJSON: тип, вызов и сайт.
fn client_data(raw: &[u8], kind: &str, challenge: &[u8], origin: &str) -> Result<()> {
    let data: Value = serde_json::from_slice(raw).map_err(|_| PasskeyError::Malformed)?;
    if data["type"].as_str() != Some(kind) {
        return Err(PasskeyError::Malformed);
    }
    let got = b64(data["challenge"].as_str().unwrap_or_default())?;
    if got != challenge {
        return Err(PasskeyError::Challenge);
    }
    if data["origin"].as_str().map(|o| o.trim_end_matches('/'))
        != Some(origin.trim_end_matches('/'))
    {
        return Err(PasskeyError::Origin);
    }
    Ok(())
}

/// Данные аутентификатора: хеш сайта, флаги, счётчик и (при регистрации)
/// ключ.
struct AuthData<'a> {
    flags: u8,
    sign_count: u32,
    attested: &'a [u8],
}

fn auth_data<'a>(data: &'a [u8], rp_id: &str) -> Result<AuthData<'a>> {
    if data.len() < 37 {
        return Err(PasskeyError::Malformed);
    }
    if data[..32] != *Sha256::digest(rp_id.as_bytes()) {
        return Err(PasskeyError::Origin);
    }
    let flags = data[32];
    // Пользователь присутствовал (UP).
    if flags & 0x01 == 0 {
        return Err(PasskeyError::Presence);
    }
    let sign_count = u32::from_be_bytes([data[33], data[34], data[35], data[36]]);
    Ok(AuthData {
        flags,
        sign_count,
        attested: &data[37..],
    })
}

/// Открытый ключ в виде, который понимает `ring`: (алгоритм, байты).
fn cose_key(key: &Cbor) -> Result<(i64, Vec<u8>)> {
    let alg = key
        .get_int(3)
        .and_then(Cbor::as_int)
        .ok_or(PasskeyError::Malformed)?;
    match alg {
        // EC2 P-256: точка без сжатия 04 || x || y.
        -7 => {
            let x = key
                .get_int(-2)
                .and_then(Cbor::as_bytes)
                .ok_or(PasskeyError::Malformed)?;
            let y = key
                .get_int(-3)
                .and_then(Cbor::as_bytes)
                .ok_or(PasskeyError::Malformed)?;
            if x.len() != 32 || y.len() != 32 {
                return Err(PasskeyError::Malformed);
            }
            let mut point = vec![0x04];
            point.extend_from_slice(x);
            point.extend_from_slice(y);
            Ok((alg, point))
        }
        // OKP Ed25519.
        -8 => {
            let x = key
                .get_int(-2)
                .and_then(Cbor::as_bytes)
                .ok_or(PasskeyError::Malformed)?;
            if x.len() != 32 {
                return Err(PasskeyError::Malformed);
            }
            Ok((alg, x.to_vec()))
        }
        // RSA: DER RSAPublicKey (SEQUENCE { n, e }).
        -257 => {
            let n = key
                .get_int(-1)
                .and_then(Cbor::as_bytes)
                .ok_or(PasskeyError::Malformed)?;
            let e = key
                .get_int(-2)
                .and_then(Cbor::as_bytes)
                .ok_or(PasskeyError::Malformed)?;
            Ok((alg, rsa_der(n, e)))
        }
        _ => Err(PasskeyError::Algorithm),
    }
}

/// DER-длина.
fn der_len(len: usize, out: &mut Vec<u8>) {
    if len < 0x80 {
        out.push(u8::try_from(len).unwrap_or(0));
    } else {
        let bytes: Vec<u8> = len
            .to_be_bytes()
            .into_iter()
            .skip_while(|b| *b == 0)
            .collect();
        out.push(0x80 | u8::try_from(bytes.len()).unwrap_or(0));
        out.extend(bytes);
    }
}

/// DER INTEGER (без знака: при старшем бите — ведущий ноль).
fn der_uint(value: &[u8], out: &mut Vec<u8>) {
    let trimmed: Vec<u8> = value.iter().copied().skip_while(|b| *b == 0).collect();
    let pad = trimmed.first().is_some_and(|b| b & 0x80 != 0);
    out.push(0x02);
    der_len(trimmed.len() + usize::from(pad), out);
    if pad {
        out.push(0);
    }
    out.extend(trimmed);
}

fn rsa_der(n: &[u8], e: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    der_uint(n, &mut body);
    der_uint(e, &mut body);
    let mut out = vec![0x30];
    der_len(body.len(), &mut out);
    out.extend(body);
    out
}

// ─── Регистрация ─────────────────────────────────────────────────────────

/// Новый ключ доступа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPasskey {
    /// Id ключа (base64url).
    pub id: String,
    /// Алгоритм COSE.
    pub alg: i64,
    /// Открытый ключ (base64url, вид для `ring`).
    pub public_key: String,
    /// Счётчик подписей.
    pub sign_count: u32,
}

/// Проверяет ответ `navigator.credentials.create()`.
///
/// `credential` — JSON с полями `rawId`, `response.clientDataJSON`,
/// `response.attestationObject` (base64url).
///
/// # Errors
/// Ответ повреждён, не для этого вызова или сайта, ключ не поддерживается.
pub fn verify_registration(
    credential: &Value,
    challenge: &[u8],
    origin: &str,
    rp_id: &str,
) -> Result<NewPasskey> {
    let response = &credential["response"];
    let client = b64(response["clientDataJSON"].as_str().unwrap_or_default())?;
    client_data(&client, "webauthn.create", challenge, origin)?;
    let attestation = b64(response["attestationObject"].as_str().unwrap_or_default())?;
    let (object, _) = cbor(&attestation, 0)?;
    let auth = object
        .get_text("authData")
        .and_then(Cbor::as_bytes)
        .ok_or(PasskeyError::Malformed)?;
    let parsed = auth_data(auth, rp_id)?;
    // Есть данные ключа (AT).
    if parsed.flags & 0x40 == 0 || parsed.attested.len() < 18 {
        return Err(PasskeyError::Malformed);
    }
    let id_len = usize::from(u16::from_be_bytes([
        parsed.attested[16],
        parsed.attested[17],
    ]));
    let id = parsed
        .attested
        .get(18..18 + id_len)
        .ok_or(PasskeyError::Malformed)?;
    let (key, _) = cbor(
        parsed
            .attested
            .get(18 + id_len..)
            .ok_or(PasskeyError::Malformed)?,
        0,
    )?;
    let (alg, public) = cose_key(&key)?;
    let raw_id = b64(credential["rawId"].as_str().unwrap_or_default())?;
    if raw_id != id {
        return Err(PasskeyError::Malformed);
    }
    Ok(NewPasskey {
        id: URL_SAFE_NO_PAD.encode(id),
        alg,
        public_key: URL_SAFE_NO_PAD.encode(public),
        sign_count: parsed.sign_count,
    })
}

// ─── Вход ────────────────────────────────────────────────────────────────

/// Что известно о ключе, которым входят.
#[derive(Debug, Clone)]
pub struct StoredPasskey {
    /// Алгоритм COSE.
    pub alg: i64,
    /// Открытый ключ (base64url).
    pub public_key: String,
    /// Последний счётчик подписей.
    pub sign_count: u32,
}

/// Проверяет ответ `navigator.credentials.get()`; возвращает новый счётчик.
///
/// # Errors
/// Ответ повреждён, не для этого вызова или сайта, подпись не сходится или
/// счётчик пошёл назад (копия ключа).
pub fn verify_assertion(
    credential: &Value,
    stored: &StoredPasskey,
    challenge: &[u8],
    origin: &str,
    rp_id: &str,
) -> Result<u32> {
    let response = &credential["response"];
    let client = b64(response["clientDataJSON"].as_str().unwrap_or_default())?;
    client_data(&client, "webauthn.get", challenge, origin)?;
    let auth = b64(response["authenticatorData"].as_str().unwrap_or_default())?;
    let parsed = auth_data(&auth, rp_id)?;
    let signature = b64(response["signature"].as_str().unwrap_or_default())?;
    let mut signed = auth.clone();
    signed.extend_from_slice(&Sha256::digest(&client));
    let key = b64(&stored.public_key)?;
    let algorithm: &dyn ring::signature::VerificationAlgorithm = match stored.alg {
        -7 => &ring::signature::ECDSA_P256_SHA256_ASN1,
        -8 => &ring::signature::ED25519,
        -257 => &ring::signature::RSA_PKCS1_2048_8192_SHA256,
        _ => return Err(PasskeyError::Algorithm),
    };
    ring::signature::UnparsedPublicKey::new(algorithm, &key)
        .verify(&signed, &signature)
        .map_err(|_| PasskeyError::Signature)?;
    // Счётчик: если ключ его ведёт, он только растёт.
    if parsed.sign_count != 0 && parsed.sign_count <= stored.sign_count {
        return Err(PasskeyError::Signature);
    }
    Ok(parsed.sign_count)
}

/// Id ключа из ответа на вход (`rawId`, base64url без дополнения).
#[must_use]
pub fn assertion_id(credential: &Value) -> Option<String> {
    let raw = b64(credential["rawId"].as_str()?).ok()?;
    Some(URL_SAFE_NO_PAD.encode(raw))
}

#[cfg(test)]
#[allow(clippy::panic, reason = "в тестах паника допустима")]
mod tests {
    use super::*;

    #[test]
    fn cbor_reads_webauthn_shapes() {
        // {1: 2, 3: -7, -1: 1, -2: h'0102', "fmt": "none"}
        let data = [
            0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21, 0x42, 0x01, 0x02, 0x63, b'f', b'm',
            b't', 0x64, b'n', b'o', b'n', b'e',
        ];
        let Ok((value, used)) = cbor(&data, 0) else {
            panic!("CBOR не разобран");
        };
        assert_eq!(used, data.len());
        assert_eq!(value.get_int(3).and_then(Cbor::as_int), Some(-7));
        assert_eq!(
            value.get_int(-2).and_then(Cbor::as_bytes),
            Some(&[1_u8, 2][..])
        );
        assert_eq!(value.get_text("fmt"), Some(&Cbor::Text("none".into())));
        assert!(cbor(&[0x5f], 0).is_err());
        assert!(cbor(&[0x42, 0x01], 0).is_err());
    }

    #[test]
    fn rsa_key_is_der() {
        let der = rsa_der(&[0x80, 0x01], &[0x01, 0x00, 0x01]);
        assert_eq!(
            der,
            vec![
                0x30, 0x0a, 0x02, 0x03, 0x00, 0x80, 0x01, 0x02, 0x03, 0x01, 0x00, 0x01
            ]
        );
    }

    /// Полный круг: ключ Ed25519, собранные вручную регистрация и вход.
    #[test]
    fn ed25519_register_and_sign_in() {
        use ring::signature::KeyPair;
        let rng = ring::rand::SystemRandom::new();
        let Ok(pkcs8) = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng) else {
            panic!("ключ не создан");
        };
        let Ok(pair) = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()) else {
            panic!("ключ не прочитан");
        };
        let (origin, rp_id, challenge) = ("https://chat.example", "chat.example", [7_u8; 32]);
        let cred_id = [9_u8; 16];
        // COSE: {1: 1 (OKP), 3: -8, -1: 6, -2: x}
        let mut cose = vec![0xa4, 0x01, 0x01, 0x03, 0x27, 0x20, 0x06, 0x21, 0x58, 0x20];
        cose.extend_from_slice(pair.public_key().as_ref());
        let mut auth = Sha256::digest(rp_id.as_bytes()).to_vec();
        auth.push(0x45); // UP + UV + AT
        auth.extend_from_slice(&0_u32.to_be_bytes());
        auth.extend_from_slice(&[0; 16]);
        auth.extend_from_slice(&16_u16.to_be_bytes());
        auth.extend_from_slice(&cred_id);
        auth.extend_from_slice(&cose);
        // {"fmt": "none", "attStmt": {}, "authData": auth}
        let mut att = vec![0xa3, 0x63, b'f', b'm', b't', 0x64, b'n', b'o', b'n', b'e'];
        att.extend_from_slice(&[0x67, b'a', b't', b't', b'S', b't', b'm', b't', 0xa0]);
        att.extend_from_slice(&[0x68, b'a', b'u', b't', b'h', b'D', b'a', b't', b'a', 0x58]);
        att.push(u8::try_from(auth.len()).unwrap_or(0));
        att.extend_from_slice(&auth);
        let client = |kind: &str| {
            serde_json::json!({
                "type": kind,
                "challenge": URL_SAFE_NO_PAD.encode(challenge),
                "origin": origin,
            })
            .to_string()
        };
        let created = serde_json::json!({
            "rawId": URL_SAFE_NO_PAD.encode(cred_id),
            "response": {
                "clientDataJSON": URL_SAFE_NO_PAD.encode(client("webauthn.create")),
                "attestationObject": URL_SAFE_NO_PAD.encode(&att),
            }
        });
        let Ok(new) = verify_registration(&created, &challenge, origin, rp_id) else {
            panic!("регистрация не прошла");
        };
        assert_eq!(new.alg, -8);
        assert!(verify_registration(&created, &[1; 32], origin, rp_id).is_err());
        assert!(verify_registration(&created, &challenge, "https://evil.example", rp_id).is_err());

        let mut login_auth = Sha256::digest(rp_id.as_bytes()).to_vec();
        login_auth.push(0x05);
        login_auth.extend_from_slice(&1_u32.to_be_bytes());
        let client_json = client("webauthn.get");
        let mut signed = login_auth.clone();
        signed.extend_from_slice(&Sha256::digest(client_json.as_bytes()));
        let signature = pair.sign(&signed);
        let assertion = serde_json::json!({
            "rawId": URL_SAFE_NO_PAD.encode(cred_id),
            "response": {
                "clientDataJSON": URL_SAFE_NO_PAD.encode(&client_json),
                "authenticatorData": URL_SAFE_NO_PAD.encode(&login_auth),
                "signature": URL_SAFE_NO_PAD.encode(signature.as_ref()),
            }
        });
        let stored = StoredPasskey {
            alg: new.alg,
            public_key: new.public_key,
            sign_count: 0,
        };
        assert_eq!(
            verify_assertion(&assertion, &stored, &challenge, origin, rp_id),
            Ok(1)
        );
        let replayed = StoredPasskey {
            sign_count: 1,
            ..stored
        };
        assert!(verify_assertion(&assertion, &replayed, &challenge, origin, rp_id).is_err());
        assert_eq!(
            assertion_id(&assertion),
            Some(URL_SAFE_NO_PAD.encode(cred_id))
        );
    }
}
