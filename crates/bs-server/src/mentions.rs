//! Упоминания в тексте сообщения (`shared/mentions.ts`).
//!
//! Упоминания внутри кода (в обратных кавычках) не считаются. `@everyone` и
//! `@here` считаются, только если стоят отдельным словом.

use std::collections::BTreeSet;

/// Что упомянуто в сообщении.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Mentions {
    /// Пользователи (`<@id>`).
    pub users: BTreeSet<String>,
    /// Роли (`<@&id>`), в порядке первого появления.
    pub roles: Vec<String>,
    /// `@everyone` или `@here`.
    pub everyone: bool,
}

const fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

const fn is_id(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Длина идентификатора в начале `text`, если сразу за ним стоит `>`.
fn id_then_close(text: &str) -> Option<usize> {
    let len = text.chars().take_while(|c| is_id(*c)).count();
    (len > 0 && text[len..].starts_with('>')).then_some(len)
}

/// Разбирает текст сообщения.
#[must_use]
pub fn parse(content: Option<&str>) -> Mentions {
    let mut found = Mentions::default();
    let Some(content) = content else { return found };
    let mut index = 0;
    let mut previous: Option<char> = None;
    while index < content.len() {
        let rest = &content[index..];
        // Блок кода: до ближайших закрывающих трёх кавычек.
        if let Some(after) = rest.strip_prefix("```")
            && let Some(end) = after.find("```")
        {
            index += 3 + end + 3;
            previous = Some('`');
            continue;
        }
        // Код в строке: хотя бы один знак между кавычками.
        if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find('`')
            && end > 0
        {
            index += 1 + end + 1;
            previous = Some('`');
            continue;
        }
        if let Some(after) = rest.strip_prefix("<@&")
            && let Some(len) = id_then_close(after)
        {
            let id = &after[..len];
            if !found.roles.iter().any(|known| known == id) {
                found.roles.push(id.to_owned());
            }
            index += 3 + len + 1;
            previous = Some('>');
            continue;
        }
        if let Some(after) = rest.strip_prefix("<@")
            && let Some(len) = id_then_close(after)
        {
            found.users.insert(after[..len].to_owned());
            index += 2 + len + 1;
            previous = Some('>');
            continue;
        }
        if let Some(after) = rest.strip_prefix('@')
            && !previous.is_some_and(|c| is_word(c) || c == '@')
        {
            let keyword = ["everyone", "here"].into_iter().find(|keyword| {
                after.starts_with(keyword)
                    && !after[keyword.len()..]
                        .chars()
                        .next()
                        .is_some_and(|c| is_word(c) || c == '-')
            });
            if let Some(keyword) = keyword {
                found.everyone = true;
                index += 1 + keyword.len();
                previous = keyword.chars().last();
                continue;
            }
        }
        let Some(c) = rest.chars().next() else { break };
        index += c.len_utf8();
        previous = Some(c);
    }
    found
}

/// Есть ли массовое упоминание: всех или роли.
#[must_use]
pub fn has_mass(content: Option<&str>) -> bool {
    let found = parse(content);
    found.everyone || !found.roles.is_empty()
}

/// Добавляет ли правка массовое упоминание, которого не было.
#[must_use]
pub fn adds_mass(before: Option<&str>, after: Option<&str>) -> bool {
    let old = parse(before);
    let new = parse(after);
    (new.everyone && !old.everyone) || new.roles.iter().any(|role| !old.roles.contains(role))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_mentions_outside_code() {
        let found = parse(Some(
            "hi <@1> and <@&22> @everyone `@here <@3>` ```<@&4>```",
        ));
        assert!(found.users.contains("1") && found.users.len() == 1);
        assert_eq!(found.roles, vec!["22"]);
        assert!(found.everyone);
    }

    #[test]
    fn keyword_must_stand_alone() {
        assert!(!parse(Some("mail@everyone")).everyone);
        assert!(!parse(Some("@@everyone")).everyone);
        assert!(!parse(Some("@everyone-else")).everyone);
        assert!(!parse(Some("@everyonex")).everyone);
        assert!(parse(Some("(@here)")).everyone);
        assert!(
            parse(Some("привет@here")).everyone,
            "как в JS: \\w только ASCII"
        );
        assert!(parse(Some("@everyone.")).everyone);
    }

    #[test]
    fn unclosed_code_does_not_hide_mentions() {
        assert!(parse(Some("``` @everyone")).everyone);
        assert!(parse(Some("`` @here")).everyone);
        assert!(!parse(Some("`x @here`")).everyone);
    }

    #[test]
    fn edits_adding_mass_mentions() {
        assert!(adds_mass(Some("a"), Some("a @everyone")));
        assert!(!adds_mass(Some("@here"), Some("@everyone")));
        assert!(adds_mass(Some("<@&1>"), Some("<@&1> <@&2>")));
        assert!(!adds_mass(Some("<@&1>"), Some("<@&1>")));
        assert!(!has_mass(Some("<@1>")));
    }
}
