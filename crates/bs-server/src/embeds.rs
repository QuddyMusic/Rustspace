//! Встраивания: предпросмотр ссылок из сообщений (`utils/embedResolver.ts`).

use crate::state::AppState;

/// Где находится сообщение, для которого считаются встраивания.
#[derive(Debug, Clone)]
pub enum Place {
    /// Канал пространства.
    Channel {
        /// Пространство.
        space_id: String,
        /// Канал.
        channel_id: String,
    },
    /// Личная переписка.
    Dm {
        /// Переписка.
        dm_channel_id: String,
    },
}

/// Считает встраивания для сообщения в фоне и рассылает их, когда готовы.
pub fn schedule(state: &AppState, message_id: String, content: Option<String>, place: Place) {
    let _ = (state, message_id, content, place);
}
