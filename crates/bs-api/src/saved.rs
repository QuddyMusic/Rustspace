//! «Избранное», закреплённые сообщения и закладки.
//!
//! «Избранное» и закрепы — расширения Rustspace (`/api/rustspace/...`);
//! закладки — как в PR оригинала #474 (`/api/bookmarks`).

use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::error::ApiError;

/// Чат закрепа или закладки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatKind {
    /// Канал пространства.
    Channel,
    /// Личная переписка или группа.
    Dm,
}

impl ChatKind {
    /// Как в адресе.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Dm => "dm",
        }
    }
}

/// Закреплённое сообщение.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    /// Сообщение.
    pub message_id: String,
    /// Видят все участники (`false` — только закрепивший).
    pub shared: bool,
    /// Кто закрепил.
    pub pinned_by: String,
    /// Когда, мс.
    pub created_at: i64,
    /// Само сообщение, как в ленте.
    pub message: serde_json::Value,
}

/// Состояние оригинала закладки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BookmarkStatus {
    /// Есть и не менялся.
    Ok,
    /// Изменён после сохранения.
    Edited,
    /// Удалён.
    Deleted,
    /// Есть, но больше не виден.
    Unavailable,
}

/// Вложение закладки.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkAttachment {
    /// Имя файла у отправителя.
    pub original_name: String,
    /// Тип.
    pub mimetype: String,
    /// Файл на сервере (`None` — удалён вместе с сообщением).
    #[serde(default)]
    pub filename: Option<String>,
    /// Превью.
    #[serde(default)]
    pub thumbnail_filename: Option<String>,
}

/// Закладка: копия сообщения со ссылкой на оригинал.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    /// Идентификатор.
    pub id: String,
    /// Чат оригинала.
    pub kind: ChatKind,
    /// Оригинал.
    pub message_id: String,
    /// Канал или переписка оригинала.
    pub channel_id: String,
    /// Пространство (у ЛС нет).
    #[serde(default)]
    pub space_id: Option<String>,
    /// Текст, как был при сохранении.
    #[serde(default)]
    pub content: Option<String>,
    /// Автор.
    pub author_id: String,
    /// Имя автора при сохранении.
    pub author_name: String,
    /// Пространство.
    #[serde(default)]
    pub space_name: Option<String>,
    /// Канал или собеседники.
    #[serde(default)]
    pub channel_name: Option<String>,
    /// Когда написано, мс.
    pub message_created_at: i64,
    /// Когда сохранено, мс.
    pub created_at: i64,
    /// Вложения.
    #[serde(default)]
    pub attachments: Vec<BookmarkAttachment>,
    /// Что с оригиналом.
    pub status: BookmarkStatus,
}

/// Страница закладок.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkPage {
    /// Закладки, новые сверху.
    pub items: Vec<Bookmark>,
    /// Курсор следующей страницы.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[derive(Serialize)]
struct Shared {
    shared: bool,
}

impl ApiClient {
    /// «Избранное» — переписка с самим собой (создаётся при первом обращении).
    ///
    /// # Errors
    /// Сервер без этой функции (404).
    pub async fn saved_messages(&self) -> Result<bs_types::DmChannel, ApiError> {
        self.get_json("/api/rustspace/saved").await
    }

    /// Закреплённые сообщения чата: общие и свои, новые сверху.
    ///
    /// # Errors
    /// Нет доступа к чату или сервер без закрепов.
    pub async fn pins(&self, kind: ChatKind, channel: &str) -> Result<Vec<Pin>, ApiError> {
        self.get_json(&format!("/api/rustspace/pins/{}/{channel}", kind.code()))
            .await
    }

    /// Закрепить: для всех (`shared`) или для себя.
    ///
    /// # Errors
    /// Нет права, предел закрепов.
    pub async fn pin(
        &self,
        kind: ChatKind,
        channel: &str,
        message: &str,
        shared: bool,
    ) -> Result<(), ApiError> {
        self.put_json::<serde_json::Value, _>(
            &format!("/api/rustspace/pins/{}/{channel}/{message}", kind.code()),
            &Shared { shared },
        )
        .await?;
        Ok(())
    }

    /// Открепить.
    ///
    /// # Errors
    /// Нет права.
    pub async fn unpin(
        &self,
        kind: ChatKind,
        channel: &str,
        message: &str,
        shared: bool,
    ) -> Result<(), ApiError> {
        self.delete_json::<serde_json::Value>(&format!(
            "/api/rustspace/pins/{}/{channel}/{message}?shared={shared}",
            kind.code()
        ))
        .await?;
        Ok(())
    }

    /// Страница закладок.
    ///
    /// # Errors
    /// Сервер без закладок (404).
    pub async fn bookmarks(&self, cursor: Option<&str>) -> Result<BookmarkPage, ApiError> {
        let path = cursor.map_or_else(
            || "/api/bookmarks?limit=100".to_owned(),
            |cursor| format!("/api/bookmarks?limit=100&cursor={cursor}"),
        );
        self.get_json(&path).await
    }

    /// Какие сообщения чата в закладках.
    ///
    /// # Errors
    /// Сервер без закладок (404).
    pub async fn bookmark_keys(
        &self,
        kind: ChatKind,
        channel: &str,
    ) -> Result<Vec<String>, ApiError> {
        self.get_json(&format!(
            "/api/bookmarks/keys?kind={}&channelId={channel}",
            kind.code()
        ))
        .await
    }

    /// Добавить в закладки.
    ///
    /// # Errors
    /// Сообщение не найдено или системное.
    pub async fn add_bookmark(&self, kind: ChatKind, message: &str) -> Result<Bookmark, ApiError> {
        self.put_json(&format!("/api/bookmarks/{}/{message}", kind.code()), &())
            .await
    }

    /// Убрать из закладок.
    ///
    /// # Errors
    /// Ошибка сети.
    pub async fn remove_bookmark(&self, kind: ChatKind, message: &str) -> Result<(), ApiError> {
        self.delete_json::<serde_json::Value>(&format!("/api/bookmarks/{}/{message}", kind.code()))
            .await?;
        Ok(())
    }
}
