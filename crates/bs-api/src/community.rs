//! Пространства и профиль: создание, каталог (Explore), вступление, правка
//! своего профиля.
//!
//! Маршруты и поля — как в `packages/server/src/routes/{spaces,explore,users}.ts`.

use bs_types::{
    ChosenUserStatus, ExploreSpace, SpaceVisibility, SpaceWithChannelsAndMembers, User,
};
use serde::{Deserialize, Serialize};
use url::form_urlencoded;

use crate::client::ApiClient;
use crate::error::ApiError;

/// Тело `POST /api/spaces`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSpaceRequest {
    /// Имя, 1–100 символов.
    pub name: String,
    /// Видимость; сервер по умолчанию делает пространство приватным.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<SpaceVisibility>,
    /// Описание, до 200 символов.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Цвет значка из палитры (`mint`, `sky`, `lavender`, `coral`, `rose`, `teal`, `amber`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_color: Option<String>,
}

/// Ответ `GET /api/spaces/explore`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorePage {
    /// Найденные пространства.
    #[serde(default)]
    pub spaces: Vec<ExploreSpace>,
    /// Сколько всего подошло под запрос.
    #[serde(default)]
    pub total: u64,
    /// Включён ли каталог на инстансе.
    #[serde(default)]
    pub discovery_enabled: bool,
}

/// Тело `PATCH /api/users/@me`: передаются только изменяемые поля.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProfileRequest {
    /// Отображаемое имя, до 32 символов; пустая строка сбрасывает.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Раздел «Обо мне», до 190 символов.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    /// Пользовательский статус, до 128 символов.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_status: Option<String>,
    /// Выбранный статус присутствия.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ChosenUserStatus>,
    /// Цвет аватара из палитры.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_color: Option<String>,
    /// Показывать ли пользователя в поиске по инстансу.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discoverable: Option<bool>,
    /// Показывать ли друзьям свою активность.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_activity: Option<bool>,
}

#[derive(Serialize)]
struct RequestJoinBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
}

impl ApiClient {
    /// `POST /api/spaces` — создать пространство.
    pub async fn create_space(
        &self,
        request: &CreateSpaceRequest,
    ) -> Result<SpaceWithChannelsAndMembers, ApiError> {
        self.post_json("/api/spaces", request).await
    }

    /// `GET /api/spaces/explore` — каталог пространств этого инстанса.
    pub async fn explore_spaces(
        &self,
        query: &str,
        limit: u16,
        offset: u32,
    ) -> Result<ExplorePage, ApiError> {
        // Сериализатор не `Send`: он не должен жить через `.await`.
        let path = {
            let mut qs = form_urlencoded::Serializer::new(String::new());
            if !query.trim().is_empty() {
                qs.append_pair("q", query.trim());
            }
            qs.append_pair("limit", &limit.to_string());
            qs.append_pair("offset", &offset.to_string());
            format!("/api/spaces/explore?{}", qs.finish())
        };
        self.get_json(&path).await
    }

    /// `POST /api/spaces/:id/public-join` — вступить в публичное пространство.
    pub async fn join_public_space(
        &self,
        space_id: &str,
    ) -> Result<SpaceWithChannelsAndMembers, ApiError> {
        let path = format!("/api/spaces/{space_id}/public-join");
        self.post_json(&path, &serde_json::json!({})).await
    }

    /// `POST /api/spaces/:id/request-join` — подать заявку на вступление.
    pub async fn request_join_space(
        &self,
        space_id: &str,
        message: Option<&str>,
    ) -> Result<(), ApiError> {
        let path = format!("/api/spaces/{space_id}/request-join");
        self.post_json::<serde_json::Value, _>(&path, &RequestJoinBody { message })
            .await?;
        Ok(())
    }

    /// `PATCH /api/users/@me` — изменить свой профиль.
    pub async fn update_me(&self, request: &UpdateProfileRequest) -> Result<User, ApiError> {
        self.patch_json("/api/users/@me", request).await
    }
}

#[cfg(test)]
mod tests {
    use super::{CreateSpaceRequest, UpdateProfileRequest};
    use bs_types::SpaceVisibility;

    #[test]
    fn create_space_body_is_camel_case_and_skips_empty() {
        let body = CreateSpaceRequest {
            name: "Dev".into(),
            visibility: Some(SpaceVisibility::Request),
            description: None,
            avatar_color: Some("lavender".into()),
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"name":"Dev","visibility":"request","avatarColor":"lavender"})
        );
    }

    #[test]
    fn profile_patch_sends_only_changed_fields() {
        let body = UpdateProfileRequest {
            custom_status: Some("hi".into()),
            ..UpdateProfileRequest::default()
        };
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            serde_json::json!({"customStatus":"hi"})
        );
    }
}
