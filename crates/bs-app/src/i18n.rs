//! Локализация интерфейса.
//!
//! Исходный язык — русский: строки в коде написаны по-русски и служат
//! ключами каталогов (подход gettext). Каталог — JSON `{"русская строка":
//! "перевод"}`; встроены `en`, дополнительные языки можно положить файлами
//! `locales/<код>.json` в каталог настроек приложения. Строки без перевода
//! показываются по-русски, так что неполный каталог ничего не ломает.
//!
//! - [`tr`] — простая строка;
//! - [`tf!`](crate::tf) — шаблон с именованными подстановками `{name}`:
//!   `tf!("Сохранено: {path}", path = shown)`. Переводчик переставляет
//!   подстановки как угодно, имена должны совпадать.
//!
//! Язык переключается на лету: интерфейс берёт строки каждый кадр.

use std::collections::HashMap;
use std::fmt::{Display, Write as _};
use std::sync::RwLock;

/// Встроенный английский каталог.
const EN: &str = include_str!("../locales/en.json");

/// Код языка и каталог переводов.
struct Catalog {
    code: String,
    entries: HashMap<String, &'static str>,
}

/// Текущий каталог; `None` — русский (исходный).
#[cfg(not(test))]
static ACTIVE: RwLock<Option<Catalog>> = RwLock::new(None);

// В тестах язык свой у каждого потока: тесты идут параллельно, и смена
// языка в одном не должна ломать проверки русских строк в другом.
#[cfg(test)]
thread_local! {
    static ACTIVE: RwLock<Option<Catalog>> = const { RwLock::new(None) };
}

fn with_active<R>(f: impl FnOnce(&RwLock<Option<Catalog>>) -> R) -> R {
    #[cfg(not(test))]
    {
        f(&ACTIVE)
    }
    #[cfg(test)]
    {
        ACTIVE.with(f)
    }
}

/// Описание доступного языка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LanguageInfo {
    /// Код (`ru`, `en`, …).
    pub(crate) code: String,
    /// Самоназвание языка.
    pub(crate) name: String,
}

/// Встроенные языки и найденные в каталоге настроек.
pub(crate) fn available() -> Vec<LanguageInfo> {
    let mut out = vec![
        LanguageInfo {
            code: "ru".to_owned(),
            name: "Русский".to_owned(),
        },
        LanguageInfo {
            code: "en".to_owned(),
            name: "English".to_owned(),
        },
    ];
    if let Some(dir) = user_dir()
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        let mut extra: Vec<LanguageInfo> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let code = path.file_stem()?.to_str()?.to_owned();
                (path.extension()? == "json" && valid_code(&code) && code != "ru" && code != "en")
                    .then(|| {
                    let name = parse(&crate::files::read_to_string(&path).ok()?)
                        .ok()?
                        .remove("@language")
                        .unwrap_or_else(|| code.clone());
                    Some(LanguageInfo { code, name })
                })?
            })
            .collect();
        extra.sort_by(|a, b| a.code.cmp(&b.code));
        out.extend(extra);
    }
    out
}

/// Каталог пользовательских переводов.
fn user_dir() -> Option<std::path::PathBuf> {
    crate::saved_session::config_dir().map(|dir| dir.join("locales"))
}

/// Код языка: 2–8 латинских букв, цифр или дефисов (`pt-BR`, `zh`).
fn valid_code(code: &str) -> bool {
    (2..=8).contains(&code.len()) && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn parse(json: &str) -> Result<HashMap<String, String>, serde_json::Error> {
    serde_json::from_str(json)
}

/// Включает язык. Неизвестный или битый каталог — остаётся русский.
pub(crate) fn set_language(code: &str) {
    let catalog = match code {
        "ru" | "" => None,
        "en" => load(code, EN),
        other if valid_code(other) => user_dir()
            .and_then(|dir| crate::files::read_to_string(dir.join(format!("{other}.json"))).ok())
            .and_then(|json| load(other, &json)),
        _ => None,
    };
    with_active(|lock| {
        if let Ok(mut active) = lock.write() {
            *active = catalog;
        }
    });
}

/// Код текущего языка.
pub(crate) fn current() -> String {
    with_active(|lock| {
        lock.read()
            .ok()
            .and_then(|active| active.as_ref().map(|c| c.code.clone()))
    })
    .unwrap_or_else(|| "ru".to_owned())
}

/// Строки каталога живут всю программу: язык меняют редко, а утечка — пара
/// сотен килобайт за переключение. Зато `tr` отдаёт `&'static str` и
/// встаёт на место любого литерала.
fn load(code: &str, json: &str) -> Option<Catalog> {
    match parse(json) {
        Ok(map) => Some(Catalog {
            code: code.to_owned(),
            entries: map
                .into_iter()
                .filter(|(key, value)| !key.starts_with('@') && !value.is_empty())
                .map(|(key, value)| (key, &*Box::leak(value.into_boxed_str())))
                .collect(),
        }),
        Err(err) => {
            tracing::warn!(%err, code, "каталог перевода не разобран");
            None
        }
    }
}

/// Перевод строки; без перевода — исходная.
#[must_use]
pub(crate) fn tr(source: &'static str) -> &'static str {
    with_active(|lock| {
        lock.read()
            .ok()
            .and_then(|active| active.as_ref()?.entries.get(source).copied())
    })
    .unwrap_or(source)
}

/// Подставляет значения в шаблон `{name}`; `{{`/`}}` — фигурные скобки.
/// Неизвестная подстановка остаётся как есть.
#[must_use]
pub(crate) fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if let Some(stripped) = tail.strip_prefix('{')
            && let Some(end) = stripped.find('}')
            && let Some((_, value)) = args.iter().find(|(name, _)| *name == &stripped[..end])
        {
            let _ = write!(out, "{value}");
            rest = &stripped[end + 1..];
            continue;
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// Переведённый шаблон с подстановками: `tf!("Привет, {name}", name = user)`.
#[macro_export]
macro_rules! tf {
    ($template:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::tr($template),
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*],
        )
    };
}

/// Ключи встроенного английского каталога (для проверки полноты).
#[cfg(test)]
pub(crate) fn english_keys() -> Vec<String> {
    parse(EN)
        .map(|m| m.into_keys().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{fill, set_language, tr};

    #[test]
    fn fill_substitutes_named_values_and_escapes() {
        assert_eq!(fill("{a} и {b}", &[("a", &1), ("b", &"x")]), "1 и x");
        assert_eq!(fill("{{литерал}} {a}", &[("a", &2)]), "{литерал} 2");
        assert_eq!(fill("{нет} {a}", &[("a", &3)]), "{нет} 3");
        assert_eq!(fill("незакрыта {a", &[("a", &3)]), "незакрыта {a");
    }

    /// Разбирает строковый литерал Rust, начинающийся с `"` в `src[at..]`.
    fn literal(src: &str, at: usize) -> Option<String> {
        let mut out = String::new();
        let mut chars = src[at + 1..].chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    '\n' => {
                        while chars.peek().is_some_and(|c| c.is_whitespace()) {
                            chars.next();
                        }
                    }
                    other => out.push(other),
                },
                c => out.push(c),
            }
        }
        None
    }

    /// Каждая строка `tr("…")`/`tf!("…")` в коде есть в английском каталоге.
    #[test]
    fn english_catalog_covers_every_string() {
        let keys: std::collections::HashSet<String> = super::english_keys().into_iter().collect();
        let mut missing = Vec::new();
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") || path.ends_with("i18n.rs") {
                    continue;
                }
                let src = crate::files::read_to_string(&path).unwrap();
                let src = src.split("#[cfg(test)]").next().unwrap_or_default();
                for marker in ["tr(", "tf!("] {
                    let mut from = 0;
                    while let Some(found) = src[from..].find(marker) {
                        let start = from + found;
                        let at = start + marker.len();
                        from = at;
                        let word = src[..start]
                            .chars()
                            .next_back()
                            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '!');
                        let line = &src[src[..start].rfind('\n').map_or(0, |i| i + 1)..start];
                        if word || line.trim_start().starts_with("//") {
                            continue;
                        }
                        let rest = &src[at..];
                        let quote = rest.len() - rest.trim_start().len();
                        if !rest.trim_start().starts_with('"') {
                            continue;
                        }
                        if let Some(text) = literal(src, at + quote)
                            && !keys.contains(&text)
                        {
                            missing.push(format!("{}: {text}", path.display()));
                        }
                    }
                }
            }
        }
        assert!(missing.is_empty(), "нет перевода:\n{}", missing.join("\n"));
    }

    #[test]
    fn english_catalog_translates_and_falls_back() {
        set_language("en");
        assert_eq!(tr("Пароль"), "Password");
        assert_eq!(tr("строки нет в каталоге"), "строки нет в каталоге");
        assert_eq!(
            crate::tf!("Не меньше {PASSWORD_MIN} символов", PASSWORD_MIN = 8),
            "At least 8 characters"
        );
        set_language("ru");
        assert_eq!(tr("Пароль"), "Пароль");
    }
}
