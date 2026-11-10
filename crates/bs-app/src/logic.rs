//! Чистая логика GUI, вынесенная из `show()`-методов.
//!
//! Всё здесь — обычные функции без `egui` и без сети: их легко покрыть
//! юнит-тестами, тогда как сами экраны остаются тонкими обёртками.

use bs_instances::ConnectionState;

use crate::app::Screen;
use crate::i18n::tr;

/// Отображаемое имя автора сообщения: `display_name`, иначе `username`.
///
/// Возвращает `&str` из переданных полей, чтобы не клонировать строку
/// на каждый кадр отрисовки.
pub(crate) fn author_name<'a>(display_name: Option<&'a str>, username: &'a str) -> &'a str {
    display_name.unwrap_or(username)
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

/// Предел длины сообщения на сервере (`MAX_MESSAGE_LENGTH` оригинала).
///
/// Сервер меряет `content.length` в JavaScript, то есть в единицах UTF-16.
pub(crate) const MAX_MESSAGE_LENGTH: usize = 4000;

/// Длина строки так, как её считает сервер (UTF-16).
#[must_use]
pub(crate) fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Ограда блока кода (` ``` ` в начале строки, можно с отступом и языком).
fn fence(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

/// Режет длинный текст на сообщения не длиннее `limit` (в UTF-16).
///
/// Режем по строкам; строку длиннее предела — по пробелу, а если его нет,
/// то по символам. Если разрыв приходится внутрь блока кода, текущая часть
/// закрывается ` ``` `, а следующая снова открывается той же оградой
/// (с языком), так что каждая часть — корректный Markdown.
#[must_use]
pub(crate) fn split_message(text: &str, limit: usize) -> Vec<String> {
    if js_len(text) <= limit {
        return vec![text.to_owned()];
    }
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut open: Option<String> = None;
    let flush = |current: &mut String, parts: &mut Vec<String>, open: &Option<String>| {
        if open.is_some() {
            if !current.ends_with('\n') {
                current.push('\n');
            }
            current.push_str("```");
        }
        let part = current.trim_end_matches('\n').to_owned();
        if !part.trim().is_empty() {
            parts.push(part);
        }
        current.clear();
        if let Some(line) = open {
            current.push_str(line);
            current.push('\n');
        }
    };
    for line in text.split_inclusive('\n') {
        let mut rest = line;
        while !rest.is_empty() {
            // Внутри кода оставляем место под закрывающую ограду.
            let reserve = if open.is_some() { 4 } else { 0 };
            let room = limit.saturating_sub(js_len(&current) + reserve);
            if js_len(rest) <= room {
                current.push_str(rest);
                break;
            }
            let fresh = open.as_ref().map_or(0, |line| js_len(line) + 1);
            if js_len(&current) > fresh {
                // Строка не влезает в начатую часть: переносим её целиком.
                flush(&mut current, &mut parts, &open);
                continue;
            }
            // Строка длиннее целой части: режем её саму.
            let cut = cut_point(rest, room.max(1));
            current.push_str(&rest[..cut]);
            rest = &rest[cut..];
            flush(&mut current, &mut parts, &open);
        }
        if fence(line) {
            open = if open.is_some() {
                None
            } else {
                Some(line.trim_end_matches(['\n', '\r']).to_owned())
            };
        }
    }
    // Хвост: незакрытый пользователем блок не закрываем за него.
    let tail = current.trim_end_matches('\n');
    if !tail.trim().is_empty() && tail != open.as_deref().unwrap_or_default() {
        parts.push(tail.to_owned());
    }
    parts
}

/// Байтовая позиция, до которой `text` укладывается в `room` единиц UTF-16:
/// по последнему пробелу, если он есть, иначе по границе символа.
fn cut_point(text: &str, room: usize) -> usize {
    let mut used = 0;
    let mut end = 0;
    let mut space = None;
    for (index, ch) in text.char_indices() {
        let width = ch.len_utf16();
        if used + width > room {
            break;
        }
        used += width;
        end = index + ch.len_utf8();
        if ch == ' ' {
            space = Some(end);
        }
    }
    match space {
        Some(at) if at > end / 2 => at,
        _ if end == 0 => text.chars().next().map_or(text.len(), char::len_utf8),
        _ => end,
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
        ConnectionState::Connecting => tr("подключение…").to_owned(),
        ConnectionState::Connected => tr("на связи").to_owned(),
        ConnectionState::Reconnecting { attempt } => {
            crate::tf!("переподключение (попытка {attempt})…", attempt = attempt)
        }
        ConnectionState::Stopped => tr("соединение остановлено").to_owned(),
    }
}

/// Можно ли править или удалять это сообщение.
///
/// Править и удалять вправе только автор; сравнение идёт по id
/// текущего пользователя, а не по имени.
pub(crate) fn can_manage_message(author_id: &str, me: &str) -> bool {
    author_id == me
}

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
        Some("rate_limited") => tr("Слишком часто: подождите несколько секунд"),
        Some("content_too_long") => tr("Сообщение слишком длинное"),
        Some("content_required") => tr("Нельзя отправить пустое сообщение"),
        Some("group_dm_too_few_members") => {
            tr("В группе должно быть не менее двух других участников")
        }
        Some("group_dm_too_many_members") => tr("В группе может быть не более 10 участников"),
        Some("group_dm_includes_self") => tr("Вы добавляетесь в группу автоматически"),
        Some("dm_not_found") => tr("Беседа не найдена"),
        Some("dm_not_group") => tr("Это возможно только в групповой беседе"),
        Some("dm_owner_only") => tr("Это может сделать только владелец группы"),
        Some("group_dm_name_length") => tr("Название группы: от 1 до 50 символов"),
        Some("owner_cannot_kick_self") => {
            tr("Владелец не может удалить сам себя. Вместо этого покиньте группу")
        }
        Some("target_not_dm_member") => tr("Этот пользователь не участник беседы"),
        Some("new_owner_required") => tr("Выберите, кто станет новым владельцем"),
        Some("already_owner") => tr("Этот пользователь уже владелец группы"),
        Some("not_dm_member") => tr("Вы не участник этого личного канала"),
        Some("recipient_deleted") => tr("Собеседник удалён: переписка доступна только для чтения"),
        Some("missing_permission") => tr("Недостаточно прав"),
        Some("not_message_author") => tr("Править и удалять можно только свои сообщения"),
        Some("system_message_immutable") => tr("Системное сообщение изменить нельзя"),
        Some("reply_target_invalid") => tr("На это сообщение нельзя ответить"),
        Some("attachment_invalid" | "attachment_not_owned") => tr("Вложение недоступно"),
        Some("user_banned") => tr("Вы заблокированы в этом пространстве"),
        Some("already_member") => tr("Вы уже участник этого пространства"),
        Some("join_request_required") => {
            tr("Это пространство принимает по заявке: найдите его в «Обзоре»")
        }
        Some("join_request_pending") => tr("Заявка на вступление уже отправлена"),
        Some("invite_not_found" | "invalid_invite") => tr("Приглашение не найдено или устарело"),
        Some("role_name_taken") => tr("Роль с таким названием уже существует."),
        Some("role_hierarchy") => {
            tr("Это можно делать только с участниками и ролями ниже вашей самой высокой роли.")
        }
        Some("role_permissions_conflict") => {
            tr("Кто-то другой изменил права этой роли. Проверьте их и сохраните ещё раз.")
        }
        Some("permissions_invalid") => tr("Недопустимое значение прав."),
        Some("role_name_required") => tr("Укажите название роли."),
        Some("cannot_change_own_roles") => tr("Вы не можете менять собственные роли."),
        Some("everyone_role_not_assignable") => {
            tr("Роль @everyone действует для всех, её нельзя назначить.")
        }
        Some("everyone_role_not_deletable") => tr("Роль @everyone нельзя удалить."),
        Some("role_not_in_space") => tr("Эта роль не принадлежит этому пространству."),
        Some("member_not_found") => tr("Участник не найден."),
        Some("space_owner_only") => tr("Это может сделать только владелец пространства."),
        Some("cannot_grant_unowned_permissions") => {
            tr("Нельзя выдать права, которых нет у вас самих.")
        }
        Some("cannot_deny_unowned_permissions") => {
            tr("Нельзя запретить права, которых нет у вас самих.")
        }
        Some("cannot_change_unowned_permissions") => {
            tr("Изменять можно только те права, которые есть у вас самих.")
        }
        Some("overrides_conflict") => {
            tr("Кто-то другой изменил эти права. Проверьте их и сохраните ещё раз.")
        }
        Some("override_bits_invalid") => tr("Значения прав указаны неверно."),
        Some("override_target_invalid") => {
            tr("Переопределение прав применяется к роли или к участнику.")
        }
        Some("override_target_required") => tr("Выберите роль или участника для переопределения."),
        Some("channel_not_found") => tr("Этот канал не найден."),
        Some("category_not_found") => tr("Эта категория не найдена."),
        Some("category_not_in_space") => tr("Эта категория не относится к данному пространству."),
        Some("channel_name_required") => tr("Укажите название канала."),
        Some("category_name_required") => tr("Укажите название категории."),
        Some("channel_name_length") => tr("Название канала должно содержать от 1 до 100 символов."),
        Some("category_name_length") => {
            tr("Название категории должно содержать от 1 до 100 символов.")
        }
        Some("channel_topic_length") => tr("Тема канала не может быть длиннее 1024 символов."),
        Some("no_fields_to_update") => tr("Нечего сохранять."),
        Some("owner_cannot_leave") => tr("Владелец не может покинуть пространство"),
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
        tr("Сервер отклонил действие").to_owned()
    } else {
        cleaned
    }
}

/// Текст отказа регистрации нового аккаунта.
pub(crate) fn register_error_text(err: &bs_api::ApiError) -> String {
    if err.is_rate_limited() {
        return tr("Слишком много попыток регистрации: подождите пару минут").to_owned();
    }
    let known = match err.code() {
        Some("username_taken") => tr("Это имя уже занято"),
        Some("username_length_invalid") => tr("Имя: от 3 до 32 символов"),
        Some("username_characters_invalid") => {
            tr("В имени допустимы только латинские буквы, цифры и подчёркивание")
        }
        Some("username_required") => tr("Укажите имя пользователя"),
        Some("password_required") => tr("Укажите пароль"),
        Some("password_too_short") => tr("Пароль слишком короткий: минимум 8 символов"),
        Some("registration_closed") => tr("Регистрация на этом инстансе закрыта"),
        Some("invite_required") => {
            tr("Регистрация только по приглашению: вставьте ссылку или код приглашения")
        }
        Some("invite_invalid") => tr("Приглашение недействительно, истекло или уже использовано"),
        _ => "",
    };
    if !known.is_empty() {
        return known.to_owned();
    }
    err.body().map_or_else(
        || err.to_string(),
        |body| describe_server_error(body.code.as_deref(), &body.error),
    )
}

/// Достаёт токен приглашения из ссылки вида `…/register?invite=TOKEN` или
/// возвращает строку как есть, если это уже сам токен.
pub(crate) fn invite_token_from_input(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(url) = url::Url::parse(trimmed) {
        return url
            .query_pairs()
            .find(|(key, _)| key == "invite")
            .map(|(_, value)| value.into_owned())
            .filter(|token| !token.is_empty());
    }
    Some(trimmed.to_owned())
}

/// Локальная проверка имени по правилам сервера (3–32, `[a-z0-9_]`).
pub(crate) fn username_problem(name: &str) -> Option<&'static str> {
    let name = name.trim();
    let len = name.chars().count();
    if len == 0 {
        Some(tr("Укажите имя пользователя"))
    } else if !(3..=32).contains(&len) {
        Some(tr("Имя: от 3 до 32 символов"))
    } else if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some(tr("Только латинские буквы, цифры и подчёркивание"))
    } else {
        None
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
mod register_tests {
    use super::{invite_token_from_input, username_problem};

    #[test]
    fn invite_token_comes_from_link_or_raw_input() {
        assert_eq!(
            invite_token_from_input("https://chat.example/register?invite=abc_DEF-123").as_deref(),
            Some("abc_DEF-123")
        );
        assert_eq!(
            invite_token_from_input("  rawtoken ").as_deref(),
            Some("rawtoken")
        );
        assert_eq!(
            invite_token_from_input("https://chat.example/register"),
            None
        );
        assert_eq!(invite_token_from_input("   "), None);
    }

    #[test]
    fn username_rules_match_server() {
        assert!(username_problem("ab").is_some());
        assert!(username_problem("alice").is_none());
        assert!(username_problem("Alice_42").is_none());
        assert!(username_problem("алиса").is_some());
        assert!(username_problem(&"a".repeat(33)).is_some());
    }
}

#[cfg(test)]
mod tests {
    use bs_instances::ConnectionState;

    use super::{
        author_name, can_manage_message, connection_label, describe_server_error, js_len,
        message_actions, next_screen, outgoing_text, should_ack, should_load_older, should_wake,
        split_message,
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
    fn short_message_is_not_split() {
        assert_eq!(split_message("привет", 10), vec!["привет".to_owned()]);
    }

    #[test]
    fn long_text_splits_on_lines_within_limit() {
        let text = "aaaa\nbbbb\ncccc\ndddd";
        let parts = split_message(text, 10);
        assert_eq!(parts, vec!["aaaa\nbbbb", "cccc\ndddd"]);
        assert!(parts.iter().all(|p| js_len(p) <= 10));
    }

    #[test]
    fn code_block_is_closed_and_reopened_at_the_break() {
        let code = (0..30).fold(String::new(), |acc, i| acc + &format!("let x{i} = {i};\n"));
        let text = format!("Смотри:\n```rust\n{code}```\nвсё");
        let parts = split_message(&text, 120);
        assert!(parts.len() > 2);
        for part in &parts {
            assert!(js_len(part) <= 120, "{part}");
            let fences = part
                .lines()
                .filter(|l| l.trim_start().starts_with("```"))
                .count();
            assert_eq!(fences % 2, 0, "ограды парные: {part}");
        }
        assert!(
            parts[1].starts_with("```rust\n"),
            "язык сохранён: {}",
            parts[1]
        );
        let rebuilt: String = parts
            .iter()
            .flat_map(|p| p.lines())
            .filter(|l| !l.trim_start().starts_with("```"))
            .fold(String::new(), |acc, l| acc + l + "\n");
        assert!(rebuilt.contains("let x29 = 29;"));
        assert_eq!(
            rebuilt.matches("let x").count(),
            30,
            "строки не потерялись и не задвоились"
        );
    }

    #[test]
    fn long_line_is_cut_on_space_and_counts_utf16() {
        let text = "😀".repeat(5) + " " + &"слово ".repeat(10);
        let parts = split_message(&text, 12);
        assert!(parts.iter().all(|p| js_len(p) <= 12));
        assert_eq!(parts.concat().replace(' ', ""), text.replace(' ', ""));
    }

    #[test]
    fn unbroken_line_is_cut_by_characters() {
        let text = "x".repeat(25);
        let parts = split_message(&text, 10);
        assert_eq!(parts, vec!["x".repeat(10), "x".repeat(10), "x".repeat(5)]);
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
