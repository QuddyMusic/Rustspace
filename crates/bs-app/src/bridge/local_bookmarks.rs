//! Закладки на этом устройстве — для инстансов без своих закладок (сервер
//! оригинала их не хранит). Копия сообщения лежит в `local-bookmarks.json`
//! рядом с настройками, по адресу инстанса.

use std::collections::BTreeMap;
use std::path::PathBuf;

use bs_api::{Bookmark, BookmarkAttachment, BookmarkStatus, ChatKind};

type Store = BTreeMap<String, Vec<Bookmark>>;

fn path() -> Option<PathBuf> {
    crate::saved_session::config_dir().map(|d| d.join("local-bookmarks.json"))
}

fn load() -> Store {
    path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(store: &Store) {
    let Some(path) = path() else {
        return;
    };
    let written = serde_json::to_vec_pretty(store)
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
        });
    if let Err(err) = written {
        tracing::warn!(%err, "локальные закладки не сохранились");
    }
}

/// Закладки инстанса, новые сверху.
pub(crate) fn list(base: &str) -> Vec<Bookmark> {
    let mut list = load().remove(base).unwrap_or_default();
    list.sort_by_key(|b| std::cmp::Reverse(b.created_at));
    list
}

/// Какие сообщения чата в закладках.
pub(crate) fn keys(base: &str, kind: ChatKind, channel: &str) -> Vec<String> {
    list(base)
        .into_iter()
        .filter(|b| b.kind == kind && b.channel_id == channel)
        .map(|b| b.message_id)
        .collect()
}

/// Добавить закладку (повтор не дублируется).
pub(crate) fn add(base: &str, bookmark: Bookmark) {
    let mut store = load();
    let list = store.entry(base.to_owned()).or_default();
    list.retain(|b| b.message_id != bookmark.message_id);
    list.push(bookmark);
    save(&store);
}

/// Убрать закладку.
pub(crate) fn remove(base: &str, message: &str) {
    let mut store = load();
    if let Some(list) = store.get_mut(base) {
        list.retain(|b| b.message_id != message);
    }
    save(&store);
}

/// Копия сообщения из кэша сессии.
pub(crate) fn snapshot(
    cache: &bs_core::Cache,
    kind: ChatKind,
    channel: &str,
    message: &str,
) -> Option<Bookmark> {
    let attachment = |a: &bs_types::Attachment| BookmarkAttachment {
        original_name: a.original_name.clone(),
        mimetype: a.mimetype.clone(),
        filename: Some(a.filename.clone()),
        thumbnail_filename: a.thumbnail_filename.clone(),
    };
    let name = |user: &bs_types::User| {
        user.display_name
            .clone()
            .unwrap_or_else(|| user.username.clone())
    };
    let now = crate::registry::now_ms();
    match kind {
        ChatKind::Channel => {
            let found = cache
                .messages(channel)
                .iter()
                .find(|m| m.message.id == message)?;
            let space_id = cache.channel(channel).map(|c| c.space_id.clone());
            Some(Bookmark {
                id: format!("local-{message}"),
                kind,
                message_id: message.to_owned(),
                channel_id: channel.to_owned(),
                space_name: space_id
                    .as_deref()
                    .and_then(|id| cache.space(id))
                    .map(|s| s.space.name.clone()),
                space_id,
                content: found.message.content.clone(),
                author_id: found.user.id.clone(),
                author_name: name(&found.user),
                channel_name: cache.channel(channel).map(|c| c.name.clone()),
                message_created_at: found.message.created_at,
                created_at: now,
                attachments: found.attachments.iter().map(attachment).collect(),
                status: BookmarkStatus::Ok,
            })
        }
        ChatKind::Dm => {
            let found = cache
                .dm_messages(channel)
                .iter()
                .find(|m| m.message.id == message)?;
            Some(Bookmark {
                id: format!("local-{message}"),
                kind,
                message_id: message.to_owned(),
                channel_id: channel.to_owned(),
                space_id: None,
                space_name: None,
                content: found.message.content.clone(),
                author_id: found.user.id.clone(),
                author_name: name(&found.user),
                channel_name: cache
                    .dm_channel(channel)
                    .map(|dm| crate::social::dm_display_name(dm, &cache.me().id)),
                message_created_at: found.message.created_at,
                created_at: now,
                attachments: found.attachments.iter().map(attachment).collect(),
                status: BookmarkStatus::Ok,
            })
        }
    }
}
