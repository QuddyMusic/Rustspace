//! Разбор и запись JSON так, как это делают `JSON.parse` и `JSON.stringify`
//! в JavaScript (V8).
//!
//! Подпись запроса между инстансами считается не по байтам тела, а по
//! `JSON.stringify(request.body)`: оригинал разбирает тело и записывает его
//! заново. Чтобы подписи сходились, здесь повторяются разбор и запись:
//! порядок ключей (сначала индексы по возрастанию, потом остальные в порядке
//! появления; у повторного ключа остаётся последнее значение на первом
//! месте), запись чисел (`1.0` → `1`, `1e21` → `1e+21`) и экранирование строк
//! (включая одиночные суррогаты).
//!
//! Тот же разбор даёт тексты ошибок V8 («Unexpected token 'x', … is not
//! valid JSON», позиция в единицах UTF-16 со строкой и столбцом): их Fastify
//! возвращает клиенту, когда тело запроса — не JSON.

use std::fmt::Write as _;

/// Наибольшая вложенность: глубже разбор отказывает, а не рискует стеком.
const MAX_DEPTH: usize = 512;
/// Исходники короче этого показываются в ошибке целиком.
const MIN_LENGTH_FOR_CONTEXT: usize = 21;
/// Сколько символов по сторонам от ошибки показывается в длинном исходнике.
const CONTEXT: usize = 10;

#[derive(Debug)]
enum Node {
    Null,
    Bool(bool),
    Number(f64),
    Text(Vec<u16>),
    List(Vec<Self>),
    Object(Vec<(Vec<u16>, Self)>),
}

/// Ошибка разбора: что и где.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// Неожиданный символ (или конец ввода) там, где ждали значение.
    Token,
    /// Неожиданное число.
    Number,
    /// Ошибка с собственным текстом; к нему добавляется позиция.
    At(&'static str),
    /// Глубже [`MAX_DEPTH`].
    TooDeep,
}

struct Parser {
    units: Vec<u16>,
    at: usize,
    fault: Option<(Fault, usize)>,
}

const QUOTE: u16 = b'"' as u16;
const SLASH: u16 = b'\\' as u16;

fn is_digit(unit: u16) -> bool {
    (u16::from(b'0')..=u16::from(b'9')).contains(&unit)
}

impl Parser {
    fn peek(&self) -> Option<u16> {
        self.units.get(self.at).copied()
    }

    fn is(&self, byte: u8) -> bool {
        self.peek() == Some(u16::from(byte))
    }

    fn fail<T>(&mut self, fault: Fault, at: usize) -> Option<T> {
        self.fault = Some((fault, at));
        None
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(0x20 | 0x09 | 0x0a | 0x0d)) {
            self.at += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Option<Node> {
        if depth > MAX_DEPTH {
            return self.fail(Fault::TooDeep, self.at);
        }
        self.space();
        let Some(unit) = self.peek() else {
            return self.fail(Fault::Token, self.at);
        };
        match u8::try_from(unit).unwrap_or(0) {
            b'"' => self.text().map(Node::Text),
            b'[' => self.list(depth),
            b'{' => self.object(depth),
            b'-' | b'0'..=b'9' => self.number(),
            b't' => self.word("true").map(|()| Node::Bool(true)),
            b'f' => self.word("false").map(|()| Node::Bool(false)),
            b'n' => self.word("null").map(|()| Node::Null),
            _ => self.fail(Fault::Token, self.at),
        }
    }

    fn word(&mut self, word: &str) -> Option<()> {
        for expected in word.encode_utf16() {
            if self.peek() != Some(expected) {
                return self.fail(Fault::Token, self.at);
            }
            self.at += 1;
        }
        Some(())
    }

    fn list(&mut self, depth: usize) -> Option<Node> {
        self.at += 1;
        let mut items = Vec::new();
        self.space();
        if self.is(b']') {
            self.at += 1;
            return Some(Node::List(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.space();
            if self.is(b',') {
                self.at += 1;
            } else if self.is(b']') {
                self.at += 1;
                return Some(Node::List(items));
            } else {
                return self.fail(
                    Fault::At("Expected ',' or ']' after array element"),
                    self.at,
                );
            }
        }
    }

    fn object(&mut self, depth: usize) -> Option<Node> {
        self.at += 1;
        let mut fields: Vec<(Vec<u16>, Node)> = Vec::new();
        self.space();
        if self.is(b'}') {
            self.at += 1;
            return Some(Node::Object(fields));
        }
        let mut expected = "Expected property name or '}'";
        loop {
            self.space();
            if !self.is(b'"') {
                return self.fail(Fault::At(expected), self.at);
            }
            let key = self.text()?;
            self.space();
            if !self.is(b':') {
                return self.fail(Fault::At("Expected ':' after property name"), self.at);
            }
            self.at += 1;
            let value = self.value(depth + 1)?;
            match fields.iter_mut().find(|(name, _)| *name == key) {
                Some(slot) => slot.1 = value,
                None => fields.push((key, value)),
            }
            self.space();
            if self.is(b',') {
                self.at += 1;
                expected = "Expected double-quoted property name";
            } else if self.is(b'}') {
                self.at += 1;
                return Some(Node::Object(fields));
            } else {
                return self.fail(
                    Fault::At("Expected ',' or '}' after property value"),
                    self.at,
                );
            }
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while self.peek().is_some_and(is_digit) {
            self.at += 1;
        }
        self.at - start
    }

    fn number(&mut self) -> Option<Node> {
        let start = self.at;
        if self.is(b'-') {
            self.at += 1;
            if !self.peek().is_some_and(is_digit) {
                return self.fail(Fault::At("No number after minus sign"), self.at);
            }
        }
        if self.is(b'0') {
            self.at += 1;
            if self.peek().is_some_and(is_digit) {
                return self.fail(Fault::Number, self.at);
            }
        } else {
            self.digits();
        }
        if self.is(b'.') {
            self.at += 1;
            if self.digits() == 0 {
                return self.fail(Fault::At("Unterminated fractional number"), self.at);
            }
        }
        if self.is(b'e') || self.is(b'E') {
            self.at += 1;
            if self.is(b'+') || self.is(b'-') {
                self.at += 1;
            }
            if self.digits() == 0 {
                return self.fail(Fault::At("Exponent part is missing a number"), self.at);
            }
        }
        let text = String::from_utf16_lossy(&self.units[start..self.at]);
        Some(Node::Number(text.parse().unwrap_or(f64::NAN)))
    }

    fn text(&mut self) -> Option<Vec<u16>> {
        self.at += 1;
        let mut out = Vec::new();
        loop {
            let Some(unit) = self.peek() else {
                return self.fail(Fault::At("Unterminated string"), self.at);
            };
            match unit {
                QUOTE => {
                    self.at += 1;
                    return Some(out);
                }
                SLASH => {
                    self.at += 1;
                    let Some(escape) = self.peek() else {
                        return self.fail(Fault::Token, self.at);
                    };
                    let simple = match u8::try_from(escape).unwrap_or(0) {
                        b'"' => Some(0x22),
                        b'\\' => Some(0x5c),
                        b'/' => Some(0x2f),
                        b'b' => Some(0x08),
                        b'f' => Some(0x0c),
                        b'n' => Some(0x0a),
                        b'r' => Some(0x0d),
                        b't' => Some(0x09),
                        b'u' => None,
                        _ => return self.fail(Fault::At("Bad escaped character"), self.at),
                    };
                    self.at += 1;
                    if let Some(simple) = simple {
                        out.push(simple);
                        continue;
                    }
                    let mut code: u16 = 0;
                    for _ in 0..4 {
                        let digit = self
                            .peek()
                            .and_then(|unit| char::from_u32(u32::from(unit)))
                            .and_then(|ch| ch.to_digit(16));
                        let Some(digit) = digit else {
                            return self.fail(Fault::At("Bad Unicode escape"), self.at);
                        };
                        code = code * 16 + u16::try_from(digit).unwrap_or(0);
                        self.at += 1;
                    }
                    out.push(code);
                }
                0..0x20 => {
                    return self.fail(
                        Fault::At("Bad control character in string literal"),
                        self.at,
                    );
                }
                _ => {
                    out.push(unit);
                    self.at += 1;
                }
            }
        }
    }

    /// Текст ошибки, как его даёт V8.
    fn message(&self) -> String {
        let (fault, at) = self.fault.unwrap_or((Fault::Token, self.at));
        let source = &self.units;
        let position = |text: &str| {
            let (mut line, mut start, mut index) = (1, 0, 0);
            while index < at.min(source.len()) {
                match source[index] {
                    0x0d if source.get(index + 1) == Some(&0x0a) => {
                        line += 1;
                        index += 2;
                        start = index;
                        continue;
                    }
                    0x0a | 0x0d => {
                        line += 1;
                        start = index + 1;
                    }
                    _ => {}
                }
                index += 1;
            }
            let place = if text.ends_with("after JSON") {
                ""
            } else {
                " in JSON"
            };
            format!(
                "{text}{place} at position {at} (line {line} column {})",
                at - start + 1
            )
        };
        match fault {
            Fault::At(text) => position(text),
            Fault::Number => position("Unexpected number"),
            Fault::TooDeep => "Maximum call stack size exceeded".to_owned(),
            Fault::Token => {
                let Some(&unit) = source.get(at) else {
                    return "Unexpected end of JSON input".to_owned();
                };
                // Символ, с которого начинается число или строка, V8 называет
                // по виду лексемы.
                if unit == QUOTE {
                    return position("Unexpected string");
                }
                if unit == u16::from(b'-') || is_digit(unit) {
                    return position("Unexpected number");
                }
                let whole = String::from_utf16_lossy(source);
                if matches!(
                    whole.as_str(),
                    "NaN" | "Infinity" | "undefined" | "[object Object]"
                ) {
                    return format!("\"{whole}\" is not valid JSON");
                }
                let token = String::from_utf16_lossy(&[unit]);
                let length = source.len();
                if length < MIN_LENGTH_FOR_CONTEXT {
                    return format!("Unexpected token '{token}', \"{whole}\" is not valid JSON");
                }
                let slice = |from: usize, to: usize| String::from_utf16_lossy(&source[from..to]);
                if at < CONTEXT {
                    format!(
                        "Unexpected token '{token}', \"{}\"... is not valid JSON",
                        slice(0, at + CONTEXT)
                    )
                } else if at < length - CONTEXT {
                    format!(
                        "Unexpected token '{token}', ...\"{}\"... is not valid JSON",
                        slice(at - CONTEXT, at + CONTEXT)
                    )
                } else {
                    format!(
                        "Unexpected token '{token}', ...\"{}\" is not valid JSON",
                        slice(at - CONTEXT, length)
                    )
                }
            }
        }
    }
}

/// Разбор как `JSON.parse`; ошибка — текст V8.
fn parse_node(text: &str) -> Result<Node, String> {
    let mut parser = Parser {
        units: text.encode_utf16().collect(),
        at: 0,
        fault: None,
    };
    let Some(node) = parser.value(0) else {
        return Err(parser.message());
    };
    parser.space();
    if parser.at != parser.units.len() {
        let at = parser.at;
        parser.fault = Some((
            Fault::At("Unexpected non-whitespace character after JSON"),
            at,
        ));
        return Err(parser.message());
    }
    Ok(node)
}

fn units_text(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

/// Проверка `secure-json-parse`, которой Fastify защищает тело запроса:
/// ключ `__proto__` или `constructor` с вложенным `prototype`.
fn poisoned(node: &Node) -> bool {
    match node {
        Node::List(items) => items.iter().any(poisoned),
        Node::Object(fields) => fields.iter().any(|(key, value)| {
            let key = units_text(key);
            key == "__proto__"
                || (key == "constructor"
                    && matches!(value, Node::Object(inner)
                        if inner.iter().any(|(name, _)| units_text(name) == "prototype")))
                || poisoned(value)
        }),
        _ => false,
    }
}

fn to_value(node: &Node) -> serde_json::Value {
    use serde_json::Value;
    match node {
        Node::Null => Value::Null,
        Node::Bool(flag) => Value::Bool(*flag),
        Node::Number(number) => {
            serde_json::Number::from_f64(*number).map_or(Value::Null, Value::Number)
        }
        Node::Text(units) => Value::String(units_text(units)),
        Node::List(items) => Value::Array(items.iter().map(to_value).collect()),
        Node::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (units_text(key), to_value(value)))
                .collect(),
        ),
    }
}

/// Разбор тела запроса, как его делает Fastify: значение и запись
/// `JSON.stringify(значения)`. Ошибка — текст, который вернул бы Fastify.
///
/// Значение по возможности берётся из `serde_json` (с его представлением
/// чисел); то, что принимает только V8 (одиночные суррогаты, очень большие
/// показатели), строится из собственного разбора.
///
/// # Errors
/// Тело не JSON или содержит запрещённые ключи.
pub fn parse_body(bytes: &[u8]) -> Result<(serde_json::Value, String), String> {
    let text = String::from_utf8_lossy(bytes);
    let node = parse_node(&text)?;
    if poisoned(&node) {
        return Err("Object contains forbidden prototype property".to_owned());
    }
    let mut out = String::with_capacity(bytes.len());
    write_node(&mut out, &node);
    let value = serde_json::from_str(&text).unwrap_or_else(|_| to_value(&node));
    Ok((value, out))
}

/// Индекс массива в смысле JavaScript: такие ключи идут первыми по возрастанию.
fn array_index(key: &[u16]) -> Option<u32> {
    if key.is_empty() || key.len() > 10 || (key.len() > 1 && key[0] == 0x30) {
        return None;
    }
    let mut value: u64 = 0;
    for unit in key {
        let digit = u64::from(*unit).checked_sub(0x30).filter(|d| *d <= 9)?;
        value = value * 10 + digit;
    }
    u32::try_from(value).ok().filter(|index| *index < u32::MAX)
}

/// Число так, как его печатает JavaScript (`Number::toString`).
#[must_use]
pub fn number(value: f64) -> String {
    if !value.is_finite() {
        return "null".to_owned();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let count = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    let point = exponent + 1;
    let mut out = String::new();
    if value < 0.0 {
        out.push('-');
    }
    let zeros = |n: i32| "0".repeat(usize::try_from(n).unwrap_or(0));
    if count <= point && point <= 21 {
        out.push_str(&digits);
        out.push_str(&zeros(point - count));
    } else if 0 < point && point <= 21 {
        let split = usize::try_from(point).unwrap_or(0);
        out.push_str(&digits[..split]);
        out.push('.');
        out.push_str(&digits[split..]);
    } else if -6 < point && point <= 0 {
        out.push_str("0.");
        out.push_str(&zeros(-point));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exponent < 0 { '-' } else { '+' });
        out.push_str(&exponent.unsigned_abs().to_string());
    }
    out
}

fn write_text(out: &mut String, units: &[u16]) {
    out.push('"');
    for item in char::decode_utf16(units.iter().copied()) {
        match item {
            Ok('"') => out.push_str("\\\""),
            Ok('\\') => out.push_str("\\\\"),
            Ok('\u{8}') => out.push_str("\\b"),
            Ok('\u{c}') => out.push_str("\\f"),
            Ok('\n') => out.push_str("\\n"),
            Ok('\r') => out.push_str("\\r"),
            Ok('\t') => out.push_str("\\t"),
            Ok(ch) if u32::from(ch) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(ch));
            }
            Ok(ch) => out.push(ch),
            Err(lone) => {
                let _ = write!(out, "\\u{:04x}", lone.unpaired_surrogate());
            }
        }
    }
    out.push('"');
}

fn write_node(out: &mut String, node: &Node) {
    match node {
        Node::Null => out.push_str("null"),
        Node::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Node::Number(value) => out.push_str(&number(*value)),
        Node::Text(units) => write_text(out, units),
        Node::List(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_node(out, item);
            }
            out.push(']');
        }
        Node::Object(fields) => {
            let mut indexed: Vec<(u32, &(Vec<u16>, Node))> = fields
                .iter()
                .filter_map(|field| array_index(&field.0).map(|index| (index, field)))
                .collect();
            indexed.sort_by_key(|(index, _)| *index);
            let named = fields
                .iter()
                .filter(|field| array_index(&field.0).is_none());
            out.push('{');
            for (position, (key, value)) in indexed
                .into_iter()
                .map(|(_, field)| field)
                .chain(named)
                .enumerate()
            {
                if position > 0 {
                    out.push(',');
                }
                write_text(out, key);
                out.push(':');
                write_node(out, value);
            }
            out.push('}');
        }
    }
}

/// `JSON.stringify(JSON.parse(text))`. `None` — текст не является JSON.
#[must_use]
pub fn restringify(text: &[u8]) -> Option<String> {
    let node = parse_node(std::str::from_utf8(text).ok()?).ok()?;
    let mut out = String::with_capacity(text.len());
    write_node(&mut out, &node);
    Some(out)
}

/// Запись значения, которую `JSON.stringify(JSON.parse(…))` оставит как есть:
/// в таком виде уходят подписанные тела запросов.
#[must_use]
pub fn stringify(value: &serde_json::Value) -> String {
    let text = value.to_string();
    restringify(text.as_bytes()).unwrap_or(text)
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    fn same(input: &str, expected: &str) {
        assert_eq!(
            restringify(input.as_bytes()).as_deref(),
            Some(expected),
            "{input}"
        );
    }

    #[test]
    fn numbers_follow_javascript() {
        // Значения сверены с Node.js.
        for (value, text) in [
            (1.0, "1"),
            (-0.0, "0"),
            (1.5, "1.5"),
            (100.0, "100"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (123_456_789_012_345_680_000.0, "123456789012345680000"),
            (0.000_001, "0.000001"),
            (0.000_000_1, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (-2.5e22, "-2.5e+22"),
            (0.1 + 0.2, "0.30000000000000004"),
            (9_007_199_254_740_993.0, "9007199254740992"),
            (f64::INFINITY, "null"),
        ] {
            assert_eq!(number(value), text);
        }
    }

    #[test]
    fn keys_keep_javascript_order() {
        same(
            r#"{"b":1,"10":2,"a":3,"2":4,"b":5,"01":6}"#,
            r#"{"2":4,"10":2,"b":5,"a":3,"01":6}"#,
        );
    }

    #[test]
    fn whitespace_numbers_and_escapes_are_rewritten() {
        same(
            "{ \"a\" : [ 1.0 , 2e3, -0 ],\n\"b\":\"\\u0041\\/\\u001f\\n\" }",
            r#"{"a":[1,2000,0],"b":"A/\u001f\n"}"#,
        );
        same(r#""\ud83d\ude00 \ud800""#, "\"\u{1f600} \\ud800\"");
        same("\"\u{2028}é\"", "\"\u{2028}é\"");
        same("1e999", "null");
    }

    #[test]
    fn parse_matches_v8_on_a_corpus() {
        // Вход, «ok» с записью JSON.stringify или «error» с текстом ошибки V8;
        // корпус снят с Node.js 22 (JSON.parse), часть входов — случайные
        // искажения правильных документов.
        // Строки записаны единицами UTF-16: в тексты ошибок V8 попадают
        // половинки суррогатных пар.
        let cases: Vec<(Vec<u16>, String, Vec<u16>)> =
            serde_json::from_str(include_str!("../tests/data/v8_json.json")).expect("корпус");
        assert!(cases.len() > 300);
        for (input, kind, expected) in cases {
            let input = String::from_utf16(&input).expect("вход без одиночных суррогатов");
            let expected = String::from_utf16_lossy(&expected);
            let got = parse_node(&input).map(|node| {
                let mut out = String::new();
                write_node(&mut out, &node);
                out
            });
            match kind.as_str() {
                "ok" => assert_eq!(got.as_ref().ok(), Some(&expected), "{input:?}"),
                _ => assert_eq!(got.as_ref().err(), Some(&expected), "{input:?}"),
            }
        }
    }

    #[test]
    fn prototype_keys_are_refused_like_fastify() {
        for body in [
            r#"{"__proto__":1}"#,
            r#"{"a":[{"constructor":{"prototype":1}}]}"#,
            r#"{"_\u005fproto__":{}}"#,
        ] {
            assert_eq!(
                parse_body(body.as_bytes()).err().as_deref(),
                Some("Object contains forbidden prototype property"),
                "{body}"
            );
        }
        for body in [
            r#"{"constructor":{"x":1}}"#,
            r#"["__proto__"]"#,
            r#"{"constructor":1}"#,
        ] {
            assert!(parse_body(body.as_bytes()).is_ok(), "{body}");
        }
        // Одиночный суррогат принимает только V8: значение строится своим разбором.
        let (value, text) = parse_body(br#"{"a":"\ud800","n":1e999}"#).expect("разбор");
        assert_eq!(text, r#"{"a":"\ud800","n":null}"#);
        assert_eq!(value["a"], "\u{fffd}");
    }

    #[test]
    fn invalid_json_is_refused() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "01",
            "1.",
            "'a'",
            "\"\t\"",
            "nul",
            "{} x",
            "\"\\x\"",
        ] {
            assert!(restringify(bad.as_bytes()).is_none(), "{bad}");
        }
    }

    #[test]
    fn own_output_is_a_fixed_point() {
        let value = serde_json::json!({"z": 1, "5": [1.5, "x\u{7}"], "a": {"10": null, "9": true}});
        let text = stringify(&value);
        assert_eq!(
            text,
            r#"{"5":[1.5,"x\u0007"],"a":{"9":true,"10":null},"z":1}"#
        );
        assert_eq!(restringify(text.as_bytes()).as_deref(), Some(text.as_str()));
    }
}
