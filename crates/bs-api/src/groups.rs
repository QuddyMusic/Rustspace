//! Групповые личные беседы: создание, состав, владелец, название.

use bs_types::DmChannel;
use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::error::ApiError;

/// Наибольшее число участников группы, включая создателя (как на сервере).
pub const GROUP_MAX_MEMBERS: usize = 10;
/// Длина названия группы: от 1 до 50 символов.
pub const GROUP_NAME_MAX: usize = 50;

/// Кого позвать: локальный id или федеративная личность.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMember {
    /// Id на инстансе, где создаётся группа.
    pub id: String,
    /// Id на домашнем инстансе (для пользователей с других серверов).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_user_id: Option<String>,
    /// Домашний инстанс.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_instance: Option<String>,
}

impl GroupMember {
    /// Пользователь этого же сервера.
    #[must_use]
    pub fn local(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            home_user_id: None,
            home_instance: None,
        }
    }
}

#[derive(Serialize)]
struct CreateGroup<'a> {
    users: &'a [GroupMember],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AddMember<'a> {
    user_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Transfer<'a> {
    new_owner_id: &'a str,
}

/// Изменение метаданных группы; не заданное поле не трогается.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GroupUpdate {
    /// Новое название; `Some(None)` сбрасывает его.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<Option<String>>,
    /// Новый значок (имя загруженного файла); `Some(None)` убирает.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<Option<String>>,
}

/// Кому отправить приглашение в пространство.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum InviteTarget {
    /// Пользователь этого инстанса.
    #[serde(rename_all = "camelCase")]
    Local {
        /// Его id.
        user_id: String,
    },
    /// Пользователь другого инстанса.
    #[serde(rename_all = "camelCase")]
    Remote {
        /// Id на домашнем инстансе.
        home_user_id: String,
        /// Домашний инстанс.
        home_instance: String,
    },
}

/// Тело `POST /api/dm/space-invite`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceInviteRequest {
    /// Получатель (должен быть в друзьях).
    pub target: InviteTarget,
    /// Пространство на его инстансе.
    pub space_id: String,
    /// Инстанс пространства; пустая строка — этот же.
    pub space_instance_origin: String,
    /// Уже выданный код приглашения.
    pub invite_code: String,
}

/// Ответ `POST /api/dm/space-invite`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceInviteResponse {
    /// Беседа, куда ушло приглашение.
    pub dm_channel_id: String,
    /// Id системного сообщения.
    pub message_id: String,
}

impl ApiClient {
    /// `POST /api/dm/space-invite` — отправить другу приглашение в
    /// пространство системным сообщением в ЛС.
    ///
    /// # Errors
    /// Сеть или отказ сервера (`not_a_friend`, `invite_invalid`, …).
    pub async fn send_space_invite(
        &self,
        request: &SpaceInviteRequest,
    ) -> Result<SpaceInviteResponse, ApiError> {
        self.post_json("/api/dm/space-invite", request).await
    }

    /// `POST /api/dm/group` — создать группу из `users` (без себя, от 2 человек).
    ///
    /// # Errors
    /// Мало или слишком много участников, пользователь не найден.
    pub async fn create_group_dm(&self, users: &[GroupMember]) -> Result<DmChannel, ApiError> {
        self.post_json("/api/dm/group", &CreateGroup { users })
            .await
    }

    /// `PATCH /api/dm/:id` — название и значок группы (только владелец).
    ///
    /// # Errors
    /// Не владелец или неверное название.
    pub async fn update_group_dm(
        &self,
        id: &str,
        change: &GroupUpdate,
    ) -> Result<serde_json::Value, ApiError> {
        self.patch_json(&format!("/api/dm/{id}"), change).await
    }

    /// `POST /api/dm/:id/members` — добавить друга (может любой участник).
    ///
    /// # Errors
    /// Не друг, группа заполнена или это не группа.
    pub async fn add_group_member(
        &self,
        id: &str,
        user_id: &str,
    ) -> Result<serde_json::Value, ApiError> {
        self.post_json(&format!("/api/dm/{id}/members"), &AddMember { user_id })
            .await
    }

    /// `DELETE /api/dm/:id/members/:user` — убрать участника (только владелец).
    ///
    /// # Errors
    /// Не владелец, либо цель — сам владелец.
    pub async fn remove_group_member(
        &self,
        id: &str,
        user_id: &str,
    ) -> Result<serde_json::Value, ApiError> {
        self.delete_json(&format!("/api/dm/{id}/members/{user_id}"))
            .await
    }

    /// `DELETE /api/dm/:id/members` — покинуть группу.
    ///
    /// # Errors
    /// Вы не участник или это не группа.
    pub async fn leave_group_dm(&self, id: &str) -> Result<serde_json::Value, ApiError> {
        self.delete_json(&format!("/api/dm/{id}/members")).await
    }

    /// `DELETE /api/rustspace/dm/:id` — удалить переписку у себя: мостовая
    /// удаляется с историей, обычная закрывается. Сервер без расширения
    /// (оригинальный Backspace) — просто закрываем (`DELETE /api/dm/:id`).
    ///
    /// # Errors
    /// Не участник или сервер недоступен.
    pub async fn delete_own_dm(&self, id: &str) -> Result<serde_json::Value, ApiError> {
        match self.delete_json(&format!("/api/rustspace/dm/{id}")).await {
            Err(err)
                if err.status() == Some(reqwest::StatusCode::NOT_FOUND) && err.code().is_none() =>
            {
                self.delete_json(&format!("/api/dm/{id}")).await
            }
            other => other,
        }
    }

    /// `POST /api/rustspace/dm/:id/resync` — перечитать историю переписки,
    /// связанной с личным мостом (недостающее дописывается в фоне).
    ///
    /// # Errors
    /// Это не мостовая переписка, мост не подключён или сервер недоступен.
    pub async fn resync_dm(&self, id: &str) -> Result<serde_json::Value, ApiError> {
        self.post_json(
            &format!("/api/rustspace/dm/{id}/resync"),
            &serde_json::json!({}),
        )
        .await
    }

    /// `POST /api/dm/:id/transfer` — передать права владельца.
    ///
    /// # Errors
    /// Не владелец или цель не участник.
    pub async fn transfer_group_owner(
        &self,
        id: &str,
        new_owner_id: &str,
    ) -> Result<serde_json::Value, ApiError> {
        self.post_json(
            &format!("/api/dm/{id}/transfer"),
            &Transfer { new_owner_id },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_serializes_only_given_fields() {
        let rename = GroupUpdate {
            name: Some(Some("Команда".to_owned())),
            icon: None,
        };
        assert_eq!(
            serde_json::to_string(&rename).unwrap_or_default(),
            r#"{"name":"Команда"}"#
        );
        let clear = GroupUpdate {
            name: Some(None),
            icon: None,
        };
        assert_eq!(
            serde_json::to_string(&clear).unwrap_or_default(),
            r#"{"name":null}"#
        );
    }

    #[test]
    fn local_member_has_only_id() {
        let body = serde_json::to_string(&GroupMember::local("u1")).unwrap_or_default();
        assert_eq!(body, r#"{"id":"u1"}"#);
    }
}
