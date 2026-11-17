//! Настройки голосового соединения и обработки звука.

use serde::{Deserialize, Serialize};

/// Параметры аппаратной/программной обработки микрофона.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioProcessingSettings {
    /// Эхоподавление (AEC).
    pub echo_cancellation: bool,
    /// Шумоподавление (NS).
    pub noise_suppression: bool,
    /// Автоматическая регулировка усиления (AGC).
    pub auto_gain_control: bool,
    /// Предпочитать аппаратную обработку, если доступна (важно для macOS/iOS).
    pub prefer_hardware: bool,
}

impl Default for AudioProcessingSettings {
    fn default() -> Self {
        Self {
            echo_cancellation: true,
            noise_suppression: true,
            auto_gain_control: true,
            prefer_hardware: cfg!(target_os = "ios"),
        }
    }
}

/// Полная конфигурация голосовой сессии.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VoiceConfig {
    /// Подключаться и сразу публиковать микрофон.
    pub auto_publish_mic: bool,
    /// Стартовое состояние «микрофон выключен».
    pub start_muted: bool,
    /// Стартовое состояние «звук выключен» (deafen).
    pub start_deafened: bool,
    /// Обработка звука.
    pub processing: AudioProcessingSettings,
    /// Включать звук новых участников по умолчанию.
    pub auto_subscribe: bool,
    /// Битрейт микрофона, кбит/с; `None` — по умолчанию LiveKit.
    pub mic_bitrate_kbps: Option<u32>,
}
