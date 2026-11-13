//! Скачивание вложений, пришедших с других инстансов
//! (`processFileQueueTick` в `utils/federationWorker.ts`).

use std::time::Duration;

use serde_json::json;
use url::Url;

use crate::db::{DbResult, Row, Tx};
use crate::federation::outbox::{self, Event};
use crate::federation::{append_mutation_log, our_origin, relay_enabled};
use crate::hub::Outbox;
use crate::params;
use crate::state::{AppState, now_ms};

/// Как часто смотреть очередь (`FILE_QUEUE_INTERVAL_MS`).
pub const INTERVAL: Duration = Duration::from_secs(30);
const BATCH: i64 = 5;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(1);
const MAX_ATTEMPTS: i64 = 10;
const BACKOFF_MS: [i64; 7] = [
    30_000, 60_000, 300_000, 900_000, 3_600_000, 21_600_000, 86_400_000,
];

fn backoff(app: &AppState, attempt: i64) -> i64 {
    let index = usize::try_from((attempt - 1).max(0))
        .unwrap_or(0)
        .min(BACKOFF_MS.len() - 1);
    let divisor = i64::from(app.config.federation_backoff_divisor.max(1));
    (BACKOFF_MS[index] + divisor - 1) / divisor
}

async fn max_upload(app: &AppState) -> i64 {
    let configured = i64::try_from(app.config.max_upload_size).unwrap_or(i64::MAX);
    app.db
        .transaction(|tx: &mut dyn Tx| {
            tx.query_one(
                "SELECT max_upload_size_bytes FROM instance_settings WHERE id = 1",
                params![],
            )
        })
        .await
        .ok()
        .flatten()
        .and_then(|row| row.opt_int("max_upload_size_bytes").ok().flatten())
        .unwrap_or(configured)
}

async fn set_status(app: &AppState, id: &str, status: &str, reason: &str) {
    let (id, status, reason) = (id.to_owned(), status.to_owned(), reason.to_owned());
    let result = app
        .db
        .transaction(move |tx: &mut dyn Tx| {
            tx.execute(
                "UPDATE federation_file_queue SET status = ?, rejection_reason = ? WHERE id = ?",
                params![status, reason, id],
            )
        })
        .await;
    if let Err(error) = result {
        tracing::error!(%error, "не удалось обновить очередь файлов");
    }
}

fn message_updated(tx: &mut dyn Tx, out: &mut Outbox, message: &str) -> DbResult<()> {
    if let Some(row) = crate::dm::message_row(tx, message)?
        && let Some(full) = crate::dm::message_json(tx, &row)?
    {
        let channel = row.text("dm_channel_id")?;
        crate::broadcast::to_dm_members(
            tx,
            out,
            &channel,
            &json!({"type": "dm_message_updated", "message": full}),
            None,
        )?;
    }
    Ok(())
}

/// Файл больше предела: отправителю уходит `file_rejected`
/// (`handleSizeRejection`).
fn size_rejection(
    tx: &mut dyn Tx,
    app: &AppState,
    out: &mut Outbox,
    entry: &Row,
    limit: i64,
) -> DbResult<()> {
    let message_id = entry.text("dm_message_id")?;
    let source_url = entry.text("source_url")?;
    let Some(message) = crate::dm::message_row(tx, &message_id)? else {
        return Ok(());
    };
    let (Some(source), Some(source_id)) = (
        message
            .opt_text("source_instance")?
            .filter(|s| !s.is_empty()),
        message
            .opt_text("source_message_id")?
            .filter(|s| !s.is_empty()),
    ) else {
        return Ok(());
    };
    let attachment = tx.query_one(
        "SELECT id FROM attachments WHERE dm_message_id = ? AND source_url = ?",
        params![&message_id, &source_url],
    )?;
    let sender = message.text("user_id")?;
    let sender_name = match tx.query_one(
        "SELECT display_name, username FROM users WHERE id = ?",
        params![&sender],
    )? {
        Some(row) => row
            .opt_text("display_name")?
            .filter(|n| !n.is_empty())
            .or(Some(row.text("username")?).filter(|n| !n.is_empty()))
            .unwrap_or_else(|| "unknown".to_owned()),
        None => "unknown".to_owned(),
    };
    let attachment_id = match &attachment {
        Some(row) => {
            let id = row.text("id")?;
            let meta = crate::dm::system_content(&[
                ("sourceInstance", json!(source)),
                ("sourceUserId", json!(sender)),
                ("sourceUsername", json!(sender_name)),
            ]);
            tx.execute(
                "UPDATE attachments SET federation_status = 'remote', federation_meta = ? WHERE id = ?",
                params![meta, &id],
            )?;
            id
        }
        None => source_url.clone(),
    };
    let ours = our_origin(&app.config);
    let channel = message.text("dm_channel_id")?;
    let mut ids = Vec::new();
    let mut users = Vec::new();
    for member in tx.query(
        "SELECT u.id, u.home_user_id, u.home_instance FROM dm_members m JOIN users u ON u.id = m.user_id \
         WHERE m.dm_channel_id = ?",
        params![&channel],
    )? {
        let home = member.opt_text("home_instance")?.filter(|h| !h.is_empty());
        let local = match &home {
            None => true,
            Some(home) => {
                let full = if home.starts_with("http") {
                    home.clone()
                } else {
                    format!("https://{home}")
                };
                full == ours
            }
        };
        if !local {
            continue;
        }
        let home_id = member
            .opt_text("home_user_id")?
            .filter(|id| !id.is_empty())
            .map_or_else(|| member.text("id"), Ok)?;
        ids.push(json!(home_id));
        users.push(json!({"homeUserId": home_id, "homeInstance": ours}));
    }
    let filename = source_url
        .rsplit('/')
        .next()
        .unwrap_or(&source_url)
        .to_owned();
    let now = now_ms();
    let log = json!({
        "attachmentId": attachment_id,
        "sourceFilename": filename,
        "rejectionReason": "size_limit_exceeded",
        "rejectionLimit": limit,
        "affectedUserIds": ids,
        "affectedUsers": users,
    });
    let event = json!({
        "eventType": "file_rejected",
        "messageId": source_id,
        "encryptionVersion": 0,
        "timestamp": now,
        "attachmentId": attachment_id,
        "sourceFilename": filename,
        "rejectionReason": "size_limit_exceeded",
        "rejectionLimit": limit,
        "affectedUserIds": ids,
        "affectedUsers": users,
    });
    append_mutation_log(
        tx,
        &app.ids,
        &source_id,
        &channel,
        "file_rejected",
        Some(&log),
    )?;
    let targets = [source];
    outbox::queue(
        tx,
        &app.ids,
        &Event {
            entity_id: &source_id,
            context_id: &channel,
            event_type: "file_rejected",
            payload: &event,
            targets: Some(&targets),
            context_type: "dm",
        },
    )?;
    message_updated(tx, out, &message_id)
}

async fn reject_for_size(app: &AppState, entry: Row, limit: i64) {
    let app_tx = app.clone();
    let result = app
        .run(move |tx, out| {
            tx.execute(
                "UPDATE federation_file_queue SET status = 'rejected', rejection_reason = 'size_limit_exceeded' WHERE id = ?",
                params![entry.text("id")?],
            )?;
            size_rejection(tx, &app_tx, out, &entry, limit)?;
            Ok(())
        })
        .await;
    if let Err(error) = result {
        tracing::error!(?error, "не удалось отклонить файл по размеру");
    }
}

async fn download(app: &AppState, entry: &Row) -> Result<(String, i64, Option<String>), String> {
    let url = entry.text("source_url").map_err(|e| e.to_string())?;
    let fetched = super::profile::safe_fetch(app, &url, DOWNLOAD_TIMEOUT).await?;
    if !(200..300).contains(&fetched.status) {
        return Err(format!(
            "HTTP {} downloading file from {url}",
            fetched.status
        ));
    }
    let original = entry
        .opt_text("original_name")
        .ok()
        .flatten()
        .unwrap_or_default();
    let ext = std::path::Path::new(&original)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{ext}"))
        .unwrap_or_default();
    let name = format!("{}{ext}", app.ids.next());
    let path = crate::files::upload_path(&app.config, &name).ok_or("bad path")?;
    let limit = usize::try_from(max_upload(app).await.max(0)).unwrap_or(usize::MAX);
    let (body, truncated) =
        crate::net::read_limited(fetched.response, limit.saturating_add(1), DOWNLOAD_TIMEOUT).await;
    let _ = tokio::fs::create_dir_all(&app.config.upload_dir).await;
    tokio::fs::write(&path, &body)
        .await
        .map_err(|e| e.to_string())?;
    let size = if truncated {
        i64::MAX
    } else {
        i64::try_from(body.len()).unwrap_or(i64::MAX)
    };
    let mimetype = entry
        .opt_text("mimetype")
        .ok()
        .flatten()
        .unwrap_or_default();
    let thumb_name = crate::files::thumb_filename(&name);
    let done = crate::media::process(&path, &mimetype, &app.config.upload_dir, &thumb_name).await;
    Ok((name, size, done.thumbnail))
}

async fn process_entry(app: &AppState, entry: Row, limit: i64, now: i64) {
    let Ok(id) = entry.text("id") else {
        return;
    };
    let source = entry.text("source_url").unwrap_or_default();
    let peer = entry.text("peer_origin").unwrap_or_default();
    match (Url::parse(&source), Url::parse(&peer)) {
        (Ok(source), Ok(peer)) => {
            if source.host_str() != peer.host_str() {
                tracing::warn!(entry = id, "адрес вложения не на узле пира");
                set_status(app, &id, "rejected", "ssrf_hostname_mismatch").await;
                return;
            }
        }
        _ => {
            set_status(app, &id, "rejected", "invalid_url").await;
            return;
        }
    }
    if entry.opt_int("size").ok().flatten().unwrap_or(0) > limit {
        reject_for_size(app, entry, limit).await;
        return;
    }
    match download(app, &entry).await {
        Ok((name, size, thumbnail)) => {
            if size > limit {
                crate::files::delete_upload_file(&app.config, &name).await;
                reject_for_size(app, entry, limit).await;
                return;
            }
            let app_tx = app.clone();
            let result = app
                .run(move |tx, out| {
                    let message = entry.text("dm_message_id")?;
                    let source_url = entry.text("source_url")?;
                    let changed = tx.execute(
                        "UPDATE attachments SET filename = ?, size = ?, thumbnail_filename = ? \
                         WHERE dm_message_id = ? AND source_url = ?",
                        params![&name, size, &thumbnail, &message, &source_url],
                    )?;
                    if changed == 0 {
                        tx.execute(
                            "INSERT INTO attachments (id, dm_message_id, uploader_id, filename, original_name, \
                             mimetype, size, thumbnail_filename, source_url, created_at) \
                             VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?, ?)",
                            params![
                                app_tx.ids.next(),
                                &message,
                                &name,
                                entry.opt_text("original_name")?,
                                entry.opt_text("mimetype")?,
                                size,
                                &thumbnail,
                                &source_url,
                                now
                            ],
                        )?;
                    }
                    tx.execute(
                        "UPDATE federation_file_queue SET status = 'completed', target_filename = ? WHERE id = ?",
                        params![&name, entry.text("id")?],
                    )?;
                    message_updated(tx, out, &message)?;
                    Ok(())
                })
                .await;
            if let Err(error) = result {
                tracing::error!(?error, "не удалось записать скачанное вложение");
            }
        }
        Err(error) => {
            tracing::warn!(entry = id, error, "вложение с другого инстанса не скачано");
            let attempts = entry.opt_int("attempts").ok().flatten().unwrap_or(0) + 1;
            let wait = backoff(app, attempts);
            let result = app
                .db
                .transaction(move |tx: &mut dyn Tx| {
                    if attempts > MAX_ATTEMPTS {
                        tx.execute(
                            "UPDATE federation_file_queue SET status = 'failed', attempts = ?, \
                             rejection_reason = 'max_attempts_exceeded' WHERE id = ?",
                            params![attempts, &id],
                        )
                    } else {
                        tx.execute(
                            "UPDATE federation_file_queue SET attempts = ?, next_retry_at = ? WHERE id = ?",
                            params![attempts, now + wait, &id],
                        )
                    }
                })
                .await;
            if let Err(error) = result {
                tracing::error!(%error, "не удалось обновить очередь файлов");
            }
        }
    }
}

/// Один проход очереди (`processFileQueueTick`).
pub async fn tick(app: &AppState) {
    let now = now_ms();
    let pending = app
        .db
        .transaction(move |tx: &mut dyn Tx| {
            if !relay_enabled(tx)? {
                return Ok(Vec::new());
            }
            tx.query(
                "SELECT * FROM federation_file_queue WHERE status = 'pending' AND next_retry_at <= ? LIMIT ?",
                params![now, BATCH],
            )
        })
        .await;
    let pending = match pending {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "очередь файлов не прочитана");
            return;
        }
    };
    if pending.is_empty() {
        return;
    }
    let limit = max_upload(app).await;
    for entry in pending {
        process_entry(app, entry, limit, now).await;
    }
}
