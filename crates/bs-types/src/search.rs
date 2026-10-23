//! Ответы поиска сообщений (`GET /api/channels/:id/search` и `/api/dm/:id/search`).

use serde::{Deserialize, Serialize};

use crate::message::{DmMessageWithUser, MessageWithUser};

/// Ответ поиска по каналу пространства.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageSearchResponse {
    pub results: Vec<MessageWithUser>,
    pub total_count: i64,
}

/// Ответ поиска по DM-каналу.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmSearchResponse {
    pub results: Vec<DmMessageWithUser>,
    pub total_count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_message_search_round_trips() {
        let json = serde_json::json!({ "results": [], "totalCount": 0 });
        let page: MessageSearchResponse = serde_json::from_value(json).unwrap();
        assert!(page.results.is_empty());
        assert_eq!(page.total_count, 0);
    }

    #[test]
    fn empty_dm_search_round_trips() {
        let json = serde_json::json!({ "results": [], "totalCount": 7 });
        let page: DmSearchResponse = serde_json::from_value(json).unwrap();
        assert_eq!(page.total_count, 7);
    }
}
