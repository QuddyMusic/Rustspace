//! Мелочи, повторяющие поведение JavaScript там, где от него зависит ответ.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// `String.prototype.trim()`: пробельные символы Unicode, переводы строк и BOM.
#[must_use]
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// `string.length`: длина в единицах UTF-16.
#[must_use]
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `string.slice(0, max)`, не разрывая суррогатную пару.
#[must_use]
pub fn utf16_prefix(text: &str, max: usize) -> &str {
    let mut units = 0;
    for (index, c) in text.char_indices() {
        units += c.len_utf16();
        if units > max {
            return &text[..index];
        }
    }
    text
}

/// Истинность значения по правилам JavaScript.
#[must_use]
pub fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// Поле тела запроса; отсутствующее — `None` (`undefined`), `null` — `Some(Null)`.
#[must_use]
pub fn field<'a>(body: &'a Value, name: &str) -> Option<&'a Value> {
    body.as_object().and_then(|object| object.get(name))
}

/// Строковое поле, если оно есть и это строка.
#[must_use]
pub fn str_field<'a>(body: &'a Value, name: &str) -> Option<&'a str> {
    field(body, name).and_then(Value::as_str)
}

static FORBIDDEN: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(
        clippy::expect_used,
        reason = "выражение постоянное и проверено тестом"
    )]
    Regex::new(r"[\p{Cc}\u{2028}\u{2029}\u{202A}-\u{202E}\u{2066}-\u{2069}]").expect("regex")
});
static INVISIBLE: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(
        clippy::expect_used,
        reason = "выражение постоянное и проверено тестом"
    )]
    Regex::new(r"[\p{Cf}\p{White_Space}\u{115F}\u{1160}\u{3164}\u{FFA0}]").expect("regex")
});

/// Годится ли обрезанная непустая подпись в имена и заголовки
/// (`isVisibleSingleLine`): без управляющих символов, переводов строк и
/// двунаправленных меток и хотя бы с одним видимым знаком.
#[must_use]
pub fn is_visible_single_line(trimmed: &str) -> bool {
    !FORBIDDEN.is_match(trimmed) && !INVISIBLE.replace_all(trimmed, "").is_empty()
}

/// Наибольшая длина ника в пространстве и титула владельца.
pub const MAX_LABEL_LENGTH: usize = 32;

/// Ник участника в том виде, в каком он хранится (`normalizeMemberNickname`):
/// `Ok(None)` — сбросить, `Err(())` — значение не годится.
///
/// # Errors
/// Значение — не ник.
pub fn normalize_nickname(value: &Value) -> Result<Option<String>, ()> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => {
            let trimmed = js_trim(text);
            if trimmed.is_empty() {
                Ok(None)
            } else if utf16_len(trimmed) > MAX_LABEL_LENGTH || !is_visible_single_line(trimmed) {
                Err(())
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        _ => Err(()),
    }
}

/// Титул владельца (`normalizeOwnerTitle`): пустой, в отличие от ника, не
/// сбрасывает — только `null`.
///
/// # Errors
/// Значение — не титул.
pub fn normalize_owner_title(value: &Value) -> Result<Option<String>, ()> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => {
            let trimmed = js_trim(text);
            if trimmed.is_empty()
                || utf16_len(trimmed) > MAX_LABEL_LENGTH
                || !is_visible_single_line(trimmed)
            {
                Err(())
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn js_string_rules() {
        assert_eq!(js_trim("\u{feff} a \n"), "a");
        assert_eq!(utf16_len("a😀"), 3);
        assert_eq!(utf16_prefix("a😀b", 2), "a");
        assert_eq!(utf16_prefix("abc", 2), "ab");
        assert!(truthy(Some(&json!("x"))));
        assert!(!truthy(Some(&json!(""))));
        assert!(!truthy(Some(&json!(0))));
        assert!(truthy(Some(&json!([]))));
    }

    #[test]
    fn labels() {
        assert_eq!(
            normalize_nickname(&json!("  Вася ")),
            Ok(Some("Вася".into()))
        );
        assert_eq!(normalize_nickname(&json!("   ")), Ok(None));
        assert_eq!(normalize_nickname(&json!(null)), Ok(None));
        assert!(normalize_nickname(&json!("a\nb")).is_err());
        assert!(normalize_nickname(&json!("\u{200b}")).is_err());
        assert!(normalize_nickname(&json!("\u{202e}evil")).is_err());
        assert!(normalize_nickname(&json!(5)).is_err());
        assert!(normalize_nickname(&json!("x".repeat(33))).is_err());
        assert!(normalize_owner_title(&json!("")).is_err());
        assert_eq!(
            normalize_owner_title(&json!("Король")),
            Ok(Some("Король".into()))
        );
    }
}
