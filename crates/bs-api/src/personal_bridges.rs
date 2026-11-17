//! Личные мосты Rustspace (`/api/rustspace/personal-bridges…`): свои
//! аккаунты Matrix и Telegram, чаты которых становятся личными переписками.

use serde::Deserialize;
use serde_json::json;

use crate::client::ApiClient;
use crate::error::ApiError;

/// Связанный чат.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalChat {
    /// Идентификатор связи.
    pub id: String,
    /// Чат на той стороне.
    pub remote_chat: String,
    /// Название.
    pub title: String,
    /// Групповой.
    #[serde(default)]
    pub group: bool,
    /// Личная переписка.
    pub dm_channel_id: String,
}

/// Подключённый аккаунт.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalAccount {
    /// Идентификатор.
    pub id: String,
    /// Сеть: `matrix`, `telegram`.
    pub kind: String,
    /// Адрес аккаунта на той стороне.
    #[serde(default)]
    pub remote_id: String,
    /// Имя для показа.
    #[serde(default)]
    pub display: String,
    /// `active`, `starting` или `error`.
    #[serde(default)]
    pub state: String,
    /// Текст ошибки.
    #[serde(default)]
    pub error: Option<String>,
    /// Связанные чаты.
    #[serde(default)]
    pub chats: Vec<PersonalChat>,
}

/// Сведения о сети.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalNetwork {
    /// Доступна.
    #[serde(default)]
    pub available: bool,
    /// У сервера есть свои ключи приложения (Telegram).
    #[serde(default)]
    pub server_keys: bool,
}

/// Сети сервера.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct PersonalNetworks {
    /// Matrix.
    #[serde(default)]
    pub matrix: PersonalNetwork,
    /// Telegram.
    #[serde(default)]
    pub telegram: PersonalNetwork,
}

/// Ответ `GET /api/rustspace/personal-bridges`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct PersonalBridges {
    /// Сети.
    #[serde(default)]
    pub networks: PersonalNetworks,
    /// Свои аккаунты.
    #[serde(default)]
    pub accounts: Vec<PersonalAccount>,
}

/// Чат на той стороне, который можно связать.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RemoteChat {
    /// Идентификатор.
    pub id: String,
    /// Название.
    pub title: String,
    /// Групповой.
    #[serde(default)]
    pub group: bool,
    /// Уже связан.
    #[serde(default)]
    pub linked: bool,
}

/// Шаг входа в Telegram.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramStep {
    /// Нужен облачный пароль.
    #[serde(default)]
    pub password_required: bool,
    /// Подсказка к паролю.
    #[serde(default)]
    pub hint: Option<String>,
    /// Готовый аккаунт.
    #[serde(default)]
    pub account: Option<PersonalAccount>,
}

/// Вход в Matrix: токен или логин с паролем.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatrixLogin {
    /// Токен доступа.
    Token(String),
    /// Логин и пароль.
    Password(String, String),
}

const BASE: &str = "/api/rustspace/personal-bridges";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginId {
    login_id: String,
}

impl ApiClient {
    /// Сети и свои аккаунты.
    ///
    /// # Errors
    /// Ошибки запроса (сервер без личных мостов отвечает 404).
    pub async fn personal_bridges(&self) -> Result<PersonalBridges, ApiError> {
        self.get_json(BASE).await
    }

    /// Подключает аккаунт Matrix.
    ///
    /// # Errors
    /// Ошибки запроса и входа на той стороне.
    pub async fn add_matrix_bridge(
        &self,
        homeserver: &str,
        login: &MatrixLogin,
    ) -> Result<PersonalAccount, ApiError> {
        let body = match login {
            MatrixLogin::Token(token) => json!({"homeserver": homeserver, "accessToken": token}),
            MatrixLogin::Password(user, password) => {
                json!({"homeserver": homeserver, "login": user, "password": password})
            }
        };
        self.post_json(&format!("{BASE}/matrix"), &body).await
    }

    /// Начинает вход в Telegram: код уходит на номер. Возвращает
    /// идентификатор входа.
    ///
    /// # Errors
    /// Ошибки запроса и Telegram.
    pub async fn telegram_login(
        &self,
        phone: &str,
        app: Option<(i32, &str)>,
    ) -> Result<String, ApiError> {
        let mut body = json!({"phone": phone});
        if let Some((id, hash)) = app {
            body["apiId"] = json!(id);
            body["apiHash"] = json!(hash);
        }
        let login: LoginId = self
            .post_json(&format!("{BASE}/telegram/login"), &body)
            .await?;
        Ok(login.login_id)
    }

    /// Код из Telegram.
    ///
    /// # Errors
    /// Ошибки запроса, неверный код.
    pub async fn telegram_code(&self, login: &str, code: &str) -> Result<TelegramStep, ApiError> {
        self.post_json(
            &format!("{BASE}/telegram/code"),
            &json!({"loginId": login, "code": code}),
        )
        .await
    }

    /// Облачный пароль Telegram.
    ///
    /// # Errors
    /// Ошибки запроса, неверный пароль.
    pub async fn telegram_password(
        &self,
        login: &str,
        password: &str,
    ) -> Result<TelegramStep, ApiError> {
        self.post_json(
            &format!("{BASE}/telegram/password"),
            &json!({"loginId": login, "password": password}),
        )
        .await
    }

    /// Отключает аккаунт.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn remove_personal_bridge(&self, id: &str) -> Result<serde_json::Value, ApiError> {
        self.delete_json(&format!("{BASE}/{id}")).await
    }

    /// Чаты на той стороне.
    ///
    /// # Errors
    /// Ошибки запроса и сети на той стороне.
    pub async fn personal_remote_chats(&self, id: &str) -> Result<Vec<RemoteChat>, ApiError> {
        self.get_json(&format!("{BASE}/{id}/remote-chats")).await
    }

    /// Связывает чат с личной перепиской.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn link_personal_chat(
        &self,
        id: &str,
        chat: &RemoteChat,
    ) -> Result<serde_json::Value, ApiError> {
        self.put_json(
            &format!("{BASE}/{id}/chats"),
            &json!({"remoteChat": chat.id, "title": chat.title, "group": chat.group}),
        )
        .await
    }

    /// Отвязывает чат (переписка остаётся).
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn unlink_personal_chat(
        &self,
        id: &str,
        chat: &str,
    ) -> Result<serde_json::Value, ApiError> {
        self.delete_json(&format!("{BASE}/{id}/chats/{chat}")).await
    }
}
