//! Аутентификация: логин и получение текущего пользователя.

use bs_types::User;
use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::error::ApiError;

/// Тело запроса `POST /api/auth/login`.
#[derive(Debug, Clone, Serialize)]
pub struct LoginRequest<'a> {
    /// Имя пользователя.
    pub username: &'a str,
    /// Пароль в открытом виде (TLS обязателен).
    pub password: &'a str,
}

/// Ответ `POST /api/auth/login`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponse {
    /// JWT для `Authorization: Bearer`.
    pub token: String,
    /// Профиль вошедшего пользователя.
    pub user: User,
}

impl ApiClient {
    /// `POST /api/auth/login` — обменивает учётные данные на токен.
    ///
    /// Токен **не** применяется к клиенту автоматически: используйте
    /// [`ApiClient::login_and_attach`], если нужно сразу авторизоваться.
    pub async fn login(&self, username: &str, password: &str) -> Result<AuthResponse, ApiError> {
        let body = LoginRequest { username, password };
        self.post_json("/api/auth/login", &body).await
    }

    /// Логин с автоматической установкой токена в клиент.
    pub async fn login_and_attach(
        &mut self,
        username: &str,
        password: &str,
    ) -> Result<AuthResponse, ApiError> {
        let response = self.login(username, password).await?;
        self.set_token(Some(response.token.clone()));
        Ok(response)
    }

    /// `GET /api/users/@me` — профиль текущего пользователя.
    ///
    /// Требует установленного токена.
    pub async fn me(&self) -> Result<User, ApiError> {
        self.get_json("/api/users/@me").await
    }
}
