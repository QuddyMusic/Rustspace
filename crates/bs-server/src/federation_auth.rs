//! Подписи запросов между инстансами (`utils/federationAuth.ts`).
//!
//! Первая часть переноса федерации: сами подписи. Запрос подписывается общим
//! секретом пары инстансов (HMAC-SHA256 над временем, одноразовым числом и
//! телом); принимающая сторона сверяет подпись, давность и — при смене
//! секрета — в течение четверти часа принимает оба.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

/// Наибольшая давность подписанного запроса.
pub const MAX_AGE_MS: i64 = 15 * 60 * 1000;
/// Сколько после смены секрета принимается и новый, ещё не подтверждённый.
pub const ROTATION_GRACE_MS: i64 = 15 * 60 * 1000;
/// Наибольшая длина одноразового числа.
const MAX_NONCE_LENGTH: usize = 64;

/// Новый общий секрет: 32 случайных байта в шестнадцатеричной записи.
#[must_use]
pub fn generate_secret() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

/// Подпись запроса (`signRequest`): шестнадцатеричный HMAC-SHA256.
#[must_use]
pub fn sign(body: &str, secret: &str, timestamp: i64, nonce: Option<&str>) -> String {
    let payload = match nonce.filter(|nonce| !nonce.is_empty()) {
        Some(nonce) => format!("{timestamp}.{nonce}.{body}"),
        None => format!("{timestamp}.{body}"),
    };
    // HMAC принимает ключ любой длины.
    let Ok(mut mac) = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()) else {
        return String::new();
    };
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Проверка подписи (`verifySignature`): давность и сама подпись, сравнение
/// за постоянное время.
#[must_use]
pub fn verify(
    body: &str,
    signature: &str,
    secret: &str,
    timestamp: i64,
    nonce: Option<&str>,
    now_ms: i64,
) -> bool {
    if signature.is_empty() || secret.is_empty() || (now_ms - timestamp).abs() > MAX_AGE_MS {
        return false;
    }
    let (Ok(expected), Ok(actual)) = (
        hex::decode(sign(body, secret, timestamp, nonce)),
        hex::decode(signature),
    ) else {
        return false;
    };
    expected.len() == actual.len() && bool::from(expected.ct_eq(&actual))
}

/// Секреты пира для проверки подписи.
#[derive(Debug, Clone, Copy)]
pub struct PeerSecrets<'a> {
    /// Действующий секрет.
    pub current: &'a str,
    /// Новый секрет на время смены.
    pub pending: Option<&'a str>,
    /// Когда началась смена.
    pub rotation_at: Option<i64>,
}

/// Проверка подписи пира с учётом смены секрета (`verifyPeerSignature`).
#[must_use]
pub fn verify_peer(
    body: &str,
    signature: &str,
    timestamp: i64,
    nonce: Option<&str>,
    peer: PeerSecrets<'_>,
    now_ms: i64,
) -> bool {
    if verify(body, signature, peer.current, timestamp, nonce, now_ms) {
        return true;
    }
    match (
        peer.pending.filter(|secret| !secret.is_empty()),
        peer.rotation_at.filter(|at| *at != 0),
    ) {
        (Some(pending), Some(started)) if now_ms - started <= ROTATION_GRACE_MS => {
            verify(body, signature, pending, timestamp, nonce, now_ms)
        }
        _ => false,
    }
}

/// Заголовки подписанного запроса (`buildFederationHeaders`).
#[must_use]
pub fn headers(body: &str, secret: &str, origin: &str, now_ms: i64) -> [(&'static str, String); 5] {
    let nonce = uuid::Uuid::new_v4().to_string();
    let signature = sign(body, secret, now_ms, Some(&nonce));
    [
        ("X-Federation-Signature", format!("sha256={signature}")),
        ("X-Federation-Origin", origin.to_owned()),
        ("X-Federation-Timestamp", now_ms.to_string()),
        ("X-Federation-Nonce", nonce),
        ("Content-Type", "application/json".to_owned()),
    ]
}

/// Разобранные заголовки подписи.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    /// Кто прислал.
    pub origin: String,
    /// Время подписи, мс.
    pub timestamp: i64,
    /// Подпись в шестнадцатеричной записи.
    pub signature: String,
    /// Одноразовое число.
    pub nonce: Option<String>,
}

/// Разбор заголовков подписи (`parseFederationHeaders`). `get` возвращает
/// значение заголовка по имени в нижнем регистре.
#[must_use]
pub fn parse_headers<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Option<Signed> {
    let signature = get("x-federation-signature")?.strip_prefix("sha256=")?;
    if signature.is_empty() || !signature.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let origin = get("x-federation-origin")?.trim();
    // `Number(text)`: целые миллисекунды; дробные и нечисловые значения не принимаются.
    let timestamp: f64 = get("x-federation-timestamp")?.trim().parse().ok()?;
    if origin.is_empty() || !timestamp.is_finite() || timestamp <= 0.0 || timestamp > 9.0e15 {
        return None;
    }
    let nonce = get("x-federation-nonce")
        .map(str::trim)
        .filter(|nonce| !nonce.is_empty());
    if nonce.is_some_and(|nonce| nonce.len() > MAX_NONCE_LENGTH) {
        return None;
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "значение проверено и помещается в i64"
    )]
    let timestamp = timestamp as i64;
    Some(Signed {
        origin: origin.to_owned(),
        timestamp,
        signature: signature.to_owned(),
        nonce: nonce.map(str::to_owned),
    })
}

/// Адрес без косых черт на конце (`stripTrailingSlashes`).
#[must_use]
pub fn strip_trailing_slashes(text: &str) -> &str {
    text.trim_end_matches('/')
}

fn strip_scheme(text: &str) -> Option<&str> {
    let lower = text.get(..8).unwrap_or(text).to_ascii_lowercase();
    if lower.starts_with("https://") {
        text.get(8..)
    } else if lower.starts_with("http://") {
        text.get(7..)
    } else {
        None
    }
}

/// Адрес инстанса для сравнения: без схемы, без косой черты на конце, в
/// нижнем регистре (`normalizeOriginForCompare`).
#[must_use]
pub fn normalize_origin(value: Option<&str>) -> Option<String> {
    let text = value?.trim();
    let bare = strip_trailing_slashes(strip_scheme(text).unwrap_or(text));
    (!bare.is_empty()).then(|| bare.to_lowercase())
}

/// Каноническая запись домашнего инстанса: со схемой, без косой черты на
/// конце (`canonicalizeHomeInstance`).
#[must_use]
pub fn canonical_home_instance(value: Option<&str>) -> Option<String> {
    let text = value?.trim();
    if text.is_empty() {
        return None;
    }
    Some(if strip_scheme(text).is_some() {
        strip_trailing_slashes(text).to_owned()
    } else {
        format!("https://{}", strip_trailing_slashes(text))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";
    const AT: i64 = 1_700_000_000_000;

    /// Известные ответы посчитаны независимой реализацией HMAC-SHA256: такую
    /// же подпись выдаёт и проверяет оригинальный сервер.
    #[test]
    fn signature_matches_the_original() {
        assert_eq!(
            sign(r#"{"a":1}"#, SECRET, AT, Some("nonce-1")),
            VECTOR_WITH_NONCE
        );
        assert_eq!(sign(r#"{"a":1}"#, SECRET, AT, None), VECTOR_WITHOUT_NONCE);
        assert_eq!(
            sign(r#"{"a":1}"#, SECRET, AT, Some("")),
            VECTOR_WITHOUT_NONCE
        );
    }

    #[test]
    fn stale_altered_and_foreign_requests_are_rejected() {
        let body = r#"{"a":1}"#;
        let good = sign(body, SECRET, AT, Some("n"));
        assert!(verify(body, &good, SECRET, AT, Some("n"), AT + 1000));
        assert!(verify(
            body,
            &good.to_uppercase(),
            SECRET,
            AT,
            Some("n"),
            AT
        ));
        assert!(
            !verify(body, &good, SECRET, AT, Some("n"), AT + MAX_AGE_MS + 1),
            "устарел"
        );
        assert!(
            !verify(body, &good, SECRET, AT, Some("n"), AT - MAX_AGE_MS - 1),
            "из будущего"
        );
        assert!(
            !verify(r#"{"a":2}"#, &good, SECRET, AT, Some("n"), AT),
            "тело изменено"
        );
        assert!(
            !verify(body, &good, SECRET, AT, Some("m"), AT),
            "другое одноразовое число"
        );
        assert!(
            !verify(body, &good, SECRET, AT + 1, Some("n"), AT),
            "другое время"
        );
        assert!(!verify(body, &good, "another-secret", AT, Some("n"), AT));
        assert!(!verify(body, "", SECRET, AT, Some("n"), AT));
        assert!(!verify(body, "zz", SECRET, AT, Some("n"), AT));
        assert!(!verify(body, &good[..10], SECRET, AT, Some("n"), AT));
        assert!(!verify(body, &good, "", AT, Some("n"), AT));
    }

    #[test]
    fn rotation_accepts_the_new_secret_only_within_grace() {
        let body = "{}";
        let with_new = sign(body, "new-secret", AT, Some("n"));
        let peer = |rotation_at| PeerSecrets {
            current: SECRET,
            pending: Some("new-secret"),
            rotation_at,
        };
        assert!(verify_peer(
            body,
            &with_new,
            AT,
            Some("n"),
            peer(Some(AT - 1000)),
            AT
        ));
        assert!(!verify_peer(
            body,
            &with_new,
            AT,
            Some("n"),
            peer(Some(AT - ROTATION_GRACE_MS - 1)),
            AT
        ));
        assert!(!verify_peer(body, &with_new, AT, Some("n"), peer(None), AT));
        let with_old = sign(body, SECRET, AT, Some("n"));
        assert!(verify_peer(body, &with_old, AT, Some("n"), peer(None), AT));
        assert!(!verify_peer(
            body,
            &with_new,
            AT,
            Some("n"),
            PeerSecrets {
                current: SECRET,
                pending: None,
                rotation_at: Some(AT)
            },
            AT
        ));
    }

    #[test]
    fn built_headers_parse_back_and_verify() {
        let body = r#"{"type":"ping"}"#;
        let built = headers(body, SECRET, "https://a.example", AT);
        let get = |name: &str| {
            built
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        };
        let signed = parse_headers(get);
        assert_eq!(
            signed
                .as_ref()
                .map(|signed| (signed.origin.as_str(), signed.timestamp)),
            Some(("https://a.example", AT))
        );
        let verified = signed.is_some_and(|signed| {
            verify(
                body,
                &signed.signature,
                SECRET,
                signed.timestamp,
                signed.nonce.as_deref(),
                AT,
            )
        });
        assert!(verified);
    }

    #[test]
    fn malformed_headers_are_refused() {
        let parse = |signature: &'static str,
                     origin: &'static str,
                     timestamp: &'static str,
                     nonce: Option<&'static str>| {
            parse_headers(move |name| match name {
                "x-federation-signature" => Some(signature),
                "x-federation-origin" => Some(origin),
                "x-federation-timestamp" => Some(timestamp),
                "x-federation-nonce" => nonce,
                _ => None,
            })
        };
        assert!(
            parse(
                "sha256=ab12",
                " https://a.example ",
                "1700000000000",
                Some(" n ")
            )
            .is_some()
        );
        assert_eq!(
            parse(
                "sha256=ab12",
                "https://a.example",
                "1700000000000",
                Some("  ")
            )
            .and_then(|signed| signed.nonce),
            None
        );
        for (signature, origin, timestamp, nonce) in [
            ("ab12", "o", "1", None),
            ("sha256=", "o", "1", None),
            ("sha256=xyz", "o", "1", None),
            ("sha256=ab", "  ", "1", None),
            ("sha256=ab", "o", "0", None),
            ("sha256=ab", "o", "-5", None),
            ("sha256=ab", "o", "abc", None),
            ("sha256=ab", "o", "NaN", None),
            ("sha256=ab", "o", "inf", None),
            ("sha256=ab", "o", "1e300", None),
        ] {
            assert!(
                parse(signature, origin, timestamp, nonce).is_none(),
                "{signature} {origin} {timestamp}"
            );
        }
        let long: &'static str = Box::leak("n".repeat(65).into_boxed_str());
        assert!(parse("sha256=ab", "o", "1", Some(long)).is_none());
    }

    #[test]
    fn origins_are_compared_without_scheme_and_slashes() {
        assert_eq!(
            normalize_origin(Some(" HTTPS://Chat.Example// ")).as_deref(),
            Some("chat.example")
        );
        assert_eq!(
            normalize_origin(Some("chat.example:8443/")).as_deref(),
            Some("chat.example:8443")
        );
        assert_eq!(normalize_origin(Some("http://")), None);
        assert_eq!(normalize_origin(None), None);
        assert_eq!(
            canonical_home_instance(Some("chat.example/")).as_deref(),
            Some("https://chat.example")
        );
        assert_eq!(
            canonical_home_instance(Some("http://Chat.Example//")).as_deref(),
            Some("http://Chat.Example")
        );
        assert_eq!(canonical_home_instance(Some("  ")), None);
        assert_eq!(generate_secret().len(), 64);
    }

    const VECTOR_WITH_NONCE: &str =
        "3876a92cfe5b494a8047d3ca58228adcb4de1ba3bf38f8167e830cba5c0ec271";
    const VECTOR_WITHOUT_NONCE: &str =
        "14d903a7968191213f1336d145a46fb179d74631812ab6f38f0018433c500c39";
}
