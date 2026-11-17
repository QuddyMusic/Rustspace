//! «Избранное», закреплённые сообщения и закладки: состояние сессии.
//!
//! Всё это живёт на инстансе, которому принадлежит чат, поэтому состояние —
//! у каждой сессии своё. GUI спрашивает «нужно ли загрузить» каждый кадр:
//! запрос уходит один раз, пока событие сервера не скажет, что данные
//! устарели.

use std::collections::{HashMap, HashSet};

use bs_api::{Bookmark, ChatKind, Pin};

use super::{BridgeRequest, Session};

/// Ключ чата: вид и идентификатор.
pub(crate) type ChatKey = (ChatKind, String);

/// Запросы к серверу.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExtrasOp {
    /// Найти или создать «Избранное».
    Saved,
    /// Закреплённые в чате.
    Pins(ChatKind, String),
    /// Закрепить (`on`) или открепить: для всех (`shared`) или для себя.
    Pin {
        /// Чат.
        kind: ChatKind,
        /// Канал или переписка.
        channel: String,
        /// Сообщение.
        message: String,
        /// Для всех.
        shared: bool,
        /// Закрепить или открепить.
        on: bool,
    },
    /// Какие сообщения чата в закладках.
    Marks(ChatKind, String),
    /// Страница закладок (`None` — первая).
    Bookmarks(Option<String>),
    /// Добавить (`on`) или убрать закладку.
    Bookmark {
        /// Чат.
        kind: ChatKind,
        /// Канал или переписка (для ключей).
        channel: String,
        /// Сообщение.
        message: String,
        /// Добавить или убрать.
        on: bool,
    },
    /// Загрузить иконку игры для активности.
    GameIcon {
        /// Название игры.
        name: String,
        /// PNG.
        image: Vec<u8>,
    },
}

/// Ответы сервера.
#[derive(Debug, Clone)]
pub(crate) enum ExtrasEvent {
    /// «Избранное»: переписка или ошибка (404 — сервер без него).
    Saved(Result<String, String>),
    /// Закреплённые в чате.
    Pins(ChatKey, Result<Vec<Pin>, String>),
    /// Закладки чата.
    Marks(ChatKey, Vec<String>),
    /// Страница закладок: первая ли и итог.
    Bookmarks(bool, Result<bs_api::BookmarkPage, String>),
    /// Итог действия: что сказать пользователю (ошибка — красным).
    Done(Result<Option<String>, String>),
    /// Иконка игры загружена (ссылка) или нет.
    GameIcon(String, Result<String, String>),
}

/// Состояние одной сессии.
#[derive(Debug, Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "флаги загрузки, не настройки"
)]
pub(crate) struct Extras {
    /// «Избранное» этой сессии.
    pub(crate) saved: Option<String>,
    /// Сервер ответил, что «Избранного» нет (сервер оригинала).
    pub(crate) saved_missing: bool,
    saved_asked: bool,
    /// Открыть «Избранное», как только придёт ответ.
    pub(crate) open_saved: bool,
    /// Закреплённые по чатам.
    pub(crate) pins: HashMap<ChatKey, Vec<Pin>>,
    pins_asked: HashSet<ChatKey>,
    /// Сообщения в закладках по чатам.
    pub(crate) marks: HashMap<ChatKey, HashSet<String>>,
    marks_asked: HashSet<ChatKey>,
    /// Загруженные закладки, новые сверху.
    pub(crate) bookmarks: Vec<Bookmark>,
    /// Курсор следующей страницы.
    pub(crate) bookmarks_next: Option<String>,
    /// Закладки загружены хотя бы раз.
    pub(crate) bookmarks_loaded: bool,
    /// Ошибка загрузки закладок.
    pub(crate) bookmarks_error: Option<String>,
    bookmarks_loading: bool,
    /// Что сказать пользователю.
    pub(crate) notices: Vec<Result<String, String>>,
    /// Итоги загрузки иконок игр (их забирает движок активности).
    pub(crate) game_icons: Vec<(String, Result<String, String>)>,
}

impl Extras {
    /// Забыть всё (после переподключения часть событий могла потеряться).
    pub(crate) fn reset(&mut self) {
        let saved = self.saved.take();
        *self = Self {
            saved,
            ..Self::default()
        };
    }

    /// Ответ сервера.
    pub(crate) fn apply(&mut self, event: ExtrasEvent) {
        match event {
            ExtrasEvent::Saved(Ok(channel)) => self.saved = Some(channel),
            ExtrasEvent::Saved(Err(_)) => {
                self.saved_missing = true;
                self.open_saved = false;
            }
            ExtrasEvent::Pins(key, Ok(list)) => {
                self.pins.insert(key, list);
            }
            ExtrasEvent::Pins(key, Err(_)) => {
                self.pins.insert(key, Vec::new());
            }
            ExtrasEvent::Marks(key, ids) => {
                self.marks.insert(key, ids.into_iter().collect());
            }
            ExtrasEvent::Bookmarks(first, result) => {
                self.bookmarks_loading = false;
                match result {
                    Ok(page) => {
                        if first {
                            self.bookmarks.clear();
                        }
                        self.bookmarks.extend(page.items);
                        self.bookmarks_next = page.next_cursor;
                        self.bookmarks_loaded = true;
                        self.bookmarks_error = None;
                    }
                    Err(err) => self.bookmarks_error = Some(err),
                }
            }
            ExtrasEvent::Done(Ok(Some(text))) => self.notices.push(Ok(text)),
            ExtrasEvent::Done(Ok(None)) => {}
            ExtrasEvent::Done(Err(err)) => self.notices.push(Err(err)),
            ExtrasEvent::GameIcon(name, result) => self.game_icons.push((name, result)),
        }
    }

    /// Событие сервера о закрепах и закладках.
    pub(crate) fn server_event(&mut self, event: &bs_types::ServerEvent) {
        use bs_types::ServerEvent;
        let kind = |code: &str| match code {
            "dm" => ChatKind::Dm,
            _ => ChatKind::Channel,
        };
        match event {
            ServerEvent::RsPinsUpdated {
                kind: code,
                channel_id,
            } => {
                self.pins_asked.remove(&(kind(code), channel_id.clone()));
            }
            ServerEvent::BookmarkAdded { bookmark } => {
                if let Ok(bookmark) = serde_json::from_value::<Bookmark>(bookmark.clone()) {
                    self.marks
                        .entry((bookmark.kind, bookmark.channel_id.clone()))
                        .or_default()
                        .insert(bookmark.message_id.clone());
                    if self.bookmarks_loaded && !self.bookmarks.iter().any(|b| b.id == bookmark.id)
                    {
                        self.bookmarks.insert(0, bookmark);
                    }
                }
            }
            ServerEvent::BookmarkRemoved {
                kind: code,
                channel_id,
                message_id,
            } => {
                if let Some(set) = self.marks.get_mut(&(kind(code), channel_id.clone())) {
                    set.remove(message_id);
                }
                self.bookmarks.retain(|b| &b.message_id != message_id);
            }
            _ => {}
        }
    }
}

impl Session {
    /// Состояние «Избранного», закрепов и закладок.
    pub(crate) const fn extras(&self) -> &Extras {
        &self.extras
    }

    /// Изменяемое состояние (уведомления забирает GUI).
    pub(crate) const fn extras_mut(&mut self) -> &mut Extras {
        &mut self.extras
    }

    /// Спросить «Избранное», если ещё не спрашивали.
    pub(crate) fn ensure_saved(&mut self) {
        if !self.extras.saved_asked && self.is_live() {
            self.extras.saved_asked = self.request(BridgeRequest::Extras(ExtrasOp::Saved));
        }
    }

    /// Загрузить закрепы чата, если их нет или они устарели.
    pub(crate) fn ensure_pins(&mut self, kind: ChatKind, channel: &str) {
        let key = (kind, channel.to_owned());
        if !self.extras.pins_asked.contains(&key)
            && self.request(BridgeRequest::Extras(ExtrasOp::Pins(
                kind,
                channel.to_owned(),
            )))
        {
            self.extras.pins_asked.insert(key);
        }
    }

    /// Загрузить закладки чата, если их нет.
    pub(crate) fn ensure_marks(&mut self, kind: ChatKind, channel: &str) {
        let key = (kind, channel.to_owned());
        if !self.extras.marks_asked.contains(&key)
            && self.request(BridgeRequest::Extras(ExtrasOp::Marks(
                kind,
                channel.to_owned(),
            )))
        {
            self.extras.marks_asked.insert(key);
        }
    }

    /// Загрузить закладки: первую страницу (`more = false`) или следующую.
    pub(crate) fn load_bookmarks(&mut self, more: bool) {
        if self.extras.bookmarks_loading {
            return;
        }
        let cursor = if more {
            let Some(cursor) = self.extras.bookmarks_next.clone() else {
                return;
            };
            Some(cursor)
        } else {
            None
        };
        self.extras.bookmarks_loading =
            self.request(BridgeRequest::Extras(ExtrasOp::Bookmarks(cursor)));
    }

    /// Закрепить или открепить. Закрепы чата перечитываются после ответа.
    pub(crate) fn set_pin(&mut self, op: ExtrasOp) {
        if let ExtrasOp::Pin { kind, channel, .. } = &op {
            self.extras.pins_asked.remove(&(*kind, channel.clone()));
        }
        self.request(BridgeRequest::Extras(op));
    }

    /// Добавить или убрать закладку (ключи обновляются сразу).
    pub(crate) fn set_bookmark(&mut self, kind: ChatKind, channel: &str, message: &str, on: bool) {
        let set = self
            .extras
            .marks
            .entry((kind, channel.to_owned()))
            .or_default();
        if on {
            set.insert(message.to_owned());
        } else {
            set.remove(message);
            self.extras.bookmarks.retain(|b| b.message_id != message);
        }
        self.request(BridgeRequest::Extras(ExtrasOp::Bookmark {
            kind,
            channel: channel.to_owned(),
            message: message.to_owned(),
            on,
        }));
    }
}

/// Выполняет запрос (в фоне).
pub(super) async fn run(client: &bs_api::ApiClient, op: ExtrasOp) -> ExtrasEvent {
    let text = |err: bs_api::ApiError| super::worker::space_message(&err);
    match op {
        ExtrasOp::Saved => ExtrasEvent::Saved(
            client
                .saved_messages()
                .await
                .map(|channel| channel.id)
                .map_err(text),
        ),
        ExtrasOp::Pins(kind, channel) => {
            let result = client.pins(kind, &channel).await.map_err(text);
            ExtrasEvent::Pins((kind, channel), result)
        }
        ExtrasOp::Pin {
            kind,
            channel,
            message,
            shared,
            on,
        } => {
            let result = if on {
                client.pin(kind, &channel, &message, shared).await
            } else {
                client.unpin(kind, &channel, &message, shared).await
            };
            ExtrasEvent::Done(result.map(|()| None).map_err(text))
        }
        ExtrasOp::Marks(kind, channel) => {
            let ids = client
                .bookmark_keys(kind, &channel)
                .await
                .unwrap_or_default();
            ExtrasEvent::Marks((kind, channel), ids)
        }
        ExtrasOp::Bookmarks(cursor) => {
            let first = cursor.is_none();
            ExtrasEvent::Bookmarks(
                first,
                client.bookmarks(cursor.as_deref()).await.map_err(text),
            )
        }
        ExtrasOp::Bookmark {
            kind, message, on, ..
        } => {
            let result = if on {
                client
                    .add_bookmark(kind, &message)
                    .await
                    .map(|_| Some(crate::i18n::tr("Добавлено в закладки").to_owned()))
            } else {
                client.remove_bookmark(kind, &message).await.map(|()| None)
            };
            ExtrasEvent::Done(result.map_err(text))
        }
        ExtrasOp::GameIcon { name, image } => {
            ExtrasEvent::GameIcon(name, client.upload_game_icon(&image).await.map_err(text))
        }
    }
}
