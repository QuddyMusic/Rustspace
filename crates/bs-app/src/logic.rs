//! Чистая логика GUI, вынесенная из `show()`-методов.
//!
//! Всё здесь — обычные функции без `egui` и без сети: их легко покрыть
//! юнит-тестами, тогда как сами экраны остаются тонкими обёртками.

use bs_instances::ConnectionState;
use bs_types::ChannelId;

use crate::app::Screen;

/// Отображаемое имя автора сообщения: `display_name`, иначе `username`.
///
/// Возвращает `&str` из переданных полей, чтобы не клонировать строку
/// на каждый кадр отрисовки.
pub(crate) fn author_name<'a>(display_name: Option<&'a str>, username: &'a str) -> &'a str {
    display_name.unwrap_or(username)
}

/// Можно ли отправить сообщение в текущем состоянии экрана.
///
/// Нужен выбранный канал и непустой (после `trim`) черновик.
pub(crate) fn can_send(channel: Option<&ChannelId>, draft: &str) -> bool {
    channel.is_some() && !draft.trim().is_empty()
}

/// Текст для отправки: черновик без ведущих и хвостовых пробелов.
///
/// Возвращает `None`, если после `trim` ничего не осталось.
pub(crate) fn outgoing_text(draft: &str) -> Option<String> {
    let trimmed = draft.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Следующий экран после обработки событий моста.
///
/// Приоритеты: потеря сессии всегда возвращает на логин (даже из чата),
/// иначе первый `ready` открывает чат, иначе экран не меняется.
pub(crate) const fn next_screen(current: Screen, ready: bool, session_lost: bool) -> Screen {
    if session_lost {
        Screen::Login
    } else if ready {
        Screen::Chat
    } else {
        current
    }
}

/// Человекочитаемая подпись состояния соединения для строки статуса.
///
/// Возвращает `String`, потому что для `Reconnecting` в подпись входит
/// номер попытки.
pub(crate) fn connection_label(state: &ConnectionState) -> String {
    match state {
        ConnectionState::Connecting => "подключение…".to_owned(),
        ConnectionState::Connected => "на связи".to_owned(),
        ConnectionState::Reconnecting { attempt } => {
            format!("переподключение (попытка {attempt})…")
        }
        ConnectionState::Stopped => "соединение остановлено".to_owned(),
    }
}

/// Нужно ли будить UI, пока идёт логин или уже есть живой кэш.
///
/// Без этого окно засыпало бы и не перерисовывалось при приходе событий
/// из фонового tokio-потока.
pub(crate) const fn should_wake(busy: bool, has_cache: bool) -> bool {
    busy || has_cache
}

#[cfg(test)]
mod tests {
    use bs_instances::ConnectionState;
    use bs_types::ChannelId;

    use super::{author_name, can_send, connection_label, next_screen, outgoing_text, should_wake};
    use crate::app::Screen;

    #[test]
    fn author_name_prefers_display_name() {
        assert_eq!(author_name(Some("Alice"), "alice42"), "Alice");
    }

    #[test]
    fn author_name_falls_back_to_username() {
        assert_eq!(author_name(None, "alice42"), "alice42");
    }

    #[test]
    fn author_name_keeps_empty_display_name() {
        // Пустая строка — валидное значение поля, не повод подменять его.
        assert_eq!(author_name(Some(""), "alice42"), "");
    }

    #[test]
    fn can_send_requires_channel_and_text() {
        let channel = ChannelId::from("chan-1");
        assert!(can_send(Some(&channel), "привет"));
        assert!(!can_send(None, "привет"), "без канала отправлять некуда");
        assert!(!can_send(Some(&channel), ""), "пустой черновик");
        assert!(!can_send(Some(&channel), "   \n\t "), "пробелы — не текст");
    }

    #[test]
    fn outgoing_text_trims() {
        assert_eq!(outgoing_text("  привет  ").as_deref(), Some("привет"));
        assert_eq!(outgoing_text("\n\tпривет\n").as_deref(), Some("привет"));
    }

    #[test]
    fn outgoing_text_rejects_blank() {
        assert_eq!(outgoing_text(""), None);
        assert_eq!(outgoing_text("   "), None);
        assert_eq!(outgoing_text("\n\t"), None);
    }

    #[test]
    fn outgoing_text_keeps_inner_spaces() {
        assert_eq!(outgoing_text(" a  b ").as_deref(), Some("a  b"));
    }

    #[test]
    fn next_screen_moves_to_chat_on_ready() {
        assert_eq!(next_screen(Screen::Login, true, false), Screen::Chat);
    }

    #[test]
    fn next_screen_stays_without_ready() {
        assert_eq!(next_screen(Screen::Login, false, false), Screen::Login);
        assert_eq!(next_screen(Screen::Chat, false, false), Screen::Chat);
    }

    #[test]
    fn next_screen_returns_to_login_when_session_lost() {
        assert_eq!(next_screen(Screen::Chat, false, true), Screen::Login);
        // Потеря сессии важнее одновременного `ready`.
        assert_eq!(next_screen(Screen::Chat, true, true), Screen::Login);
    }

    #[test]
    fn connection_label_covers_all_states() {
        assert_eq!(
            connection_label(&ConnectionState::Connecting),
            "подключение…"
        );
        assert_eq!(connection_label(&ConnectionState::Connected), "на связи");
        assert_eq!(
            connection_label(&ConnectionState::Reconnecting { attempt: 3 }),
            "переподключение (попытка 3)…"
        );
        assert_eq!(
            connection_label(&ConnectionState::Stopped),
            "соединение остановлено"
        );
    }

    #[test]
    fn should_wake_while_busy_or_with_cache() {
        assert!(should_wake(true, false), "идёт логин");
        assert!(should_wake(false, true), "есть кэш");
        assert!(should_wake(true, true));
        assert!(!should_wake(false, false), "нечего ждать — можно спать");
    }
}
