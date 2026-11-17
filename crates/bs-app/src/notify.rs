//! Системные (десктопные) уведомления о новых сообщениях.
//!
//! Решение «оповещать ли» принимает то же правило, что и звук
//! (`isMessageAlert` оригинала, см. [`crate::voice::message_alerts_user`]);
//! здесь только заголовок, текст и показ. Показ идёт в отдельном потоке:
//! на Linux это D-Bus-вызов, который не должен тормозить кадр. Если
//! демона уведомлений нет, ошибка пишется в лог и больше ничего не происходит.

use bs_core::Cache;

use crate::bridge::NewMessage;
use crate::i18n::tr;

/// Длина текста в уведомлении.
const PREVIEW_CHARS: usize = 140;

/// Заголовок и текст уведомления.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Toast {
    /// «Алиса» или «Алиса · #общий».
    pub(crate) title: String,
    /// Начало сообщения или нейтральная подпись.
    pub(crate) body: String,
}

/// Собирает уведомление; `previews` — показывать ли текст сообщения.
pub(crate) fn toast(cache: &Cache, message: &NewMessage, previews: bool) -> Toast {
    let author = author_name(cache, &message.author_id);
    let title = if message.dm {
        author
    } else {
        cache.channel(&message.channel_id).map_or_else(
            || author.clone(),
            |channel| format!("{author} · #{}", channel.name),
        )
    };
    let body = if previews {
        preview(&message.content)
    } else {
        tr("Новое сообщение").to_owned()
    };
    Toast { title, body }
}

/// Показывает уведомление, если оно включено и окно не в фокусе (или
/// пользователь просил показывать всегда).
pub(crate) fn message(cache: &Cache, message: &NewMessage, focused: bool) {
    let prefs = crate::prefs::get().desktop;
    if !prefs.notifications || (focused && prefs.only_unfocused) {
        return;
    }
    show(toast(cache, message, prefs.previews));
}

/// Показ в фоновом потоке.
pub(crate) fn show(toast: Toast) {
    let spawned = std::thread::Builder::new()
        .name("notify".to_owned())
        .spawn(move || {
            let result = notify_rust::Notification::new()
                .appname(crate::brand::NAME)
                .summary(&toast.title)
                .body(&toast.body)
                .show();
            if let Err(err) = result {
                tracing::debug!(%err, "системное уведомление не показано");
            }
        });
    if let Err(err) = spawned {
        tracing::debug!(%err, "поток уведомления не запущен");
    }
}

fn author_name(cache: &Cache, user_id: &str) -> String {
    let user = cache
        .spaces()
        .flat_map(|space| space.members.iter().map(|m| &m.user))
        .chain(cache.dm_channels().flat_map(|dm| dm.members.iter()))
        .find(|user| user.id == user_id);
    user.map_or_else(
        || crate::brand::NAME.to_owned(),
        |user| {
            user.display_name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(&user.username)
                .to_owned()
        },
    )
}

/// Одна строка без разметки упоминаний, не длиннее [`PREVIEW_CHARS`].
fn preview(content: &str) -> String {
    let flat: String = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return tr("Вложение").to_owned();
    }
    if crate::media::sticker_of(&flat).is_some() {
        return tr("Стикер").to_owned();
    }
    crate::theme::truncate_chars(&flat, PREVIEW_CHARS)
}

#[cfg(test)]
mod tests {
    use super::preview;

    #[test]
    fn preview_is_one_short_line() {
        assert_eq!(preview("привет\n  мир"), "привет мир");
        assert_eq!(preview("   "), "Вложение");
        let long = "а".repeat(500);
        assert_eq!(preview(&long).chars().count(), 140);
    }
}
