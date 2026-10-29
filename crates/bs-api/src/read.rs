//! Чтение: пространства, каналы и история сообщений.

use bs_types::{
    Channel, DmMessageWithUser, MessageId, MessageWithUser, Space, SpaceId,
    SpaceWithChannelsAndMembers,
};

use crate::client::ApiClient;
use crate::error::ApiError;

/// Направление постраничного чтения истории.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PagingCursor {
    /// Сообщения **старше** указанного id.
    Before(MessageId),
    /// Сообщения **новее** указанного id.
    After(MessageId),
}

/// Значение `limit` по умолчанию в `GET /api/channels/:id/messages`.
pub const MESSAGES_LIMIT_DEFAULT: u16 = 50;
/// Минимальный допустимый `limit`.
pub const MESSAGES_LIMIT_MIN: u16 = 1;
/// Максимальный допустимый `limit`.
pub const MESSAGES_LIMIT_MAX: u16 = 100;

/// Результат страницы истории.
#[derive(Debug, Clone, PartialEq)]
pub struct MessagePage {
    /// Сообщения в порядке «старые → новые».
    pub messages: Vec<MessageWithUser>,
    /// `true`, если сервер подтвердил сборку страницы по курсору `after`
    /// (заголовок `X-Backspace-Paging: after`).
    pub paged_after: bool,
}

/// Страница истории личного канала: те же правила, что у [`MessagePage`].
#[derive(Debug, Clone, PartialEq)]
pub struct DmMessagePage {
    /// Сообщения в порядке «старые → новые».
    pub messages: Vec<DmMessageWithUser>,
    /// `true`, если сервер подтвердил сборку страницы по курсору `after`.
    pub paged_after: bool,
}

/// Путь `GET .../messages` с `limit` и одним курсором.
///
/// Общий для каналов пространств и личных каналов: контракт пагинации у них
/// один (`docs/systems/api.md`, «Message history paging»).
fn history_path(base: &str, cursor: Option<PagingCursor>, limit: Option<u16>) -> String {
    let mut query: Vec<String> = Vec::with_capacity(2);
    if let Some(limit) = limit {
        let clamped = limit.clamp(MESSAGES_LIMIT_MIN, MESSAGES_LIMIT_MAX);
        query.push(format!("limit={clamped}"));
    }
    match cursor {
        Some(PagingCursor::Before(id)) => query.push(format!("before={id}")),
        Some(PagingCursor::After(id)) => query.push(format!("after={id}")),
        None => {}
    }
    let mut path = base.to_owned();
    if !query.is_empty() {
        path.push('?');
        path.push_str(&query.join("&"));
    }
    path
}

/// `true`, если сервер ответил заголовком `X-Backspace-Paging: after`.
fn is_paged_after(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .get("x-backspace-paging")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("after"))
}

impl ApiClient {
    /// `GET /api/spaces` — пространства текущего пользователя.
    pub async fn spaces(&self) -> Result<Vec<Space>, ApiError> {
        self.get_json("/api/spaces").await
    }

    /// `GET /api/spaces/:id` — детали пространства с каналами и участниками.
    pub async fn space(&self, id: &str) -> Result<SpaceWithChannelsAndMembers, ApiError> {
        self.get_json(&format!("/api/spaces/{id}")).await
    }

    /// `GET /api/spaces/:id/channels` — каналы пространства (отсортированы сервером).
    pub async fn channels(&self, space_id: &SpaceId) -> Result<Vec<Channel>, ApiError> {
        self.get_json(&format!("/api/spaces/{space_id}/channels"))
            .await
    }

    /// `GET /api/channels/:id/messages` — страница истории.
    ///
    /// `limit` зажимается в `1..=100`. Курсор один: при передаче обоих сервер
    /// вернул бы `400 paging_cursor_conflict`.
    pub async fn messages(
        &self,
        channel_id: &str,
        cursor: Option<PagingCursor>,
        limit: Option<u16>,
    ) -> Result<MessagePage, ApiError> {
        let path = history_path(
            &format!("/api/channels/{channel_id}/messages"),
            cursor,
            limit,
        );

        let (messages, headers) = self
            .get_json_with_headers::<Vec<MessageWithUser>>(&path)
            .await?;
        let paged_after = is_paged_after(&headers);

        Ok(MessagePage {
            messages,
            paged_after,
        })
    }

    /// `GET /api/dm/:id/messages` — страница истории личного канала.
    ///
    /// Контракт тот же, что у [`ApiClient::messages`]: `limit` зажимается
    /// в `1..=100`, курсор один, страница идёт «старые → новые».
    pub async fn dm_messages(
        &self,
        dm_channel_id: &str,
        cursor: Option<PagingCursor>,
        limit: Option<u16>,
    ) -> Result<DmMessagePage, ApiError> {
        let path = history_path(&format!("/api/dm/{dm_channel_id}/messages"), cursor, limit);

        let (messages, headers) = self
            .get_json_with_headers::<Vec<DmMessageWithUser>>(&path)
            .await?;
        let paged_after = is_paged_after(&headers);

        Ok(DmMessagePage {
            messages,
            paged_after,
        })
    }
}
