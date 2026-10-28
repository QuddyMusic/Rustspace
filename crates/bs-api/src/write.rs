//! Запись: друзья, поиск и личные сообщения.
//!
//! Методы требуют авторизации: без Bearer-токена сервер ответит
//! `401 unauthorized`.

use bs_types::{
    DiscoverResponse, DmChannel, DmSearchResponse, Friend, FriendRequest, LiveKitTokenRequest,
    LiveKitTokenResponse, MessageSearchResponse, User,
};
use serde::Serialize;
use url::form_urlencoded;

use crate::client::ApiClient;
use crate::error::ApiError;

/// Минимальный `limit` поиска сообщений.
pub const SEARCH_LIMIT_MIN: u16 = 1;
/// Максимальный `limit` поиска сообщений (ограничение сервера).
pub const SEARCH_LIMIT_MAX: u16 = 50;
/// `limit` поиска сообщений по умолчанию (как у сервера).
pub const SEARCH_LIMIT_DEFAULT: u16 = 25;

/// Тело `POST /api/social/requests`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendFriendRequest {
    /// Локальное имя (`alice` или `alice@host`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Федеративная идентичность: домашний id пользователя.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_user_id: Option<String>,
    /// Федеративная идентичность: домашний инстанс.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_instance: Option<String>,
}

impl SendFriendRequest {
    /// Заявка по локальному имени.
    #[must_use]
    pub fn username(username: impl Into<String>) -> Self {
        Self {
            username: Some(username.into()),
            ..Self::default()
        }
    }

    /// Заявка по федеративной идентичности.
    #[must_use]
    pub fn identity(home_user_id: impl Into<String>, home_instance: impl Into<String>) -> Self {
        Self {
            username: None,
            home_user_id: Some(home_user_id.into()),
            home_instance: Some(home_instance.into()),
        }
    }
}

/// Решение по входящей заявке в друзья.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FriendRequestDecision {
    /// Принять заявку.
    Accept,
    /// Отклонить заявку.
    Decline,
}

impl FriendRequestDecision {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accepted",
            Self::Decline => "declined",
        }
    }
}

#[derive(Debug, Serialize)]
struct DecisionBody {
    status: &'static str,
}

/// Тело `POST /api/dm`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDmRequest {
    /// Локальный id собеседника.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// Федеративная идентичность: домашний id собеседника.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_user_id: Option<String>,
    /// Федеративная идентичность: домашний инстанс собеседника.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_instance: Option<String>,
}

impl CreateDmRequest {
    /// Личный канал с локальным пользователем.
    #[must_use]
    pub fn user(user_id: impl Into<String>) -> Self {
        Self {
            user_id: Some(user_id.into()),
            ..Self::default()
        }
    }

    /// Личный канал с федеративным пользователем.
    #[must_use]
    pub fn identity(home_user_id: impl Into<String>, home_instance: impl Into<String>) -> Self {
        Self {
            user_id: None,
            home_user_id: Some(home_user_id.into()),
            home_instance: Some(home_instance.into()),
        }
    }
}

/// Параметры поиска сообщений в канале или в DM.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageSearchQuery {
    /// Подстрока текста сообщения.
    pub text: Option<String>,
    /// Фильтр по автору (подстрока username).
    pub from: Option<String>,
    /// Фильтр по типу вложения (`link`, `image`, `file`, ...).
    pub has: Option<String>,
    /// Только сообщения старше указанного id.
    pub before: Option<String>,
    /// Только сообщения новее указанного id.
    pub after: Option<String>,
    /// Смещение страницы.
    pub offset: Option<u32>,
    /// Размер страницы (зажимается в `1..=50`).
    pub limit: Option<u16>,
}

impl MessageSearchQuery {
    /// Поиск по тексту.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Self::default()
        }
    }

    /// Собирает query-string без ведущего `?`.
    fn to_query_string(&self) -> String {
        let mut ser = form_urlencoded::Serializer::new(String::new());
        if let Some(text) = &self.text {
            ser.append_pair("q", text);
        }
        if let Some(from) = &self.from {
            ser.append_pair("from", from);
        }
        if let Some(has) = &self.has {
            ser.append_pair("has", has);
        }
        if let Some(before) = &self.before {
            ser.append_pair("before", before);
        }
        if let Some(after) = &self.after {
            ser.append_pair("after", after);
        }
        if let Some(offset) = self.offset {
            ser.append_pair("offset", &offset.to_string());
        }
        if let Some(limit) = self.limit {
            let clamped = limit.clamp(SEARCH_LIMIT_MIN, SEARCH_LIMIT_MAX);
            ser.append_pair("limit", &clamped.to_string());
        }
        ser.finish()
    }
}

/// Добавляет `?query` к пути, если query непустой.
fn with_query(path: String, query: &str) -> String {
    if query.is_empty() {
        return path;
    }
    let mut path = path;
    path.push('?');
    path.push_str(query);
    path
}

impl ApiClient {
    /// `GET /api/social/friends` — список друзей.
    pub async fn friends(&self) -> Result<Vec<Friend>, ApiError> {
        self.get_json("/api/social/friends").await
    }

    /// `GET /api/social/requests` — ожидающие заявки (входящие и исходящие).
    pub async fn friend_requests(&self) -> Result<Vec<FriendRequest>, ApiError> {
        self.get_json("/api/social/requests").await
    }

    /// `POST /api/social/requests` — отправить заявку в друзья.
    pub async fn send_friend_request(
        &self,
        request: &SendFriendRequest,
    ) -> Result<FriendRequest, ApiError> {
        self.post_json("/api/social/requests", request).await
    }

    /// `PATCH /api/social/requests/:id` — принять или отклонить входящую заявку.
    pub async fn respond_friend_request(
        &self,
        request_id: &str,
        decision: FriendRequestDecision,
    ) -> Result<(), ApiError> {
        let body = DecisionBody {
            status: decision.as_str(),
        };
        let path = format!("/api/social/requests/{request_id}");
        self.patch_json::<serde_json::Value, _>(&path, &body)
            .await?;
        Ok(())
    }

    /// `DELETE /api/social/requests/:id` — отменить исходящую заявку.
    pub async fn cancel_friend_request(&self, request_id: &str) -> Result<(), ApiError> {
        let path = format!("/api/social/requests/{request_id}");
        self.delete_json::<serde_json::Value>(&path).await?;
        Ok(())
    }

    /// `DELETE /api/social/friends/:id` — удалить друга.
    pub async fn remove_friend(&self, user_id: &str) -> Result<(), ApiError> {
        let path = format!("/api/social/friends/{user_id}");
        self.delete_json::<serde_json::Value>(&path).await?;
        Ok(())
    }

    /// `GET /api/social/search?q=` — поиск пользователей (до 10 результатов).
    pub async fn search_users(&self, query: &str) -> Result<Vec<User>, ApiError> {
        let query_string = {
            let mut ser = form_urlencoded::Serializer::new(String::new());
            ser.append_pair("q", query);
            ser.finish()
        };
        self.get_json(&with_query("/api/social/search".to_owned(), &query_string))
            .await
    }

    /// `GET /api/social/discover` — каталог пользователей инстанса.
    pub async fn discover_users(
        &self,
        query: &str,
        limit: Option<u16>,
        offset: Option<u32>,
    ) -> Result<DiscoverResponse, ApiError> {
        let query_string = {
            let mut ser = form_urlencoded::Serializer::new(String::new());
            ser.append_pair("q", query);
            if let Some(limit) = limit {
                ser.append_pair("limit", &limit.to_string());
            }
            if let Some(offset) = offset {
                ser.append_pair("offset", &offset.to_string());
            }
            ser.finish()
        };
        self.get_json(&with_query(
            "/api/social/discover".to_owned(),
            &query_string,
        ))
        .await
    }

    /// `GET /api/channels/:id/search` — поиск сообщений в канале.
    pub async fn search_channel_messages(
        &self,
        channel_id: &str,
        query: &MessageSearchQuery,
    ) -> Result<MessageSearchResponse, ApiError> {
        let path = with_query(
            format!("/api/channels/{channel_id}/search"),
            &query.to_query_string(),
        );
        self.get_json(&path).await
    }

    /// `GET /api/dm/:id/search` — поиск сообщений в личном канале.
    pub async fn search_dm_messages(
        &self,
        dm_channel_id: &str,
        query: &MessageSearchQuery,
    ) -> Result<DmSearchResponse, ApiError> {
        let path = with_query(
            format!("/api/dm/{dm_channel_id}/search"),
            &query.to_query_string(),
        );
        self.get_json(&path).await
    }

    /// `GET /api/dm` — открытые личные каналы (свежие сверху).
    pub async fn dm_channels(&self) -> Result<Vec<DmChannel>, ApiError> {
        self.get_json("/api/dm").await
    }

    /// `POST /api/dm` — открыть (или получить существующий) личный канал.
    pub async fn open_dm(&self, request: &CreateDmRequest) -> Result<DmChannel, ApiError> {
        self.post_json("/api/dm", request).await
    }

    /// `POST /api/livekit/token` — JWT и `wss://`-адрес LiveKit для входа
    /// в голосовой канал пространства. Комната на сервере — сам `channelId`.
    pub async fn livekit_token_channel(
        &self,
        channel_id: &str,
    ) -> Result<LiveKitTokenResponse, ApiError> {
        self.post_json(
            "/api/livekit/token",
            &LiveKitTokenRequest {
                channel_id: Some(channel_id.to_owned()),
                dm_channel_id: None,
            },
        )
        .await
    }

    /// `POST /api/livekit/token` — JWT и `wss://`-адрес LiveKit для DM-звонка.
    /// Сервер назовёт комнату по `federatedId` канала, если он есть.
    pub async fn livekit_token_dm(
        &self,
        dm_channel_id: &str,
    ) -> Result<LiveKitTokenResponse, ApiError> {
        self.post_json(
            "/api/livekit/token",
            &LiveKitTokenRequest {
                channel_id: None,
                dm_channel_id: Some(dm_channel_id.to_owned()),
            },
        )
        .await
    }
}
