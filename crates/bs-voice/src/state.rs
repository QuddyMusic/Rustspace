//! Доменное состояние голосовой комнаты, независимое от LiveKit.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{VoiceError, VoiceResult};

/// Тип медиа-трека.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackKind {
    Audio,
    Video,
    ScreenShare,
    ScreenShareAudio,
}

/// Состояние соединения комнаты.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RoomState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

/// Id пользователя из identity LiveKit.
///
/// Сервер Backspace выдаёт токены с identity `homeUserId:displayName`
/// (`routes/livekit.ts`), поэтому всё, что клиент хранит по пользователю
/// (громкость, подсветка говорящих), ключуется частью до первого `:`.
#[must_use]
pub fn user_id_of(identity: &str) -> &str {
    identity.split_once(':').map_or(identity, |(id, _)| id)
}

impl RoomState {
    /// Можно ли принимать/отправлять медиа в этом состоянии.
    #[must_use]
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Connected | Self::Reconnecting)
    }
}

/// Сведения об участнике голосового канала.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParticipantInfo {
    /// Идентичность в LiveKit — `userId` из Backspace.
    pub identity: String,
    /// Отображаемое имя.
    pub name: String,
    /// Участник говорит прямо сейчас.
    pub speaking: bool,
    /// Уровень звука 0.0..=1.0.
    pub audio_level: f32,
    /// Микрофон выключен самим участником.
    pub muted: bool,
    /// Участник выключил себе звук (deafen).
    pub deafened: bool,
    /// Локальный участник.
    pub is_local: bool,
}

impl ParticipantInfo {
    /// Создаёт запись локального участника.
    #[must_use]
    pub fn local(identity: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            identity: identity.into(),
            name: name.into(),
            speaking: false,
            audio_level: 0.0,
            muted: false,
            deafened: false,
            is_local: true,
        }
    }

    /// Нормализует уровень звука из LiveKit (0.0..=1.0, с обрезкой).
    pub fn set_audio_level(&mut self, level: f32) {
        self.audio_level = if level.is_finite() {
            level.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
}

/// Снимок комнаты: кто в ней и что происходит.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RoomSnapshot {
    /// Имя комнаты.
    pub room: String,
    /// Состояние соединения.
    pub state: RoomState,
    /// Участники по identity.
    pub participants: BTreeMap<String, ParticipantInfo>,
}

impl RoomSnapshot {
    /// Добавляет или обновляет участника.
    pub fn upsert(&mut self, info: ParticipantInfo) {
        self.participants.insert(info.identity.clone(), info);
    }

    /// Удаляет участника, возвращает запись, если была.
    pub fn remove(&mut self, identity: &str) -> Option<ParticipantInfo> {
        self.participants.remove(identity)
    }

    /// Находит участника по identity.
    #[must_use]
    pub fn get(&self, identity: &str) -> Option<&ParticipantInfo> {
        self.participants.get(identity)
    }

    /// Изменяет участника, если он есть.
    pub fn update<F: FnOnce(&mut ParticipantInfo)>(
        &mut self,
        identity: &str,
        f: F,
    ) -> VoiceResult<()> {
        let p = self
            .participants
            .get_mut(identity)
            .ok_or_else(|| VoiceError::ParticipantNotFound(identity.to_owned()))?;
        f(p);
        Ok(())
    }

    /// Сколько участников в комнате.
    #[must_use]
    pub fn len(&self) -> usize {
        self.participants.len()
    }

    /// Пуста ли комната.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.participants.is_empty()
    }
}

#[cfg(test)]
mod identity_tests {
    use super::user_id_of;

    #[test]
    fn identity_is_user_id_and_name() {
        assert_eq!(user_id_of("u1:Alice"), "u1");
        assert_eq!(user_id_of("u1:Al:ice"), "u1");
        assert_eq!(user_id_of("bare"), "bare");
    }
}
