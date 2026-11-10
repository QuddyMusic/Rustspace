//! Настройки уведомлений пространств и каналов.
//!
//! Хранятся на инстансе, где живёт пространство: у каждого инстанса — свои.

use bs_types::{
    NotificationSetting, NotificationSettingsResponse, UpdateNotificationSettingRequest,
};

use crate::client::ApiClient;
use crate::error::ApiError;

impl ApiClient {
    /// `GET /api/users/@me/notification-settings` — все сохранённые настройки.
    ///
    /// # Errors
    /// Сетевая ошибка или отказ сервера.
    pub async fn notification_settings(&self) -> Result<Vec<NotificationSetting>, ApiError> {
        let body: NotificationSettingsResponse = self
            .get_json("/api/users/@me/notification-settings")
            .await?;
        Ok(body.settings)
    }

    /// `PATCH /api/spaces/:id/notification-settings` — настройка пространства.
    ///
    /// # Errors
    /// Не участник пространства или неверное тело.
    pub async fn update_space_notifications(
        &self,
        space_id: &str,
        change: &UpdateNotificationSettingRequest,
    ) -> Result<NotificationSetting, ApiError> {
        self.patch_json(
            &format!("/api/spaces/{space_id}/notification-settings"),
            change,
        )
        .await
    }

    /// `PATCH /api/channels/:id/notification-settings` — настройка канала.
    ///
    /// # Errors
    /// Канал не виден или неверное тело.
    pub async fn update_channel_notifications(
        &self,
        channel_id: &str,
        change: &UpdateNotificationSettingRequest,
    ) -> Result<NotificationSetting, ApiError> {
        self.patch_json(
            &format!("/api/channels/{channel_id}/notification-settings"),
            change,
        )
        .await
    }
}
