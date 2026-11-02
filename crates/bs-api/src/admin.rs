//! Администрирование инстанса и управление собственным аккаунтом:
//! настройки, пользователи, хранилище, пароль и удаление аккаунта.
//!
//! Админские эндпоинты отвечают `403`, если сессия не администратора.

use serde::{Deserialize, Serialize};
use url::form_urlencoded;

use crate::client::ApiClient;
use crate::error::ApiError;

#[allow(clippy::struct_excessive_bools, reason = "зеркало ответа сервера")]
/// Настройки инстанса (`GET /api/settings/instance`); лишние поля сервера
/// (каталог, статистика) игнорируются.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstanceSettings {
    /// Название, 1–32 символа.
    pub instance_name: String,
    /// Открыта ли регистрация.
    pub registration_open: bool,
    /// Открыта ли регистрация федеративных аккаунтов.
    pub federated_registration_open: bool,
    /// Каталог пространств включён.
    pub discovery_enabled: bool,
    /// Ключ Klipy, замаскированный (`****abcd`).
    pub gif_api_key: Option<String>,
    /// Включён ли поиск GIF.
    pub gif_enabled: Option<bool>,
    /// Лимит загрузки, МБ.
    pub max_upload_size_mb: u32,
    /// Пересылка сообщений между инстансами.
    pub federation_relay_enabled: bool,
    /// Срок хранения очереди пересылки, дни.
    pub federation_relay_ttl_days: u32,
    /// Период автоматической смены ключей, дни.
    pub default_auto_rotate_interval_days: u32,
    /// Принимать пиринг автоматически.
    pub auto_accept_peering: bool,
}

/// Изменение настроек: передаются только заданные поля.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceSettingsPatch {
    /// Название.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_name: Option<String>,
    /// Регистрация.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_open: Option<bool>,
    /// Федеративная регистрация.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub federated_registration_open: Option<bool>,
    /// Каталог пространств.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovery_enabled: Option<bool>,
    /// Ключ Klipy; пустая строка удаляет.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gif_api_key: Option<String>,
    /// Лимит загрузки, МБ.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_upload_size_mb: Option<u32>,
    /// Пересылка между инстансами.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub federation_relay_enabled: Option<bool>,
    /// Срок очереди пересылки, дни.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub federation_relay_ttl_days: Option<u32>,
    /// Период смены ключей, дни.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_auto_rotate_interval_days: Option<u32>,
    /// Автоприём пиринга.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_accept_peering: Option<bool>,
}

/// Пользователь в админском списке.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminUser {
    /// Идентификатор.
    pub id: String,
    /// Логин.
    pub username: String,
    /// Отображаемое имя.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Администратор ли.
    #[serde(default)]
    pub is_admin: bool,
    /// Удалён ли.
    #[serde(default)]
    pub is_deleted: bool,
    /// Домашний инстанс федеративного пользователя.
    #[serde(default)]
    pub home_instance: Option<String>,
    /// Когда создан, мс.
    #[serde(default)]
    pub created_at: i64,
}

/// Страница пользователей.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminUserPage {
    /// Пользователи страницы.
    #[serde(default)]
    pub users: Vec<AdminUser>,
    /// Всего подходящих.
    #[serde(default)]
    pub total: u64,
    /// Номер страницы с 1.
    #[serde(default)]
    pub page: u32,
    /// Размер страницы.
    #[serde(default)]
    pub page_size: u32,
}

/// Строка разбивки хранилища по типам.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StorageBreakdown {
    /// `image`, `video`, `audio`, `document`, `other`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Файлов.
    #[serde(default)]
    pub count: u64,
    /// Байт.
    #[serde(default)]
    pub size: u64,
}

/// Сводка по хранилищу.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StorageStats {
    /// Файлов всего.
    pub total_files: u64,
    /// Байт всего.
    pub total_size: u64,
    /// Файлов, на которые есть ссылки.
    pub referenced_files: u64,
    /// Байт в таких файлах.
    pub referenced_size: u64,
    /// Файлов-сирот.
    pub orphaned_files: u64,
    /// Байт в сиротах.
    pub orphaned_size: u64,
    /// Незавершённых загрузок (старше часа).
    pub stale_tus_sessions: u64,
    /// Байт в них.
    pub stale_tus_size: u64,
    /// Разбивка по типам.
    pub breakdown: Vec<StorageBreakdown>,
}

/// Итог очистки.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CleanupResult {
    /// Это пробный запуск.
    pub dry_run: bool,
    /// Удалено файлов.
    pub deleted_files: u64,
    /// Освобождено байт.
    pub freed_bytes: u64,
    /// Ошибки.
    pub errors: Vec<String>,
}

/// Ответ на сброс пароля пользователя.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordReset {
    /// Временный пароль.
    pub temporary_password: String,
}

#[derive(Serialize)]
struct RoleBody {
    #[serde(rename = "isAdmin")]
    is_admin: bool,
}

#[derive(Serialize)]
struct DryRun {
    #[serde(rename = "dryRun")]
    dry_run: bool,
}

#[derive(Serialize)]
struct DeleteAccountBody<'a> {
    password: &'a str,
    username: &'a str,
}

/// Путь списка пользователей.
fn users_path(query: &str, page: u32, show_deleted: bool) -> String {
    let mut qs = form_urlencoded::Serializer::new(String::new());
    if !query.trim().is_empty() {
        qs.append_pair("q", query.trim());
    }
    qs.append_pair("page", &page.max(1).to_string());
    qs.append_pair("pageSize", "30");
    if show_deleted {
        qs.append_pair("showDeleted", "true");
    }
    format!("/api/admin/users?{}", qs.finish())
}

impl ApiClient {
    /// `GET /api/settings/instance` (админ).
    ///
    /// # Errors
    /// Ошибки запроса; `403` для не-админа.
    pub async fn instance_settings(&self) -> Result<InstanceSettings, ApiError> {
        self.get_json("/api/settings/instance").await
    }

    /// `PATCH /api/settings/instance` (админ).
    ///
    /// # Errors
    /// Ошибки запроса, в том числе проверки сервера.
    pub async fn update_instance_settings(
        &self,
        patch: &InstanceSettingsPatch,
    ) -> Result<InstanceSettings, ApiError> {
        self.patch_json("/api/settings/instance", patch).await
    }

    /// `GET /api/admin/users` (админ).
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn admin_users(
        &self,
        query: &str,
        page: u32,
        show_deleted: bool,
    ) -> Result<AdminUserPage, ApiError> {
        self.get_json(&users_path(query, page, show_deleted)).await
    }

    /// `PATCH /api/admin/users/:id/role` — назначить или снять администратора.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn admin_set_role(
        &self,
        user_id: &str,
        is_admin: bool,
    ) -> Result<serde_json::Value, ApiError> {
        self.patch_json(
            &format!("/api/admin/users/{user_id}/role"),
            &RoleBody { is_admin },
        )
        .await
    }

    /// `POST /api/admin/users/:id/reset-password` — временный пароль.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn admin_reset_password(&self, user_id: &str) -> Result<PasswordReset, ApiError> {
        self.post_json(
            &format!("/api/admin/users/{user_id}/reset-password"),
            &serde_json::json!({}),
        )
        .await
    }

    /// `DELETE /api/admin/users/:id` — удалить пользователя.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn admin_delete_user(&self, user_id: &str) -> Result<serde_json::Value, ApiError> {
        self.delete_json(&format!("/api/admin/users/{user_id}"))
            .await
    }

    /// `GET /api/admin/storage/stats`.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn storage_stats(&self) -> Result<StorageStats, ApiError> {
        self.get_json("/api/admin/storage/stats").await
    }

    /// `POST /api/admin/storage/cleanup` — удалить осиротевшие файлы.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn storage_cleanup(&self, dry_run: bool) -> Result<CleanupResult, ApiError> {
        self.post_json("/api/admin/storage/cleanup", &DryRun { dry_run })
            .await
    }

    /// `DELETE /api/users/@me` — удалить свой аккаунт.
    ///
    /// # Errors
    /// Ошибки запроса (неверный пароль, несовпадение имени).
    pub async fn delete_account(
        &self,
        password: &str,
        username: &str,
    ) -> Result<serde_json::Value, ApiError> {
        self.request(
            reqwest::Method::DELETE,
            "/api/users/@me",
            Some(&DeleteAccountBody { password, username }),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::{InstanceSettings, InstanceSettingsPatch, users_path};

    #[test]
    fn patch_sends_only_set_fields() {
        let patch = InstanceSettingsPatch {
            registration_open: Some(false),
            ..InstanceSettingsPatch::default()
        };
        assert_eq!(
            serde_json::to_string(&patch).unwrap(),
            r#"{"registrationOpen":false}"#
        );
    }

    #[test]
    fn settings_tolerate_unknown_and_missing_fields() {
        let s: InstanceSettings =
            serde_json::from_str(r#"{"instanceName":"A","registrationOpen":true,"x":1}"#).unwrap();
        assert_eq!(s.instance_name, "A");
        assert!(s.registration_open && !s.auto_accept_peering);
    }

    #[test]
    fn user_paths() {
        assert_eq!(
            users_path("", 0, false),
            "/api/admin/users?page=1&pageSize=30"
        );
        assert_eq!(
            users_path(" bo b", 2, true),
            "/api/admin/users?q=bo+b&page=2&pageSize=30&showDeleted=true"
        );
    }
}
