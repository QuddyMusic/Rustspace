//! Маппинг событий LiveKit в доменные события Backspace.
//!
//! Наружу отдаём только простые `'static`-структуры без ссылок на LiveKit:
//! так события можно сериализовать, складывать в канал и тестировать без
//! поднятия комнаты.

use serde::{Deserialize, Serialize};

use crate::state::TrackKind;

/// Что произошло в голосовой комнате.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum VoiceEvent {
    /// Участник подключился.
    ParticipantJoined { identity: String, name: String },
    /// Участник отключился.
    ParticipantLeft { identity: String },
    /// Изменился список говорящих.
    ActiveSpeakers { identities: Vec<String> },
    /// Участник включил или выключил микрофон.
    MuteChanged { identity: String, muted: bool },
    /// Опубликован удалённый трек.
    TrackSubscribed {
        identity: String,
        sid: String,
        kind: TrackKind,
    },
    /// Удалённый трек снят.
    TrackUnsubscribed {
        identity: String,
        sid: String,
        kind: TrackKind,
    },
    /// Соединение установлено.
    Connected { room: String },
    /// Идёт переподключение.
    Reconnecting,
    /// Переподключение успешно.
    Reconnected,
    /// Комната закрыта.
    Disconnected { reason: String },
}

/// Ключ трека для внутренних карт.
#[must_use]
pub fn track_key(identity: &str, sid: &str) -> String {
    format!("{identity}/{sid}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_serializes_with_type_tag() {
        let ev = VoiceEvent::ParticipantJoined {
            identity: "u1".into(),
            name: "Ann".into(),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains("\"type\":\"participantJoined\""));
        assert!(json.contains("\"identity\":\"u1\""));
    }

    #[test]
    fn roundtrip() {
        let ev = VoiceEvent::ActiveSpeakers {
            identities: vec!["a".into()],
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        let back: VoiceEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ev, back);
    }

    #[test]
    fn track_key_is_stable() {
        assert_eq!(track_key("u1", "TR_1"), "u1/TR_1");
    }
}
