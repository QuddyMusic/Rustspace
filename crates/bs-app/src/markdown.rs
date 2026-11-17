//! Разбор текста сообщения: markdown как в Backspace.
//!
//! Поддерживается: `**жирный**`, `*курсив*`, `~~зачёркнутый~~`, `` `код` ``,
//! блоки кода в тройных обратных кавычках, цитаты `> ` и `>>> `,
//! `||спойлер||`, ссылки `http(s)://` и `[текст](адрес)`, упоминания `<@id>`,
//! `<@&id>`, `@everyone`, `@here`; заголовки `#`, `##`, `###`, подпись `-# `,
//! списки `- `, `* `, `1. ` (вложенность — отступом), разделитель `---`.
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
    /// Адрес ссылки `[текст](адрес)`; у голой ссылки адрес — сам текст.
    pub(crate) url: Option<String>,
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
            url: None,
        }
    }
}

/// Пункт списка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListItem {
    /// Уровень вложенности, с нуля.
    pub(crate) depth: u8,
    /// Номер у нумерованного пункта; `None` — маркер.
    pub(crate) number: Option<u32>,
    /// Текст пункта.
    pub(crate) spans: Vec<Span>,
}

/// Крупный блок сообщения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Block {
    /// Абзац из отрезков (может содержать переводы строк).
    Paragraph(Vec<Span>),
    /// Цитата.
    Quote(Vec<Span>),
    /// Заголовок `#`…`###`: уровень 1–3.
    Heading(u8, Vec<Span>),
    /// Мелкая приглушённая подпись `-# `.
    Subtext(Vec<Span>),
    /// Список.
    List(Vec<ListItem>),
    /// Горизонтальный разделитель.
    Rule,
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
        if let Some(quoted) = line.strip_prefix(">>> ") {
            // Всё до конца сообщения — цитата.
            flush(&mut blocks, &mut paragraph, &mut quote);
            let body = if tail.is_empty() {
                quoted.to_owned()
            } else {
                format!("{quoted}\n{tail}")
            };
            blocks.push(Block::Quote(inline(body.trim_end_matches('\n'), name_of)));
            break;
        }
        if let Some(block) = line_block(line, name_of) {
            flush(&mut blocks, &mut paragraph, &mut quote);
            match (block, blocks.last_mut()) {
                (Block::List(items), Some(Block::List(open))) => open.extend(items),
                (block, _) => blocks.push(block),
            }
            rest = tail;
            continue;
        }
        if let Some(quoted) = line
            .strip_prefix("> ")
            .or_else(|| (line == ">").then_some(""))
        {
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

/// Строка-блок: заголовок, подпись, пункт списка или разделитель.
fn line_block(line: &str, name_of: &dyn Fn(&str) -> Option<String>) -> Option<Block> {
    let trimmed = line.trim();
    if trimmed.len() >= 3
        && ["-", "*", "_"]
            .iter()
            .any(|c| trimmed.chars().all(|ch| ch.to_string() == *c))
    {
        return Some(Block::Rule);
    }
    for (prefix, level) in [("### ", 3), ("## ", 2), ("# ", 1)] {
        if let Some(text) = line.strip_prefix(prefix) {
            return Some(Block::Heading(level, inline(text.trim(), name_of)));
        }
    }
    if let Some(text) = line.strip_prefix("-# ") {
        return Some(Block::Subtext(inline(text.trim(), name_of)));
    }
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let body = &line[indent..];
    let depth = u8::try_from((indent / 2).min(4)).unwrap_or(4);
    if let Some(text) = ["- ", "* ", "+ "]
        .iter()
        .find_map(|marker| body.strip_prefix(marker))
    {
        return Some(Block::List(vec![ListItem {
            depth,
            number: None,
            spans: inline(text, name_of),
        }]));
    }
    let digits = body.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits)
        && let Some(text) = body[digits..]
            .strip_prefix(". ")
            .or_else(|| body[digits..].strip_prefix(") "))
    {
        return Some(Block::List(vec![ListItem {
            depth,
            number: body[..digits].parse().ok(),
            spans: inline(text, name_of),
        }]));
    }
    None
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
            && last.url == span.url
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

/// Разбор строки: текст, стиль снаружи и как назвать упомянутого.
struct Inline<'a> {
    text: &'a str,
    style: Style,
    name_of: &'a dyn Fn(&str) -> Option<String>,
}

/// Найденная разметка: сколько байт съедено и что вывести.
type Found = Option<(usize, Vec<Span>)>;

impl Inline<'_> {
    /// Перед позицией `i` — не буква и не цифра (граница слова).
    fn word_start(&self, i: usize) -> bool {
        i == 0 || !self.text.as_bytes()[i - 1].is_ascii_alphanumeric()
    }

    /// Вложенный разбор с изменённым стилем.
    fn nested(&self, inner: &str, style: Style) -> Vec<Span> {
        let mut spans = Vec::new();
        inline_into(inner, style, self.name_of, &mut spans);
        spans
    }

    /// `` `код` ``.
    fn code(&self, i: usize) -> Found {
        let body = self.text[i..].strip_prefix('`')?;
        let end = body.find('`').filter(|end| *end > 0)?;
        Some((
            end + 2,
            vec![Span::of(&body[..end], Kind::Code, self.style)],
        ))
    }

    /// `**жирный**`, `~~зачёркнутый~~`, `||спойлер||`.
    fn paired(&self, i: usize) -> Found {
        let rest = &self.text[i..];
        for token in ["**", "~~", "||"] {
            let Some(body) = rest.strip_prefix(token) else {
                continue;
            };
            let Some(end) = body.find(token).filter(|end| *end > 0) else {
                continue;
            };
            let inner = &body[..end];
            let spans = match token {
                "**" => self.nested(
                    inner,
                    Style {
                        bold: true,
                        ..self.style
                    },
                ),
                "~~" => self.nested(
                    inner,
                    Style {
                        strike: true,
                        ..self.style
                    },
                ),
                _ => vec![Span::of(inner, Kind::Spoiler, self.style)],
            };
            return Some((end + token.len() * 2, spans));
        }
        None
    }

    /// `*курсив*` и `_курсив_` (подчёркивание — только по границам слов).
    fn italic(&self, i: usize) -> Found {
        let rest = &self.text[i..];
        let (marker, body) = if let Some(body) = rest.strip_prefix('*') {
            ('*', body)
        } else {
            ('_', rest.strip_prefix('_').filter(|_| self.word_start(i))?)
        };
        if body.starts_with([marker, ' ']) {
            return None;
        }
        let end = body.find(marker).filter(|end| *end > 0)?;
        if body[..end].ends_with(' ') {
            return None;
        }
        if marker == '_'
            && body[end + 1..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric)
        {
            return None;
        }
        let spans = self.nested(
            &body[..end],
            Style {
                italic: true,
                ..self.style
            },
        );
        Some((end + 2, spans))
    }

    /// Ссылка с текстом `[текст](https://…)`.
    fn masked(&self, i: usize) -> Found {
        let body = self.text[i..].strip_prefix('[')?;
        let close = body.find("](")?;
        let label = &body[..close];
        if label.is_empty() || label.contains('\n') {
            return None;
        }
        let after = &body[close + 2..];
        if !(after.starts_with("http://") || after.starts_with("https://")) {
            return None;
        }
        let end = after.find(')')?;
        let url = &after[..end];
        if url.contains(char::is_whitespace) {
            return None;
        }
        let mut spans = self.nested(label, self.style);
        for span in &mut spans {
            span.kind = Kind::Link;
            span.url = Some(url.to_owned());
        }
        Some((1 + close + 2 + end + 1, spans))
    }

    /// Голая ссылка `https://…`.
    fn autolink(&self, i: usize) -> Found {
        let rest = &self.text[i..];
        if !(rest.starts_with("http://") || rest.starts_with("https://")) || !self.word_start(i) {
            return None;
        }
        let end = link_end(rest);
        (end > "https://".len() - 1)
            .then(|| (end, vec![Span::of(&rest[..end], Kind::Link, self.style)]))
    }

    /// Упоминание `<@id>` (`<@&id>` — роль), если имя известно.
    fn mention(&self, i: usize) -> Found {
        let body = self.text[i..].strip_prefix("<@")?;
        let end = body.find('>')?;
        let id = &body[..end];
        let valid = !id.is_empty()
            && id
                .trim_start_matches('&')
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        let name = valid.then(|| (self.name_of)(id)).flatten()?;
        let mut span = Span::of(format!("@{name}"), Kind::Mention, self.style);
        // Упоминание человека (не роли) можно открыть щелчком.
        if !id.starts_with('&') {
            span.url = Some(format!("user:{id}"));
        }
        Some((end + 3, vec![span]))
    }

    /// `@everyone` и `@here` отдельным словом.
    fn everyone(&self, i: usize) -> Found {
        let rest = &self.text[i..];
        ["@everyone", "@here"].into_iter().find_map(|keyword| {
            (rest.starts_with(keyword)
                && self.word_start(i)
                && rest[keyword.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric()))
            .then(|| {
                (
                    keyword.len(),
                    vec![Span::of(keyword, Kind::Mention, self.style)],
                )
            })
        })
    }
}

fn inline_into(
    text: &str,
    style: Style,
    name_of: &dyn Fn(&str) -> Option<String>,
    out: &mut Vec<Span>,
) {
    let parser = Inline {
        text,
        style,
        name_of,
    };
    let mut plain = String::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        // Экранирование.
        if rest.starts_with('\\')
            && let Some(next) = rest[1..].chars().next()
            && "*_~`|\\<>[]#-".contains(next)
        {
            plain.push(next);
            i += 1 + next.len_utf8();
            continue;
        }
        let found = parser
            .code(i)
            .or_else(|| parser.masked(i))
            .or_else(|| parser.paired(i))
            .or_else(|| parser.italic(i))
            .or_else(|| parser.autolink(i))
            .or_else(|| parser.mention(i))
            .or_else(|| parser.everyone(i));
        if let Some((used, spans)) = found {
            if !plain.is_empty() {
                out.push(Span::plain(std::mem::take(&mut plain), style));
            }
            out.extend(spans);
            i += used;
            continue;
        }
        let Some(ch) = rest.chars().next() else {
            break;
        };
        plain.push(ch);
        i += ch.len_utf8();
    }
    if !plain.is_empty() {
        out.push(Span::plain(plain, style));
    }
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

    #[test]
    fn headings_lists_rules_and_subtext() {
        let blocks = parse(
            "## Правила\n- **2.14** спам\n  - вложенный\n1. первый\n---\n-# мелко\nтекст",
            &none,
        );
        assert!(matches!(&blocks[0], super::Block::Heading(2, s) if s[0].text == "Правила"));
        let super::Block::List(items) = &blocks[1] else {
            panic!("{blocks:?}");
        };
        assert_eq!(items.len(), 3);
        assert_eq!((items[1].depth, items[1].number), (1, None));
        assert_eq!(items[2].number, Some(1));
        assert!(matches!(blocks[2], super::Block::Rule));
        assert!(matches!(&blocks[3], super::Block::Subtext(s) if s[0].text == "мелко"));
        assert!(matches!(&blocks[4], super::Block::Paragraph(_)));
        // «2.14.0 - Спам» — не нумерованный список.
        assert!(matches!(
            &parse("2.14.0 - Спам", &none)[0],
            super::Block::Paragraph(_)
        ));
    }

    #[test]
    fn masked_links_and_long_quotes() {
        let spans = inline("см. [правила](https://example.org/r) тут", &none);
        let link = spans.iter().find(|s| s.kind == Kind::Link).expect("link");
        assert_eq!(link.text, "правила");
        assert_eq!(link.url.as_deref(), Some("https://example.org/r"));
        let blocks = parse("до\n>>> раз\nдва", &none);
        assert!(matches!(&blocks[1], super::Block::Quote(s) if s[0].text == "раз\nдва"));
    }
}
