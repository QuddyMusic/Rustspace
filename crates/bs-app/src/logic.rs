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

/// Понятный текст отказа сервера на действие пользователя.
///
/// Известные коды получают русскую формулировку. Неизвестный код показывает
/// текст сервера, но очищенный от управляющих символов и обрезанный: он
/// приходит с чужого инстанса и не должен ломать интерфейс.
pub(crate) fn describe_server_error(code: Option<&str>, message: &str) -> String {
    let known = match code {
        Some("rate_limited") => "Слишком часто: подождите несколько секунд",
        Some("content_too_long") => "Сообщение слишком длинное",
        Some("content_required") => "Нельзя отправить пустое сообщение",
        Some("not_dm_member") => "Вы не участник этого личного канала",
        Some("recipient_deleted") => "Собеседник удалён: переписка доступна только для чтения",
        Some("missing_permission") => "Недостаточно прав",
        Some("not_message_author") => "Править и удалять можно только свои сообщения",
        Some("system_message_immutable") => "Системное сообщение изменить нельзя",
        Some("reply_target_invalid") => "На это сообщение нельзя ответить",
        Some("attachment_invalid" | "attachment_not_owned") => "Вложение недоступно",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_owned();
    }
    let cleaned: String = message
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect();
    if cleaned.trim().is_empty() {
        "Сервер отклонил действие".to_owned()
    } else {
        cleaned
    }
}

/// Расстояние до верха ленты (px), с которого подгружается более старая страница.
pub(crate) const LOAD_OLDER_THRESHOLD_PX: f32 = 40.0;

/// Пора ли подгрузить более старые сообщения.
///
/// Если вся лента помещается в окно, листать нечего — догружаем, пока не
/// заполнится экран. Иначе нужен реальный скролл: сразу после первой
/// страницы лента ещё не прижата к низу (`offset` = 0), и без флага
/// `was_scrolled_down` пользователь открывал бы канал уже посреди истории.
pub(crate) fn should_load_older(
    offset_y: f32,
    content_h: f32,
    viewport_h: f32,
    was_scrolled_down: bool,
) -> bool {
    if content_h <= viewport_h {
        return true;
    }
    was_scrolled_down && offset_y < LOAD_OLDER_THRESHOLD_PX
}

#[cfg(test)]
mod tests {
    use bs_instances::ConnectionState;
    use bs_types::ChannelId;

    use super::{
        author_name, can_manage_message, can_send, connection_label, describe_server_error,
        message_actions, next_screen, outgoing_text, should_ack, should_load_older, should_wake,
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
    }

    #[test]
    fn next_screen_keeps_user_choice_within_session() {
        // Повторный `ready` не должен выдёргивать пользователя из соцэкрана.
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

    #[test]
    fn load_older_when_content_fits_viewport() {
        assert!(should_load_older(0.0, 100.0, 400.0, false));
    }

    #[test]
    fn load_older_needs_real_scroll_for_long_content() {
        // Первый кадр после загрузки: offset ещё 0, но вниз не листали.
        assert!(!should_load_older(0.0, 2000.0, 400.0, false));
        // Листали вниз, а затем вернулись к верху.
        assert!(should_load_older(10.0, 2000.0, 400.0, true));
        // Читаем середину: подгружать рано.
        assert!(!should_load_older(500.0, 2000.0, 400.0, true));
    }

    #[test]
    fn server_error_known_codes_are_localised() {
        assert_eq!(
            describe_server_error(Some("rate_limited"), "Too many"),
            "Слишком часто: подождите несколько секунд"
        );
        assert_eq!(
            describe_server_error(Some("not_dm_member"), "x"),
            "Вы не участник этого личного канала"
        );
    }

    #[test]
    fn server_error_unknown_code_is_sanitised_and_truncated() {
        let shown = describe_server_error(None, "плохо\u{1b}[31m\n\u{7}");
        assert!(!shown.chars().any(char::is_control), "{shown:?}");
        let long = "x".repeat(1000);
        assert_eq!(
            describe_server_error(Some("weird"), &long).chars().count(),
            200
        );
        assert_eq!(
            describe_server_error(None, "\u{1b}\u{7}"),
            "Сервер отклонил действие"
        );
    }
}
