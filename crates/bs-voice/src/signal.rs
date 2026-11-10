//! Сообщения канала данных LiveKit (как `streamWatchProtocol.ts` оригинала).
//!
//! - `stream_watch` — зритель начал или перестал смотреть трансляцию;
//! - `stream_republish` / `stream_resume` / `stream_stop` — ведущий
//!   сообщает о своей трансляции;
//! - `deafen` — участник выключил или включил звук.
//!
//! Всё прочее (сообщения новых версий) молча игнорируется, как и в оригинале.

use serde::{Deserialize, Serialize};

/// Сообщение канала данных.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DataMessage {
    /// Зритель смотрит (или перестал смотреть) трансляцию.
    #[serde(rename = "stream_watch")]
    StreamWatch {
        /// Id ведущего, как его знает клиент зрителя.
        target: String,
        /// Identity ведущего в LiveKit (у старых клиентов нет).
        #[serde(
            rename = "targetIdentity",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        target_identity: Option<String>,
        /// Смотрит ли.
        watching: bool,
    },
    /// Трансляция сейчас будет перевыпущена (смена кодека), это не конец.
    #[serde(rename = "stream_republish")]
    StreamRepublish,
    /// Трансляция снова опубликована после полного переподключения.
    #[serde(rename = "stream_resume")]
    StreamResume,
    /// Трансляция закончилась насовсем.
    #[serde(rename = "stream_stop")]
    StreamStop,
    /// Участник выключил или включил звук.
    #[serde(rename = "deafen")]
    Deafen {
        /// Звук выключен.
        deafened: bool,
    },
}

impl DataMessage {
    /// JSON-байты для `publish_data`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Разбирает полученные байты; чужие сообщения — `None`.
    #[must_use]
    pub fn decode(payload: &[u8]) -> Option<Self> {
        serde_json::from_slice(payload).ok()
    }

    /// Сообщение зрителя о трансляции `sharer`.
    #[must_use]
    pub fn watch(sharer_user_id: &str, sharer_identity: &str, watching: bool) -> Self {
        Self::StreamWatch {
            target: sharer_user_id.to_owned(),
            target_identity: Some(sharer_identity.to_owned()),
            watching,
        }
    }
}

/// Чью трансляцию имеет в виду `stream_watch`: identity ведущего.
///
/// У старых зрителей есть только `target` (id) — ищем участника с таким id;
/// не нашли — сообщение не к чему привязать (`None`), как `streamWatchKey`.
#[must_use]
pub fn watch_key<'a>(
    target: &str,
    target_identity: Option<&'a str>,
    identities: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    if let Some(identity) = target_identity {
        return Some(identity);
    }
    identities
        .into_iter()
        .find(|identity| crate::state::user_id_of(identity) == target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_matches_original_wire_format() {
        let message = DataMessage::watch("u1", "u1:Alice", true);
        let json: serde_json::Value = serde_json::from_slice(&message.encode()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "stream_watch",
                "target": "u1",
                "targetIdentity": "u1:Alice",
                "watching": true
            })
        );
    }

    #[test]
    fn decodes_signals_and_deafen() {
        assert_eq!(
            DataMessage::decode(br#"{"type":"stream_stop"}"#),
            Some(DataMessage::StreamStop)
        );
        assert_eq!(
            DataMessage::decode(br#"{"type":"deafen","deafened":true}"#),
            Some(DataMessage::Deafen { deafened: true })
        );
        assert_eq!(
            DataMessage::decode(br#"{"type":"stream_watch","target":"u2","watching":false}"#),
            Some(DataMessage::StreamWatch {
                target: "u2".into(),
                target_identity: None,
                watching: false
            })
        );
    }

    #[test]
    fn unknown_messages_are_ignored() {
        assert_eq!(DataMessage::decode(br#"{"type":"future_thing"}"#), None);
        assert_eq!(DataMessage::decode(b"not json"), None);
    }

    #[test]
    fn old_viewer_is_resolved_by_user_id() {
        let identities = ["u1:Alice", "u2:Bob"];
        assert_eq!(watch_key("u2", None, identities), Some("u2:Bob"));
        assert_eq!(watch_key("u9", None, identities), None);
        assert_eq!(
            watch_key("x", Some("u1:Alice"), identities),
            Some("u1:Alice")
        );
    }
}
