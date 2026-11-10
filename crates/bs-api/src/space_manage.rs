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

/// Тело `PATCH /api/spaces/:id`; пустые поля не отправляются.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceUpdate {
    /// Новое имя.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Цвет значка.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_color: Option<String>,
    /// Описание.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `public`, `request` или `private`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<bs_types::SpaceVisibility>,
    /// Имя загруженного файла значка; пустая строка — убрать.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Имя загруженного файла баннера; пустая строка — убрать.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
    /// Показывать во внешнем каталоге.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory_listed: Option<bool>,
}

/// Заявка на вступление.
pub use bs_types::JoinRequest;

#[derive(Debug, Deserialize)]
struct JoinRequestList {
    requests: Vec<JoinRequest>,
}

#[derive(Debug, Serialize)]
struct DecisionBody {
    action: &'static str,
}

/// Тело `POST /api/spaces/:id/roles`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewRole {
    /// Название.
    pub name: String,
    /// Цвет `#rrggbb`.
    pub color: String,
    /// Права, decimal string; без поля сервер берёт базовые.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<String>,
}

/// Сторона якоря при перемещении роли.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleSide {
    /// Выше якорной роли.
    Above,
    /// Ниже якорной роли.
    Below,
}

/// Тело `PATCH /api/spaces/:id/roles/:roleId`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleUpdate {
    /// Новое название.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Новый цвет.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Новые права, decimal string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<String>,
    /// Версия прав, с которой начато редактирование (защита от гонки).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions_version: Option<String>,
    /// Поставить выше этой роли.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub above: Option<String>,
    /// Поставить ниже этой роли.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub below: Option<String>,
}

impl RoleUpdate {
    /// Перемещение рядом с якорной ролью.
    #[must_use]
    pub fn moved(anchor: &str, side: RoleSide) -> Self {
        let mut update = Self::default();
        match side {
            RoleSide::Above => update.above = Some(anchor.to_owned()),
            RoleSide::Below => update.below = Some(anchor.to_owned()),
        }
        update
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RoleIdBody<'a> {
    role_id: &'a str,
}

/// К чему относятся переопределения прав.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverrideScope {
    /// Канал.
    Channel(String),
    /// Категория.
    Category(String),
}

impl OverrideScope {
    fn base(&self) -> String {
        match self {
            Self::Channel(id) => format!("/api/channels/{id}/overrides"),
            Self::Category(id) => format!("/api/categories/{id}/overrides"),
        }
    }
}

/// Переопределение прав на канал или категорию для роли либо участника.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOverride {
    /// `role` или `member`.
    pub target_type: bs_types::OverrideTargetType,
    /// Роль или пользователь.
    pub target_id: String,
    /// Разрешённые биты, decimal string.
    pub allow: String,
    /// Запрещённые биты, decimal string.
    pub deny: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverrideBody<'a> {
    target_type: &'a str,
    target_id: &'a str,
    allow: &'a str,
    deny: &'a str,
}

/// Тело `PATCH /api/channels/:id`; пустые поля не отправляются.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ChannelUpdate {
    /// Новое имя.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Новая тема; пустая строка — убрать.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
}

/// Запись списка блокировок.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ban {
    /// Заблокированный.
    pub user_id: String,
    /// Причина.
    #[serde(default)]
    pub reason: Option<String>,
    /// Профиль заблокированного.
    #[serde(default)]
    pub user: Option<bs_types::User>,
    /// Профиль модератора.
    #[serde(default)]
    pub moderator: Option<bs_types::User>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BanBody<'a> {
    user_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OwnerBody<'a> {
    new_owner_id: &'a str,
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
        let (Some("join"), Some(code), None) = (segments.next(), segments.next(), segments.next())
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

    /// `PATCH /api/spaces/:id` — изменить пространство.
    ///
    /// # Errors
    /// Нет права `MANAGE_SPACE` или неверные поля.
    pub async fn update_space(&self, space_id: &str, update: &SpaceUpdate) -> Result<(), ApiError> {
        let path = format!("/api/spaces/{space_id}");
        self.patch_json::<serde_json::Value, _>(&path, update)
            .await?;
        Ok(())
    }

    /// `DELETE /api/spaces/:id` — удалить пространство (владелец).
    ///
    /// # Errors
    /// Не владелец или сеть.
    pub async fn delete_space(&self, space_id: &str) -> Result<(), ApiError> {
        let path = format!("/api/spaces/{space_id}");
        self.delete_json::<serde_json::Value>(&path).await?;
        Ok(())
    }

    /// `GET /api/spaces/:id/bans` — список блокировок.
    ///
    /// # Errors
    /// Нет права `BAN_MEMBERS`.
    pub async fn list_bans(&self, space_id: &str) -> Result<Vec<Ban>, ApiError> {
        self.get_json(&format!("/api/spaces/{space_id}/bans")).await
    }

    /// `POST /api/spaces/:id/bans` — заблокировать участника.
    ///
    /// # Errors
    /// Нет права или цель выше по роли.
    pub async fn ban_member(
        &self,
        space_id: &str,
        user_id: &str,
        reason: Option<&str>,
    ) -> Result<(), ApiError> {
        let path = format!("/api/spaces/{space_id}/bans");
        self.post_json::<serde_json::Value, _>(&path, &BanBody { user_id, reason })
            .await?;
        Ok(())
    }

    /// `DELETE /api/spaces/:id/bans/:uid` — разблокировать.
    ///
    /// # Errors
    /// Нет права `BAN_MEMBERS`.
    pub async fn unban_member(&self, space_id: &str, user_id: &str) -> Result<(), ApiError> {
        let path = format!("/api/spaces/{space_id}/bans/{user_id}");
        self.delete_json::<serde_json::Value>(&path).await?;
        Ok(())
    }

    /// `PATCH /api/spaces/:id/transfer-ownership` — передать владение.
    ///
    /// # Errors
    /// Не владелец, цель не участник.
    pub async fn transfer_ownership(
        &self,
        space_id: &str,
        new_owner: &str,
    ) -> Result<(), ApiError> {
        let path = format!("/api/spaces/{space_id}/transfer-ownership");
        self.patch_json::<serde_json::Value, _>(
            &path,
            &OwnerBody {
                new_owner_id: new_owner,
            },
        )
        .await?;
        Ok(())
    }

    /// `GET /api/spaces/:id/join-requests` — ожидающие заявки.
    ///
    /// # Errors
    /// Нет права `MANAGE_SPACE`.
    pub async fn pending_join_requests(
        &self,
        space_id: &str,
    ) -> Result<Vec<JoinRequest>, ApiError> {
        let path = format!("/api/spaces/{space_id}/join-requests?status=pending");
        Ok(self.get_json::<JoinRequestList>(&path).await?.requests)
    }

    /// `PATCH /api/spaces/:id/join-requests/:requestId` — принять или отклонить.
    ///
    /// # Errors
    /// Нет права, заявка уже рассмотрена.
    pub async fn decide_join_request(
        &self,
        space_id: &str,
        request_id: &str,
        accept: bool,
    ) -> Result<(), ApiError> {
        let path = format!("/api/spaces/{space_id}/join-requests/{request_id}");
        let body = DecisionBody {
            action: if accept { "accept" } else { "decline" },
        };
        self.patch_json::<serde_json::Value, _>(&path, &body)
            .await?;
        Ok(())
    }

    /// `PATCH /api/channels/:id` — имя и тема канала.
    ///
    /// # Errors
    /// Нет `MANAGE_CHANNELS`, неверное имя или тема.
    pub async fn update_channel(&self, id: &str, update: &ChannelUpdate) -> Result<(), ApiError> {
        self.patch_json::<serde_json::Value, _>(&format!("/api/channels/{id}"), update)
            .await?;
        Ok(())
    }

    /// `DELETE /api/channels/:id` — удалить канал со всеми сообщениями.
    ///
    /// # Errors
    /// Нет `MANAGE_CHANNELS`.
    pub async fn delete_channel(&self, id: &str) -> Result<(), ApiError> {
        self.delete_json::<serde_json::Value>(&format!("/api/channels/{id}"))
            .await?;
        Ok(())
    }

    /// `PATCH /api/categories/:id` — переименовать категорию.
    ///
    /// # Errors
    /// Нет `MANAGE_CHANNELS`, неверное имя.
    pub async fn rename_category(&self, id: &str, name: &str) -> Result<(), ApiError> {
        self.patch_json::<serde_json::Value, _>(
            &format!("/api/categories/{id}"),
            &NameBody { name },
        )
        .await?;
        Ok(())
    }

    /// `DELETE /api/categories/:id` — удалить категорию (каналы остаются).
    ///
    /// # Errors
    /// Нет `MANAGE_CHANNELS`.
    pub async fn delete_category(&self, id: &str) -> Result<(), ApiError> {
        self.delete_json::<serde_json::Value>(&format!("/api/categories/{id}"))
            .await?;
        Ok(())
    }

    /// `GET .../overrides` — переопределения прав (нужна `MANAGE_ROLES`).
    ///
    /// # Errors
    /// Нет права читать права.
    pub async fn list_overrides(
        &self,
        scope: &OverrideScope,
    ) -> Result<Vec<PermissionOverride>, ApiError> {
        self.get_json(&scope.base()).await
    }

    /// `PUT .../overrides` — создать или заменить переопределение.
    ///
    /// # Errors
    /// Нет права, нарушена иерархия или чужие биты.
    pub async fn put_override(
        &self,
        scope: &OverrideScope,
        entry: &PermissionOverride,
    ) -> Result<(), ApiError> {
        let target_type = match entry.target_type {
            bs_types::OverrideTargetType::Role => "role",
            bs_types::OverrideTargetType::Member => "member",
        };
        let body = OverrideBody {
            target_type,
            target_id: &entry.target_id,
            allow: &entry.allow,
            deny: &entry.deny,
        };
        self.put_json::<serde_json::Value, _>(&scope.base(), &body)
            .await?;
        Ok(())
    }

    /// `DELETE .../overrides/:type/:id` — убрать переопределение.
    ///
    /// # Errors
    /// Нет права, нарушена иерархия или чужие биты.
    pub async fn delete_override(
        &self,
        scope: &OverrideScope,
        target_type: bs_types::OverrideTargetType,
        target_id: &str,
    ) -> Result<(), ApiError> {
        let kind = match target_type {
            bs_types::OverrideTargetType::Role => "role",
            bs_types::OverrideTargetType::Member => "member",
        };
        let path = format!("{}/{kind}/{target_id}", scope.base());
        self.delete_json::<serde_json::Value>(&path).await?;
        Ok(())
    }

    /// `POST /api/spaces/:id/roles` — создать роль.
    ///
    /// # Errors
    /// Нет `MANAGE_ROLES`, занято имя, нарушена иерархия или права, которых нет у вас.
    pub async fn create_role(
        &self,
        space_id: &str,
        role: &NewRole,
    ) -> Result<bs_types::Role, ApiError> {
        self.post_json(&format!("/api/spaces/{space_id}/roles"), role)
            .await
    }

    /// `PATCH /api/spaces/:id/roles/:roleId` — изменить или переместить роль.
    ///
    /// # Errors
    /// См. [`Self::create_role`]; плюс `role_permissions_conflict`.
    pub async fn update_role(
        &self,
        space_id: &str,
        role_id: &str,
        update: &RoleUpdate,
    ) -> Result<bs_types::Role, ApiError> {
        self.patch_json(&format!("/api/spaces/{space_id}/roles/{role_id}"), update)
            .await
    }

    /// `DELETE /api/spaces/:id/roles/:roleId` — удалить роль.
    ///
    /// # Errors
    /// Нет права, иерархия, `@everyone` удалить нельзя.
    pub async fn delete_role(&self, space_id: &str, role_id: &str) -> Result<(), ApiError> {
        self.delete_json::<serde_json::Value>(&format!("/api/spaces/{space_id}/roles/{role_id}"))
            .await?;
        Ok(())
    }

    /// `POST /api/spaces/:id/members/:uid/roles` — выдать роль участнику.
    ///
    /// # Errors
    /// Нет права или иерархия.
    pub async fn add_member_role(
        &self,
        space_id: &str,
        user_id: &str,
        role_id: &str,
    ) -> Result<(), ApiError> {
        let path = format!("/api/spaces/{space_id}/members/{user_id}/roles");
        self.post_json::<serde_json::Value, _>(&path, &RoleIdBody { role_id })
            .await?;
        Ok(())
    }

    /// `DELETE /api/spaces/:id/members/:uid/roles/:roleId` — снять роль.
    ///
    /// # Errors
    /// Нет права или иерархия.
    pub async fn remove_member_role(
        &self,
        space_id: &str,
        user_id: &str,
        role_id: &str,
    ) -> Result<(), ApiError> {
        let path = format!("/api/spaces/{space_id}/members/{user_id}/roles/{role_id}");
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
        assert_eq!(
            parse_invite_input(" abc123 "),
            Ok(("abc123".to_owned(), None))
        );
        assert_eq!(
            parse_invite_input("https://x.example:8443/join/abc123"),
            Ok((
                "abc123".to_owned(),
                Some("https://x.example:8443".to_owned())
            ))
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
