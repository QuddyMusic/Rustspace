//! Боты (`/api/rustspace/bots…`; у оригинала их нет) и запросы, которые
//! нужны самим ботам: правка и удаление сообщений, сообщения в личке.

use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::compose::SendMessageBody;
use crate::error::ApiError;
use bs_types::{DmMessageWithUser, MessageWithUser};

/// Пространство, в котором состоит бот.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BotSpace {
    /// Идентификатор.
    pub id: String,
    /// Название.
    pub name: String,
}

/// Бот, которым владеет пользователь.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bot {
    /// Идентификатор (он же идентификатор пользователя бота).
    pub id: String,
    /// Имя для входа и упоминаний.
    pub username: String,
    /// Отображаемое имя.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Аватар.
    #[serde(default)]
    pub avatar: Option<String>,
    /// Что бот делает.
    #[serde(default)]
    pub description: Option<String>,
    /// Когда создан, мс.
    pub created_at: i64,
    /// Пространства бота.
    #[serde(default)]
    pub spaces: Vec<BotSpace>,
}

/// Бот вместе с токеном: токен показывают один раз.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BotWithToken {
    /// Бот.
    pub bot: Bot,
    /// Токен `rsb_…`.
    pub token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NewBot<'a> {
    username: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
}

/// Изменение бота: `None` — не трогать, `Some("")` — очистить.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotPatch {
    /// Отображаемое имя.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Описание.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Serialize)]
struct Edit<'a> {
    content: &'a str,
}

impl ApiClient {
    /// `GET /api/rustspace/bots` — свои боты.
    ///
    /// # Errors
    /// Ошибки запроса; сервер без ботов (404).
    pub async fn bots(&self) -> Result<Vec<Bot>, ApiError> {
        self.get_json("/api/rustspace/bots").await
    }

    /// `POST /api/rustspace/bots` — новый бот и его токен.
    ///
    /// # Errors
    /// Имя занято или неверно, ботов слишком много.
    pub async fn create_bot(
        &self,
        username: &str,
        display_name: Option<&str>,
        description: Option<&str>,
    ) -> Result<BotWithToken, ApiError> {
        self.post_json(
            "/api/rustspace/bots",
            &NewBot {
                username,
                display_name,
                description,
            },
        )
        .await
    }

    /// `PATCH /api/rustspace/bots/:id`.
    ///
    /// # Errors
    /// Бот не свой или поле неверно.
    pub async fn update_bot(&self, id: &str, patch: &BotPatch) -> Result<Bot, ApiError> {
        self.patch_json(&format!("/api/rustspace/bots/{id}"), patch)
            .await
    }

    /// `POST /api/rustspace/bots/:id/token` — новый токен, старый перестаёт
    /// работать.
    ///
    /// # Errors
    /// Бот не свой.
    pub async fn regenerate_bot_token(&self, id: &str) -> Result<String, ApiError> {
        #[derive(Deserialize)]
        struct Token {
            token: String,
        }
        let token: Token = self
            .post_json(&format!("/api/rustspace/bots/{id}/token"), &())
            .await?;
        Ok(token.token)
    }

    /// `DELETE /api/rustspace/bots/:id` — бот выходит из всех пространств и
    /// удаляется.
    ///
    /// # Errors
    /// Бот не свой.
    pub async fn delete_bot(&self, id: &str) -> Result<(), ApiError> {
        self.delete_json::<serde_json::Value>(&format!("/api/rustspace/bots/{id}"))
            .await?;
        Ok(())
    }

    /// `PUT /api/rustspace/spaces/:space/bots/:bot` — добавить своего бота в
    /// пространство (нужно право «Управлять пространством»).
    ///
    /// # Errors
    /// Нет права, бот не свой или уже там.
    pub async fn add_bot_to_space(&self, space_id: &str, bot_id: &str) -> Result<(), ApiError> {
        self.put_json::<serde_json::Value, _>(
            &format!("/api/rustspace/spaces/{space_id}/bots/{bot_id}"),
            &(),
        )
        .await?;
        Ok(())
    }

    /// `PATCH /api/messages/:id` — изменить своё сообщение в канале.
    ///
    /// # Errors
    /// Сообщение не своё или его нет.
    pub async fn edit_channel_message(
        &self,
        message_id: &str,
        content: &str,
    ) -> Result<MessageWithUser, ApiError> {
        self.patch_json(&format!("/api/messages/{message_id}"), &Edit { content })
            .await
    }

    /// `DELETE /api/messages/:id`.
    ///
    /// # Errors
    /// Нет права или сообщения нет.
    pub async fn delete_channel_message(&self, message_id: &str) -> Result<(), ApiError> {
        self.delete_json::<serde_json::Value>(&format!("/api/messages/{message_id}"))
            .await?;
        Ok(())
    }

    /// `POST /api/dm/:id/messages` — сообщение в личку или группу.
    ///
    /// # Errors
    /// Нет доступа к переписке.
    pub async fn send_dm_message(
        &self,
        dm_channel_id: &str,
        body: &SendMessageBody,
    ) -> Result<DmMessageWithUser, ApiError> {
        self.post_json(&format!("/api/dm/{dm_channel_id}/messages"), body)
            .await
    }
}
