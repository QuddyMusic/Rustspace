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
    /// Код из приложения-аутентификатора, если у аккаунта включена
    /// двухфакторная защита (расширение `bs-server`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub totp: Option<&'a str>,
    /// Запасной код вместо кода из приложения.
    #[serde(rename = "recoveryCode", skip_serializing_if = "Option::is_none")]
    pub recovery_code: Option<&'a str>,
}

impl<'a> LoginRequest<'a> {
    /// Запрос со вторым фактором: шесть цифр — код из приложения, всё
    /// остальное — запасной код.
    #[must_use]
    pub fn new(username: &'a str, password: &'a str, second_factor: Option<&'a str>) -> Self {
        let factor = second_factor.map(str::trim).filter(|code| !code.is_empty());
        let is_totp =
            factor.is_some_and(|code| code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()));
        Self {
            username,
            password,
            totp: factor.filter(|_| is_totp),
            recovery_code: factor.filter(|_| !is_totp),
        }
    }
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
        self.login_with_factor(username, password, None).await
    }

    /// Вход со вторым фактором (код из приложения или запасной код).
    ///
    /// # Errors
    /// Сетевые ошибки и отказ сервера; код `two_factor_required` значит,
    /// что аккаунт защищён и нужен код.
    pub async fn login_with_factor(
        &self,
        username: &str,
        password: &str,
        second_factor: Option<&str>,
    ) -> Result<AuthResponse, ApiError> {
        let body = LoginRequest::new(username, password, second_factor);
        self.post_json("/api/auth/login", &body).await
    }

    /// Логин с автоматической установкой токена в клиент.
    pub async fn login_and_attach(
        &mut self,
        username: &str,
        password: &str,
    ) -> Result<AuthResponse, ApiError> {
        self.login_with_factor_and_attach(username, password, None)
            .await
    }

    /// Как [`ApiClient::login_and_attach`], но со вторым фактором.
    ///
    /// # Errors
    /// Как у [`ApiClient::login_with_factor`].
    pub async fn login_with_factor_and_attach(
        &mut self,
        username: &str,
        password: &str,
        second_factor: Option<&str>,
    ) -> Result<AuthResponse, ApiError> {
        let response = self
            .login_with_factor(username, password, second_factor)
            .await?;
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
