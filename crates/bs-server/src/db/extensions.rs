//! Таблицы расширений этого сервера, которых нет в схеме оригинала:
//! двухфакторная аутентификация, ключи сквозного шифрования, мосты, вебхуки, битрейт голоса.
//!
//! Они лежат в той же базе под именами `bs_*`. Оригинальный сервер их не
//! замечает, так что файл базы по-прежнему открывается обоими. Описания
//! написаны на общем для SQLite и PostgreSQL подмножестве SQL.

use super::{DbResult, Tx};
use crate::params;

/// Расширения по порядку: имя и операторы.
const EXTENSIONS: &[(&str, &[&str])] = &[
    (
        "0001_two_factor",
        &[
            "CREATE TABLE bs_totp (
                user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
                secret TEXT NOT NULL,
                enabled_at BIGINT,
                last_step BIGINT NOT NULL DEFAULT 0,
                created_at BIGINT NOT NULL
            )",
            "CREATE TABLE bs_totp_recovery (
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                code_hash TEXT NOT NULL,
                used_at BIGINT,
                PRIMARY KEY (user_id, code_hash)
            )",
        ],
    ),
    (
        "0002_e2ee_keys",
        &["CREATE TABLE bs_e2ee_keys (
                user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
                public_key TEXT NOT NULL,
                updated_at BIGINT NOT NULL
            )"],
    ),
    (
        "0003_bridges",
        &[
            "CREATE TABLE bs_bridge_links (
                id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
                kind TEXT NOT NULL,
                external_id TEXT NOT NULL,
                created_by TEXT,
                created_at BIGINT NOT NULL,
                UNIQUE (channel_id, kind, external_id)
            )",
            "CREATE INDEX bs_bridge_links_external ON bs_bridge_links (kind, external_id)",
            "CREATE TABLE bs_bridge_puppets (
                user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
                kind TEXT NOT NULL,
                external_id TEXT NOT NULL,
                UNIQUE (kind, external_id)
            )",
        ],
    ),
    (
        "0004_webhooks",
        &[
            "CREATE TABLE bs_webhooks (
                id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                avatar TEXT,
                token TEXT NOT NULL,
                created_by TEXT,
                created_at BIGINT NOT NULL
            )",
            "CREATE INDEX bs_webhooks_channel ON bs_webhooks (channel_id)",
        ],
    ),
    (
        "0005_voice_bitrate",
        &[
            "CREATE TABLE bs_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
            "CREATE TABLE bs_voice_bitrate (
                target_id TEXT PRIMARY KEY,
                kbps INTEGER NOT NULL,
                set_by TEXT,
                updated_at BIGINT NOT NULL
            )",
        ],
    ),
    (
        "0006_channel_flags",
        &["CREATE TABLE bs_channel_flags (
                channel_id TEXT PRIMARY KEY REFERENCES channels(id) ON DELETE CASCADE,
                nsfw INTEGER NOT NULL DEFAULT 0
            )"],
    ),
    (
        "0007_saved_messages",
        &["CREATE TABLE bs_saved_messages (
                user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
                dm_channel_id TEXT NOT NULL REFERENCES dm_channels(id) ON DELETE CASCADE
            )"],
    ),
    (
        "0008_pins",
        &[
            // user_id = '' — общий закреп, иначе личный (виден только ему).
            "CREATE TABLE bs_pins (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                channel_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                pinned_by TEXT NOT NULL,
                created_at BIGINT NOT NULL,
                UNIQUE (kind, channel_id, message_id, user_id)
            )",
            "CREATE INDEX bs_pins_channel ON bs_pins (kind, channel_id)",
        ],
    ),
    (
        "0009_bookmarks",
        &[
            // Как message_bookmarks из PR оригинала #474, но со своим именем:
            // если оригинал примет закладки, его миграция не столкнётся с нашей.
            "CREATE TABLE bs_bookmarks (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                kind TEXT NOT NULL,
                message_id TEXT NOT NULL,
                channel_id TEXT NOT NULL,
                space_id TEXT,
                content TEXT,
                author_id TEXT NOT NULL,
                author_name TEXT NOT NULL,
                space_name TEXT,
                channel_name TEXT,
                instance_origin TEXT NOT NULL,
                message_created_at BIGINT NOT NULL,
                created_at BIGINT NOT NULL,
                UNIQUE (user_id, kind, message_id)
            )",
            "CREATE INDEX bs_bookmarks_user_created ON bs_bookmarks (user_id, created_at, id)",
            "CREATE INDEX bs_bookmarks_user_channel ON bs_bookmarks (user_id, kind, channel_id)",
            "CREATE TABLE bs_bookmark_attachments (
                id TEXT PRIMARY KEY,
                bookmark_id TEXT NOT NULL REFERENCES bs_bookmarks(id) ON DELETE CASCADE,
                position INTEGER NOT NULL,
                original_name TEXT NOT NULL,
                mimetype TEXT NOT NULL,
                size BIGINT NOT NULL,
                width INTEGER,
                height INTEGER,
                duration DOUBLE PRECISION,
                playable INTEGER,
                source_filename TEXT NOT NULL,
                source_thumbnail_filename TEXT
            )",
            "CREATE INDEX bs_bookmark_attachments_bookmark ON bs_bookmark_attachments (bookmark_id)",
        ],
    ),
    (
        "0010_game_icons",
        &[
            // Как game_icon_assets и game_icons из PR оригинала #482
            // (QuddyMusic, cirzear), под своим именем — по той же причине.
            "CREATE TABLE bs_game_icon_assets (
                id TEXT PRIMARY KEY,
                created_at BIGINT NOT NULL
            )",
            "CREATE TABLE bs_game_icons (
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                icon_id TEXT NOT NULL REFERENCES bs_game_icon_assets(id),
                created_at BIGINT NOT NULL,
                PRIMARY KEY (user_id, icon_id)
            )",
        ],
    ),
    (
        "0011_instance_name",
        &[
            // Инстанс, перешедший с оригинала, назывался «Backspace» по
            // умолчанию схемы. Один раз переименовываем в «Rustspace»; своё
            // имя администратора не трогаем, сменить обратно можно в панели.
            "UPDATE instance_settings SET instance_name = 'Rustspace' \
             WHERE id = 1 AND (instance_name IS NULL OR instance_name = '' OR instance_name = 'Backspace')",
        ],
    ),
    (
        "0012_sticker_packs",
        &[
            // Стикерпаки: картинки — те же sticker_assets, что у личных
            // стикеров, поэтому в сообщениях это обычные стикеры.
            "CREATE TABLE bs_sticker_packs (
                id TEXT PRIMARY KEY,
                owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                shared INTEGER NOT NULL DEFAULT 1,
                created_at BIGINT NOT NULL,
                updated_at BIGINT NOT NULL
            )",
            "CREATE INDEX bs_sticker_packs_owner ON bs_sticker_packs (owner_id)",
            "CREATE TABLE bs_sticker_pack_items (
                pack_id TEXT NOT NULL REFERENCES bs_sticker_packs(id) ON DELETE CASCADE,
                sticker_id TEXT NOT NULL REFERENCES sticker_assets(id),
                name TEXT NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY (pack_id, sticker_id)
            )",
            "CREATE INDEX bs_sticker_pack_items_sticker ON bs_sticker_pack_items (sticker_id)",
            "CREATE TABLE bs_sticker_pack_users (
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                pack_id TEXT NOT NULL REFERENCES bs_sticker_packs(id) ON DELETE CASCADE,
                added_at BIGINT NOT NULL,
                PRIMARY KEY (user_id, pack_id)
            )",
        ],
    ),
    (
        "0013_personal_bridges",
        &[
            // Свои аккаунты пользователей в других сетях; секрет зашифрован.
            "CREATE TABLE bs_personal_accounts (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                kind TEXT NOT NULL,
                remote_id TEXT NOT NULL,
                display TEXT NOT NULL,
                secret TEXT NOT NULL,
                state TEXT NOT NULL,
                error TEXT,
                created_at BIGINT NOT NULL
            )",
            "CREATE INDEX bs_personal_accounts_user ON bs_personal_accounts (user_id)",
            // Связанные чаты: чат на той стороне ↔ личная переписка с марионеткой.
            "CREATE TABLE bs_personal_chats (
                id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL REFERENCES bs_personal_accounts(id) ON DELETE CASCADE,
                remote_chat TEXT NOT NULL,
                title TEXT NOT NULL,
                grouped INTEGER NOT NULL DEFAULT 0,
                dm_channel_id TEXT NOT NULL,
                puppet_id TEXT NOT NULL,
                created_at BIGINT NOT NULL,
                UNIQUE (account_id, remote_chat)
            )",
            "CREATE INDEX bs_personal_chats_dm ON bs_personal_chats (dm_channel_id)",
        ],
    ),
    (
        "0014_bots",
        &[
            // Боты: пользователь-бот, его владелец и хеш секрета токена.
            "CREATE TABLE bs_bots (
                user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
                owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                token_hash TEXT NOT NULL,
                description TEXT,
                created_at BIGINT NOT NULL
            )",
            "CREATE INDEX bs_bots_owner ON bs_bots (owner_id)",
        ],
    ),
    (
        "0015_user_privacy",
        &[
            // Приватность: при пересылке — без ссылки на профиль автора.
            "CREATE TABLE bs_user_privacy (
                user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
                hide_forward_author INTEGER NOT NULL DEFAULT 0
            )",
        ],
    ),
];

/// Создаёт недостающие таблицы расширений.
///
/// # Errors
/// Ошибка базы.
pub fn apply(tx: &mut dyn Tx) -> DbResult<()> {
    tx.execute("CREATE TABLE IF NOT EXISTS bs_migrations (name TEXT PRIMARY KEY, applied_at BIGINT NOT NULL)", params![])?;
    for (name, statements) in EXTENSIONS {
        if tx
            .query_one(
                "SELECT name FROM bs_migrations WHERE name = ?",
                params![*name],
            )?
            .is_some()
        {
            continue;
        }
        for statement in *statements {
            tx.execute(statement, params![])?;
        }
        tx.execute(
            "INSERT INTO bs_migrations (name, applied_at) VALUES (?, ?)",
            params![*name, crate::state::now_ms()],
        )?;
        tracing::info!(extension = name, "таблицы расширения созданы");
    }
    Ok(())
}
