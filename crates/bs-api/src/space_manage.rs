//! Управление пространством: приглашения, вход по коду, выход, каналы и
//! категории.
//!
//! Маршруты и поля — как в `packages/server/src/routes/{spaces,channels}.ts`.

use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::error::ApiError;

/// Тело `POST /api/spaces/:id/channels`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewChannel {
    /// Имя канала.
    pub name: String,
    /// `text` или `voice`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Тема (только у текстовых).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Категория; без неё канал лежит на верхнем уровне.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InviteCode {
    invite_code: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JoinBody<'a> {
    invite_code: &'a str,
}

#[derive(Debug, Deserialize)]
struct IdOnly {
    id: String,
}

#[derive(Debug, Serialize)]
struct NameBody<'a> {
    name: &'a str,
}

/// Достаёт код из того, что человек вставил: «abc123» или ссылки
/// `https://хост/join/abc123`. Возвращает код и, если это ссылка, её адрес
/// (`схема://хост[:порт]`).
///
/// # Errors
/// Пустой ввод или ссылка не вида `/join/<код>`.
pub fn parse_invite_input(input: &str) -> Result<(String, Option<String>), String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Введите код приглашения".to_owned());
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let url = url::Url::parse(trimmed).map_err(|_| "Неверная ссылка".to_owned())?;
        let mut segments = url.path_segments().ok_or("Неверная ссылка")?;
        let (Some("join"), Some(code), None) =
            (segments.next(), segments.next(), segments.next())
        else {
            return Err("Ссылка должна быть вида https://сервер/join/КОД".to_owned());
        };
        if code.is_empty() {
            return Err("Ссылка должна быть вида https://сервер/join/КОД".to_owned());
        }
        let origin = url.origin().ascii_serialization();
        return Ok((code.to_owned(), Some(origin)));
    }
    if trimmed.chars().any(char::is_whitespace) || trimmed.contains('/') {
        return Err("Неверный код приглашения".to_owned());
    }
    Ok((trimmed.to_owned(), None))
}

/// Ссылка-приглашение для адреса инстанса.
#[must_use]
pub fn invite_url(origin: &str, code: &str) -> String {
    format!("{}/join/{code}", origin.trim_end_matches('/'))
}

impl ApiClient {
    /// `POST /api/spaces/:id/invite` — код приглашения (создаётся один раз).
    ///
    /// # Errors
    /// Сеть или отказ сервера (нет права `CREATE_INVITE`).
    pub async fn space_invite_code(&self, space_id: &str) -> Result<String, ApiError> {
        let path = format!("/api/spaces/{space_id}/invite");
        let reply: InviteCode = self.post_json(&path, &serde_json::json!({})).await?;
        Ok(reply.invite_code)
    }

    /// `POST /api/spaces/join` — вступить по коду; возвращает id пространства.
    ///
    /// # Errors
    /// Код не найден, бан, уже участник, пространство «по заявке».
    pub async fn join_by_code(&self, code: &str) -> Result<String, ApiError> {
        let reply: IdOnly = self
            .post_json("/api/spaces/join", &JoinBody { invite_code: code })
            .await?;
        Ok(reply.id)
    }

    /// `GET /api/spaces/:id` — пространство целиком.
    ///
    /// # Errors
    /// Сеть или отказ сервера.
    pub async fn space_full(
        &self,
        space_id: &str,
    ) -> Result<bs_types::SpaceWithChannelsAndMembers, ApiError> {
        self.get_json(&format!("/api/spaces/{space_id}")).await
    }

    /// `DELETE /api/spaces/:id/members/:uid` — выйти (себя) из пространства.
    ///
    /// # Errors
    /// Сеть или отказ сервера (владелец выйти не может).
    pub async fn leave_space(&self, space_id: &str, user_id: &str) -> Result<(), ApiError> {
        let path = format!("/api/spaces/{space_id}/members/{user_id}");
        self.delete_json::<serde_json::Value>(&path).await?;
        Ok(())
    }

    /// `POST /api/spaces/:id/channels` — создать канал. Сам канал придёт
    /// событием `channel_created`.
    ///
    /// # Errors
    /// Нет права `MANAGE_CHANNELS`, неверное имя или тип.
    pub async fn create_channel(
        &self,
        space_id: &str,
        channel: &NewChannel,
    ) -> Result<(), ApiError> {
        let path = format!("/api/spaces/{space_id}/channels");
        self.post_json::<serde_json::Value, _>(&path, channel)
            .await?;
        Ok(())
    }

    /// `POST /api/spaces/:id/categories` — создать категорию (придёт
    /// событием `category_created`).
    ///
    /// # Errors
    /// Нет права `MANAGE_CHANNELS`, пустое или длинное имя.
    pub async fn create_category(&self, space_id: &str, name: &str) -> Result<(), ApiError> {
        let path = format!("/api/spaces/{space_id}/categories");
        self.post_json::<serde_json::Value, _>(&path, &NameBody { name })
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{NewChannel, invite_url, parse_invite_input};

    #[test]
    fn bare_code_and_url_are_parsed() {
        assert_eq!(parse_invite_input(" abc123 "), Ok(("abc123".to_owned(), None)));
        assert_eq!(
            parse_invite_input("https://x.example:8443/join/abc123"),
            Ok(("abc123".to_owned(), Some("https://x.example:8443".to_owned())))
        );
        assert!(parse_invite_input("").is_err());
        assert!(parse_invite_input("https://x.example/other/abc").is_err());
        assert!(parse_invite_input("two words").is_err());
    }

    #[test]
    fn invite_url_has_no_double_slash() {
        assert_eq!(invite_url("https://a.b/", "c1"), "https://a.b/join/c1");
    }

    #[test]
    fn channel_body_uses_type_and_skips_empty() {
        let body = NewChannel {
            name: "новый".to_owned(),
            kind: "voice".to_owned(),
            topic: None,
            category_id: Some("k1".to_owned()),
        };
        assert_eq!(
            serde_json::to_value(&body).ok(),
            Some(serde_json::json!({"name":"новый","type":"voice","categoryId":"k1"}))
        );
    }
}
