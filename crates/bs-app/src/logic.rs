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
    } else if ready && matches!(current, Screen::Login) {
        // Внутри сессии экран выбирает пользователь (чат ↔ социальное).
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

/// Можно ли править или удалять это сообщение.
///
/// Править и удалять вправе только автор; сравнение идёт по id
/// текущего пользователя, а не по имени.
pub(crate) fn can_manage_message(author_id: &str, me: &str) -> bool {
    author_id == me
}

/// Эмодзи, который ставит кнопка быстрой реакции.
pub(crate) const DEFAULT_REACTION: &str = "👍";

/// Что показать в контекстном меню сообщения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MessageActions {
    /// Показывать «Изменить» и «Удалить».
    pub(crate) manage: bool,
    /// Показывать «Реакция».
    pub(crate) react: bool,
}

/// Набор действий для сообщения: реакции доступны всегда, правка — автору.
pub(crate) fn message_actions(author_id: &str, me: &str) -> MessageActions {
    MessageActions {
        manage: can_manage_message(author_id, me),
        react: true,
    }
}

/// Стоит ли отправлять `ack` за этот канал.
///
/// GUI помнит последнюю подтверждённую пару; повторно то же самое не шлём,
/// иначе каждый кадр улетал бы лишний `ack`.
pub(crate) fn should_ack(last: Option<(&str, &str)>, channel: &str, message: &str) -> bool {
    last != Some((channel, message))
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

    use super::{
        author_name, can_manage_message, can_send, connection_label, message_actions, next_screen,
        outgoing_text, should_ack, should_wake,
    };
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
        assert_eq!(next_screen(Screen::Social, false, false), Screen::Social);
    }

    #[test]
    fn next_screen_keeps_user_choice_within_session() {
        // Повторный `ready` не должен выдёргивать пользователя из соцэкрана.
        assert_eq!(next_screen(Screen::Social, true, false), Screen::Social);
        assert_eq!(next_screen(Screen::Chat, true, false), Screen::Chat);
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
    fn only_author_can_manage_message() {
        assert!(can_manage_message("u1", "u1"));
        assert!(!can_manage_message("u2", "u1"));
        assert!(!can_manage_message("", "u1"), "пустой автор — не мы");
    }

    #[test]
    fn message_actions_follow_ownership() {
        assert!(message_actions("u1", "u1").manage);
        assert!(!message_actions("u2", "u1").manage);
        // Реакции доступны на любое сообщение.
        assert!(message_actions("u2", "u1").react);
    }

    #[test]
    fn should_ack_only_on_change() {
        assert!(should_ack(None, "c1", "m1"));
        assert!(!should_ack(Some(("c1", "m1")), "c1", "m1"));
        assert!(
            should_ack(Some(("c1", "m1")), "c1", "m2"),
            "новое сообщение"
        );
        assert!(should_ack(Some(("c1", "m1")), "c2", "m1"), "другой канал");
    }

    #[test]
    fn should_wake_while_busy_or_with_cache() {
        assert!(should_wake(true, false), "идёт логин");
        assert!(should_wake(false, true), "есть кэш");
        assert!(should_wake(true, true));
        assert!(!should_wake(false, false), "нечего ждать — можно спать");
    }
}
