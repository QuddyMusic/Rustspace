//! Подсветка синтаксиса в блоках кода (как `prism-react-renderer` оригинала).
//!
//! Грамматики и темы — из bat (`two-face` поверх `syntect`, чистый Rust,
//! без oniguruma). Язык берётся из ограды (` ```bash `); без языка, с `text`
//! или с незнакомым языком блок показывается как есть, тоже как в оригинале.
//! Разметка кэшируется: подсветка не дешёвая, а лента перерисовывается часто.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::LazyLock;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme;
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);
static THEMES: LazyLock<EmbeddedLazyThemeSet> = LazyLock::new(two_face::theme::extra);

/// Сколько подсвеченных блоков держим в памяти.
const CACHE_LIMIT: usize = 256;
/// Длиннее не подсвечиваем: огромная вставка не должна подвешивать кадр.
const MAX_CODE_BYTES: usize = 64 * 1024;

thread_local! {
    static CACHE: RefCell<HashMap<u64, LayoutJob>> = RefCell::new(HashMap::new());
}

/// Псевдонимы Prism, которых нет среди расширений и имён грамматик bat.
fn canonical(lang: &str) -> &str {
    match lang {
        "shell" | "console" | "shellscript" => "bash",
        "golang" => "go",
        "csharp" | "c#" => "cs",
        "c++" => "cpp",
        "objc" | "objectivec" => "m",
        "py3" | "python3" => "py",
        "docker" => "dockerfile",
        "ps1" | "powershell" | "pwsh" => "ps1",
        "jsonc" | "json5" => "json",
        "vue" | "svelte" => "html",
        other => other,
    }
}

/// Грамматика по языку из ограды; `None` — показываем без подсветки.
fn syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let lang = lang.trim().to_lowercase();
    if lang.is_empty() || matches!(lang.as_str(), "text" | "txt" | "plain" | "plaintext") {
        return None;
    }
    SYNTAXES.find_syntax_by_token(canonical(&lang))
}

fn theme(dark: bool) -> &'static Theme {
    THEMES.get(if dark {
        EmbeddedThemeName::OneHalfDark
    } else {
        EmbeddedThemeName::OneHalfLight
    })
}

/// Подсвеченная разметка блока кода; `None` — язык не знаем (или код огромный).
#[must_use]
pub(crate) fn job(code: &str, lang: &str, font: &FontId, dark: bool) -> Option<LayoutJob> {
    if code.len() > MAX_CODE_BYTES {
        return None;
    }
    let syntax = syntax(lang)?;
    let mut hasher = DefaultHasher::new();
    (code, syntax.name.as_str(), dark, font.size.to_bits()).hash(&mut hasher);
    let key = hasher.finish();
    if let Some(job) = CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        return Some(job);
    }
    let job = build(code, syntax, font, dark)?;
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, job.clone());
    });
    Some(job)
}

fn build(code: &str, syntax: &SyntaxReference, font: &FontId, dark: bool) -> Option<LayoutJob> {
    let mut lines = HighlightLines::new(syntax, theme(dark));
    let mut job = LayoutJob::default();
    for line in LinesWithEndings::from(code) {
        let ranges = lines.highlight_line(line, &SYNTAXES).ok()?;
        for (style, piece) in ranges {
            let color =
                Color32::from_rgb(style.foreground.r, style.foreground.g, style.foreground.b);
            let mut format = TextFormat::simple(font.clone(), color);
            if style
                .font_style
                .contains(syntect::highlighting::FontStyle::ITALIC)
            {
                format.italics = true;
            }
            job.append(piece, 0.0, format);
        }
    }
    Some(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supports(lang: &str) -> bool {
        syntax(lang).is_some()
    }

    fn colors(job: &LayoutJob) -> std::collections::HashSet<Color32> {
        job.sections.iter().map(|s| s.format.color).collect()
    }

    #[test]
    fn common_languages_and_prism_aliases_are_known() {
        for lang in [
            "bash",
            "sh",
            "shell",
            "zsh",
            "rust",
            "rs",
            "python",
            "py",
            "javascript",
            "js",
            "typescript",
            "ts",
            "tsx",
            "json",
            "yaml",
            "yml",
            "toml",
            "c",
            "cpp",
            "c++",
            "csharp",
            "go",
            "golang",
            "java",
            "kotlin",
            "swift",
            "sql",
            "html",
            "css",
            "diff",
            "dockerfile",
            "lua",
            "ruby",
            "php",
            "markdown",
            "md",
            "Rust",
        ] {
            assert!(supports(lang), "{lang}");
        }
        for lang in ["", "text", "plaintext", "definitely-not-a-language"] {
            assert!(!supports(lang), "{lang}");
        }
    }

    #[test]
    fn bash_gets_several_colors_and_keeps_text() {
        let code = "echo \"Hello\" # привет\nexport A=1\n";
        let font = FontId::monospace(13.0);
        let job = job(code, "bash", &font, true).expect("подсветка");
        assert_eq!(job.text, code);
        assert!(colors(&job).len() >= 3, "{:?}", colors(&job));
        let light = super::job(code, "bash", &font, false).expect("светлая");
        assert_ne!(
            colors(&job),
            colors(&light),
            "своя палитра для светлой темы"
        );
    }

    #[test]
    fn unknown_language_is_left_plain() {
        assert!(job("x", "text", &FontId::monospace(13.0), true).is_none());
    }
}
