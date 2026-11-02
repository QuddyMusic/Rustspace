//! Профили других пользователей: `GET /api/users/:id` и общие связи.

use bs_types::User;
use serde::Deserialize;

use crate::client::ApiClient;
use crate::error::ApiError;

/// Общее пространство из ответа `/mutuals`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutualSpace {
    /// Идентификатор.
    pub id: String,
    /// Название.
    pub name: String,
    /// Значок.
    #[serde(default)]
    pub icon: Option<String>,
}

/// Общие друзья и пространства.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mutuals {
    /// Общие друзья.
    #[serde(default)]
    pub mutual_friends: Vec<User>,
    /// Общие пространства.
    #[serde(default)]
    pub mutual_spaces: Vec<MutualSpace>,
}

/// Кодирует идентификатор для пути.
fn segment(id: &str) -> String {
    url::form_urlencoded::byte_serialize(id.as_bytes()).collect()
}

impl ApiClient {
    /// `GET /api/users/:id` — профиль пользователя.
    pub async fn user(&self, id: &str) -> Result<User, ApiError> {
        self.get_json(&format!("/api/users/{}", segment(id))).await
    }

    /// `GET /api/users/:id/mutuals` — общие друзья и пространства.
    pub async fn mutuals(&self, id: &str) -> Result<Mutuals, ApiError> {
        self.get_json(&format!("/api/users/{}/mutuals", segment(id)))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::Mutuals;

    #[test]
    fn mutuals_tolerate_missing_fields() {
        let parsed: Mutuals = serde_json::from_str(
            r#"{"mutualSpaces":[{"id":"s","name":"Клуб","icon":null,"avatarColor":"mint"}]}"#,
        )
        .unwrap_or_default();
        assert!(parsed.mutual_friends.is_empty());
        assert_eq!(parsed.mutual_spaces[0].name, "Клуб");
    }
}
