//! Разбор текста сообщения: markdown как в Backspace.
//!
//! Поддерживается: `**жирный**`, `*курсив*`, `~~зачёркнутый~~`, `` `код` ``,
//! блоки кода в тройных обратных кавычках, цитаты `> `, `||спойлер||`,
//! ссылки `http(s)://`, упоминания `<@id>`, `<@&id>`, `@everyone`, `@here`.
//! Внутри кода разметка и упоминания не работают.

/// Вид отрезка текста.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Обычный текст со своими начертаниями.
    Text,
    /// Строка кода.
    Code,
    /// Ссылка.
    Link,
    /// Упоминание пользователя, роли или `@everyone`.
    Mention,
    /// Спойлер: скрыт до щелчка.
    Spoiler,
}

/// Отрезок строки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Span {
    /// Что показывать.
    pub(crate) text: String,
    /// Вид.
    pub(crate) kind: Kind,
    /// Жирный.
    pub(crate) bold: bool,
    /// Курсив.
    pub(crate) italic: bool,
    /// Зачёркнутый.
    pub(crate) strike: bool,
}

impl Span {
    fn plain(text: impl Into<String>, style: Style) -> Self {
        Self::of(text, Kind::Text, style)
    }

    fn of(text: impl Into<String>, kind: Kind, style: Style) -> Self {
        Self {
            text: text.into(),
            kind,
            bold: style.bold,
            italic: style.italic,
            strike: style.strike,
        }
    }
}

/// Крупный блок сообщения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Block {
    /// Абзац из отрезков (может содержать переводы строк).
    Paragraph(Vec<Span>),
    /// Цитата.
    Quote(Vec<Span>),
    /// Блок кода: язык из ограды (` ```bash `) и сам код.
    Code {
        /// Язык; `None` — ограда без языка.
        lang: Option<String>,
        /// Код.
        code: String,
    },
}

#[derive(Debug, Clone, Copy, Default)]
struct Style {
    bold: bool,
    italic: bool,
    strike: bool,
}

/// Разбирает текст на блоки. `name_of` по идентификатору пользователя (или
/// роли с префиксом `&`) возвращает показываемое имя.
pub(crate) fn parse(text: &str, name_of: &dyn Fn(&str) -> Option<String>) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph = String::new();
    let mut quote = String::new();
    let mut rest = text;
    let flush = |blocks: &mut Vec<Block>, paragraph: &mut String, quote: &mut String| {
        if !paragraph.is_empty() {
            let body = paragraph.trim_end_matches('\n');
            if !body.is_empty() {
                blocks.push(Block::Paragraph(inline(body, name_of)));
            }
            paragraph.clear();
        }
        if !quote.is_empty() {
            blocks.push(Block::Quote(inline(quote.trim_end_matches('\n'), name_of)));
            quote.clear();
        }
    };
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("```")
            && let Some(end) = after.find("```")
        {
            flush(&mut blocks, &mut paragraph, &mut quote);
            let code = &after[..end];
            // Язык после открывающих кавычек (`rust\n…`) — для подсветки.
            let (lang, code) = match code.split_once('\n') {
                Some((first, body))
                    if !first.trim().is_empty()
                        && first.trim().chars().all(|c| {
                            c.is_alphanumeric() || matches!(c, '+' | '-' | '#' | '_' | '.')
                        }) =>
                {
                    (Some(first.trim().to_owned()), body)
                }
                _ => (None, code),
            };
            blocks.push(Block::Code {
                lang,
                code: code.trim_matches('\n').to_owned(),
            });
            rest = &after[end + 3..];
            rest = rest.strip_prefix('\n').unwrap_or(rest);
            continue;
        }
        let (line, tail) = rest.split_once('\n').map_or((rest, ""), |(l, t)| (l, t));
        let had_newline = rest.len() > line.len();
        if let Some(quoted) = line.strip_prefix("> ") {
            if !paragraph.is_empty() {
                flush(&mut blocks, &mut paragraph, &mut quote);
            }
            quote.push_str(quoted);
            quote.push('\n');
        } else {
            if !quote.is_empty() {
                flush(&mut blocks, &mut paragraph, &mut quote);
            }
            paragraph.push_str(line);
            if had_newline {
                paragraph.push('\n');
            }
        }
        rest = tail;
    }
    flush(&mut blocks, &mut paragraph, &mut quote);
    blocks
}

/// Разбирает строку без блочной разметки.
pub(crate) fn inline(text: &str, name_of: &dyn Fn(&str) -> Option<String>) -> Vec<Span> {
    let mut out = Vec::new();
    inline_into(text, Style::default(), name_of, &mut out);
    merge(out)
}

/// Склеивает соседние обычные отрезки одного начертания.
fn merge(spans: Vec<Span>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        if let Some(last) = out.last_mut()
            && last.kind == Kind::Text
            && span.kind == Kind::Text
            && last.bold == span.bold
            && last.italic == span.italic
            && last.strike == span.strike
        {
            last.text.push_str(&span.text);
            continue;
        }
        out.push(span);
    }
    out
}

/// Конец ссылки: без хвостовой пунктуации.
fn link_end(text: &str) -> usize {
    let end = text
        .find(|c: char| c.is_whitespace() || c == '<')
        .unwrap_or(text.len());
    let trimmed = text[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '"', '\'']);
    trimmed.len()
}

#[allow(clippy::too_many_lines)]
fn inline_into(
    text: &str,
    style: Style,
    name_of: &dyn Fn(&str) -> Option<String>,
    out: &mut Vec<Span>,
) {
    let mut plain = String::new();
    let mut i = 0;
    let bytes = text.as_bytes();
    let flush = |plain: &mut String, out: &mut Vec<Span>| {
        if !plain.is_empty() {
            out.push(Span::plain(std::mem::take(plain), style));
        }
    };
    while i < text.len() {
        let rest = &text[i..];
        // Экранирование.
        if rest.starts_with('\\')
            && let Some(next) = rest[1..].chars().next()
            && "*_~`|\\<>".contains(next)
        {
            plain.push(next);
            i += 1 + next.len_utf8();
            continue;
        }
        if let Some(body) = rest.strip_prefix('`')
            && let Some(end) = body.find('`')
            && end > 0
        {
            flush(&mut plain, out);
            out.push(Span::of(&body[..end], Kind::Code, style));
            i += end + 2;
            continue;
        }
        let paired = [("**", 0_u8), ("~~", 1), ("||", 2)];
        let mut matched = false;
        for (token, which) in paired {
            if let Some(body) = rest.strip_prefix(token)
                && let Some(end) = body.find(token)
                && end > 0
            {
                flush(&mut plain, out);
                let inner = &body[..end];
                match which {
                    0 => inline_into(
                        inner,
                        Style {
                            bold: true,
                            ..style
                        },
                        name_of,
                        out,
                    ),
                    1 => inline_into(
                        inner,
                        Style {
                            strike: true,
                            ..style
                        },
                        name_of,
                        out,
                    ),
                    _ => out.push(Span::of(inner, Kind::Spoiler, style)),
                }
                i += end + token.len() * 2;
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        if let Some(body) = rest.strip_prefix('*')
            && !body.starts_with(['*', ' '])
            && let Some(end) = body.find('*')
            && end > 0
            && !body[..end].ends_with(' ')
        {
            flush(&mut plain, out);
            inline_into(
                &body[..end],
                Style {
                    italic: true,
                    ..style
                },
                name_of,
                out,
            );
            i += end + 2;
            continue;
        }
        if let Some(body) = rest.strip_prefix('_')
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
            && !body.starts_with(['_', ' '])
            && let Some(end) = body.find('_')
            && end > 0
            && !body[..end].ends_with(' ')
            && body[end + 1..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric())
        {
            flush(&mut plain, out);
            inline_into(
                &body[..end],
                Style {
                    italic: true,
                    ..style
                },
                name_of,
                out,
            );
            i += end + 2;
            continue;
        }
        if (rest.starts_with("http://") || rest.starts_with("https://"))
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
        {
            let end = link_end(rest);
            if end > "https://".len() - 1 {
                flush(&mut plain, out);
                out.push(Span::of(&rest[..end], Kind::Link, style));
                i += end;
                continue;
            }
        }
        if let Some(body) = rest.strip_prefix("<@")
            && let Some(end) = body.find('>')
        {
            let id = &body[..end];
            let valid = !id.is_empty()
                && id
                    .trim_start_matches('&')
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if valid && let Some(name) = name_of(id) {
                flush(&mut plain, out);
                out.push(Span::of(format!("@{name}"), Kind::Mention, style));
                i += end + 3;
                continue;
            }
        }
        for keyword in ["@everyone", "@here"] {
            if rest.starts_with(keyword)
                && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
                && rest[keyword.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric())
            {
                flush(&mut plain, out);
                out.push(Span::of(keyword, Kind::Mention, style));
                i += keyword.len();
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        if let Some(ch) = rest.chars().next() {
            plain.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    flush(&mut plain, out);
}

#[cfg(test)]
mod tests {
    use super::{Block, Kind, inline, parse};

    fn none(_: &str) -> Option<String> {
        None
    }

    fn kinds(text: &str) -> Vec<(String, Kind, bool, bool, bool)> {
        inline(text, &none)
            .into_iter()
            .map(|s| (s.text, s.kind, s.bold, s.italic, s.strike))
            .collect()
    }

    #[test]
    fn plain_text_is_one_span() {
        assert_eq!(
            kinds("привет мир"),
            vec![("привет мир".into(), Kind::Text, false, false, false)]
        );
    }

    #[test]
    fn bold_italic_strike_nest() {
        let spans = inline("a **b *c* d** ~~e~~", &none);
        assert!(spans.iter().any(|s| s.text == "b " && s.bold && !s.italic));
        assert!(spans.iter().any(|s| s.text == "c" && s.bold && s.italic));
        assert!(spans.iter().any(|s| s.text == "e" && s.strike));
    }

    #[test]
    fn code_blocks_ignore_markup() {
        let spans = inline("`**не жирный**`", &none);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].kind, Kind::Code);
        assert_eq!(spans[0].text, "**не жирный**");
        let blocks = parse("до\n```rust\nlet a = *b;\n```\nпосле", &none);
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks[1],
            Block::Code {
                lang: Some("rust".into()),
                code: "let a = *b;".into()
            }
        );
        let bare = parse("```\nls -la\n```", &none);
        assert_eq!(
            bare[0],
            Block::Code {
                lang: None,
                code: "ls -la".into()
            }
        );
    }

    #[test]
    fn links_drop_trailing_punctuation() {
        let spans = inline("смотри https://example.com/a?b=1, ок", &none);
        let link = spans.iter().find(|s| s.kind == Kind::Link).unwrap();
        assert_eq!(link.text, "https://example.com/a?b=1");
        assert!(
            inline("ftp://x", &none)
                .iter()
                .all(|s| s.kind != Kind::Link)
        );
    }

    #[test]
    fn mentions_resolve_only_known_ids() {
        let names = |id: &str| (id == "42").then(|| "Аня".to_owned());
        let spans = inline("привет <@42> и <@7> и @everyone", &names);
        let mentions: Vec<_> = spans
            .iter()
            .filter(|s| s.kind == Kind::Mention)
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(mentions, vec!["@Аня", "@everyone"]);
        assert!(spans.iter().any(|s| s.text.contains("<@7>")));
    }

    #[test]
    fn spoiler_quote_and_escape() {
        assert!(
            inline("||тайна||", &none)
                .iter()
                .any(|s| s.kind == Kind::Spoiler)
        );
        let blocks = parse("> цитата\nобычный", &none);
        assert!(matches!(blocks[0], Block::Quote(_)));
        assert!(matches!(blocks[1], Block::Paragraph(_)));
        assert_eq!(kinds(r"\*не курсив\*")[0].0, "*не курсив*");
    }

    #[test]
    fn snake_case_is_not_italic() {
        assert_eq!(kinds("my_var_name").len(), 1);
        assert_eq!(kinds("2 * 3 * 4").len(), 1);
    }
}
