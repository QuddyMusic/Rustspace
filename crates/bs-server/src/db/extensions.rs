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
