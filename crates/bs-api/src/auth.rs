//! Аутентификация: логин и получение текущего пользователя.

use bs_types::User;
use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::error::ApiError;

/// Тело запроса `POST /api/auth/login`.
///
/// `Debug` не печатает пароль.
#[derive(Clone, Serialize)]
pub struct LoginRequest<'a> {
    /// Имя пользователя.
    pub username: &'a str,
    /// Пароль в открытом виде (TLS обязателен).
    pub password: &'a str,
}

impl std::fmt::Debug for LoginRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginRequest")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Ответ `POST /api/auth/login`.
///
/// `Debug` не печатает токен.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponse {
    /// JWT для `Authorization: Bearer`.
    pub token: String,
    /// Профиль вошедшего пользователя.
    pub user: User,
}

impl std::fmt::Debug for AuthResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthResponse")
            .field("token", &"<redacted>")
            .field("user", &self.user)
            .finish()
    }
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
