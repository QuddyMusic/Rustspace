//! Учёт и уборка хранилища загрузок (`utils/storageJanitor.ts`,
//! `utils/stickerAssets.ts`).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde_json::{Value, json};

use crate::config::Config;
use crate::db::{DbResult, Tx};
use crate::files::{basename, thumb_filename};
use crate::state::now_ms;
use crate::{embeds, params};

/// Загруженное, но ни к чему не прикреплённое вложение убирается через час.
const UNLINKED_AGE_MS: i64 = 3_600_000;
/// Картинка стикера, на которую никто не ссылается, живёт сутки.
const STICKER_GRACE_MS: i64 = 86_400_000;
const STICKER_TEMP_AGE_MS: i64 = 3_600_000;

/// Итог уборки (`CleanupResult`).
#[derive(Debug, Default)]
pub struct Cleanup {
    /// Удалено файлов.
    pub deleted_files: u64,
    /// Освобождено байт.
    pub freed_bytes: u64,
    /// Удалено записей о вложениях.
    pub deleted_records: u64,
    /// Что не получилось.
    pub errors: Vec<String>,
}

impl Cleanup {
    /// Ответ администратору.
    #[must_use]
    pub fn to_json(&self, dry_run: bool) -> Value {
        json!({
            "dryRun": dry_run,
            "deletedFiles": self.deleted_files,
            "freedBytes": self.freed_bytes,
            "deletedAttachmentRecords": self.deleted_records,
            "errors": self.errors,
        })
    }
}

struct DiskFile {
    name: String,
    size: u64,
    modified_ms: f64,
}

fn disk_files(dir: &Path) -> Vec<DiskFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok().filter(std::fs::Metadata::is_file)?;
            let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
            Some(DiskFile {
                name: entry.file_name().to_string_lossy().into_owned(),
                size: meta.len(),
                modified_ms: modified.as_secs_f64() * 1000.0,
            })
        })
        .collect()
}

fn classify(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" | "ico" | "bmp" | "avif" => "image",
        "mp4" | "webm" | "mov" | "avi" | "mkv" => "video",
        "mp3" | "ogg" | "wav" | "flac" | "aac" | "m4a" | "opus" => "audio",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "csv" | "json"
        | "xml" => "document",
        _ => "other",
    }
}

/// Файлы, занятые аватарами, обложками и значками.
fn profile_files(tx: &mut dyn Tx) -> DbResult<HashSet<String>> {
    let mut used = HashSet::new();
    for (table, column, extra) in [
        ("users", "avatar", ""),
        ("users", "banner", ""),
        ("spaces", "icon", ""),
        ("spaces", "banner", ""),
        ("dm_channels", "icon", " AND deleted_at IS NULL"),
    ] {
        for row in tx.query(
            &format!("SELECT {column} AS file FROM {table} WHERE {column} IS NOT NULL{extra}"),
            params![],
        )? {
            let file = row.text("file")?;
            let external = table == "dm_channels" && file.starts_with("http");
            if !file.is_empty() && !external {
                used.insert(basename(&file).to_owned());
            }
        }
    }
    Ok(used)
}

fn referenced_files(tx: &mut dyn Tx) -> DbResult<HashSet<String>> {
    let mut used = profile_files(tx)?;
    for row in tx.query(
        "SELECT filename, thumbnail_filename FROM attachments",
        params![],
    )? {
        used.insert(basename(&row.text("filename")?).to_owned());
        if let Some(thumb) = row
            .opt_text("thumbnail_filename")?
            .filter(|thumb| !thumb.is_empty())
        {
            used.insert(basename(&thumb).to_owned());
        }
    }
    Ok(used)
}

struct Attachment {
    id: String,
    filename: String,
    size: u64,
}

fn attachments(
    tx: &mut dyn Tx,
    sql: &str,
    values: &[crate::db::Value],
) -> DbResult<Vec<Attachment>> {
    tx.query(sql, values)?
        .iter()
        .map(|row| {
            Ok(Attachment {
                id: row.text("id")?,
                filename: row.text("filename")?,
                size: u64::try_from(row.int("size")?).unwrap_or(0),
            })
        })
        .collect()
}

fn unlinked(tx: &mut dyn Tx, profile: &HashSet<String>) -> DbResult<Vec<Attachment>> {
    let found = attachments(
        tx,
        "SELECT id, filename, thumbnail_filename, size FROM attachments \
         WHERE message_id IS NULL AND dm_message_id IS NULL AND created_at < ?",
        params![now_ms() - UNLINKED_AGE_MS],
    )?;
    Ok(found
        .into_iter()
        .filter(|att| !profile.contains(basename(&att.filename)))
        .collect())
}

/// Вложения, чьё сообщение уже удалено.
fn dangling(tx: &mut dyn Tx) -> DbResult<Vec<Attachment>> {
    let mut found = attachments(
        tx,
        "SELECT a.id, a.filename, a.thumbnail_filename, a.size FROM attachments a \
         LEFT JOIN messages m ON a.message_id = m.id WHERE a.message_id IS NOT NULL AND m.id IS NULL",
        params![],
    )?;
    found.extend(attachments(
        tx,
        "SELECT a.id, a.filename, a.thumbnail_filename, a.size FROM attachments a \
         LEFT JOIN dm_messages dm ON a.dm_message_id = dm.id WHERE a.dm_message_id IS NOT NULL AND dm.id IS NULL",
        params![],
    )?);
    Ok(found)
}

/// Незавершённые загрузки старше `threshold_ms`: число, размер, пути.
fn stale_uploads(config: &Config, threshold_ms: f64) -> Vec<(std::path::PathBuf, String, u64)> {
    let dir = config.upload_dir.join(".tus");
    #[allow(
        clippy::cast_precision_loss,
        reason = "миллисекунды помещаются в f64 без потерь"
    )]
    let cutoff = now_ms() as f64 - threshold_ms;
    disk_files(&dir)
        .into_iter()
        .filter(|file| file.modified_ms < cutoff)
        .map(|file| (dir.join(&file.name), file.name, file.size))
        .collect()
}

/// Сводка по хранилищу (`getStorageStats`).
///
/// # Errors
/// Ошибка базы.
pub fn stats(tx: &mut dyn Tx, config: &Config) -> DbResult<Value> {
    let files = disk_files(&config.upload_dir);
    let referenced = referenced_files(tx)?;
    let profile = profile_files(tx)?;
    let unlinked = unlinked(tx, &profile)?;
    let dangling = dangling(tx)?;
    let stale = stale_uploads(config, 3_600_000.0);
    let dangling_names: HashSet<&str> =
        dangling.iter().map(|att| basename(&att.filename)).collect();

    let (mut total, mut referenced_size, mut orphaned, mut orphaned_size, mut dangling_on_disk) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut kinds: HashMap<&'static str, (u64, u64)> = HashMap::new();
    let mut order: Vec<&'static str> = Vec::new();
    for file in &files {
        total += file.size;
        let kind = classify(&file.name);
        if !kinds.contains_key(kind) {
            order.push(kind);
        }
        let entry = kinds.entry(kind).or_default();
        entry.0 += 1;
        entry.1 += file.size;
        if dangling_names.contains(file.name.as_str()) {
            dangling_on_disk += 1;
        } else if referenced.contains(&file.name) {
            referenced_size += file.size;
        } else {
            orphaned += 1;
            orphaned_size += file.size;
        }
    }
    order.sort_by_key(|kind| std::cmp::Reverse(kinds[kind].1));
    let breakdown: Vec<Value> = order
        .iter()
        .map(|kind| json!({"type": kind, "count": kinds[kind].0, "size": kinds[kind].1}))
        .collect();
    Ok(json!({
        "totalFiles": files.len(),
        "totalSize": total,
        "referencedFiles": files.len() as u64 - orphaned - dangling_on_disk,
        "referencedSize": referenced_size,
        "orphanedFiles": orphaned,
        "orphanedSize": orphaned_size,
        "unlinkedAttachments": unlinked.len(),
        "unlinkedSize": unlinked.iter().map(|att| att.size).sum::<u64>(),
        "danglingAttachments": dangling.len(),
        "danglingSize": dangling.iter().map(|att| att.size).sum::<u64>(),
        "staleTusSessions": stale.len(),
        "staleTusSize": stale.iter().map(|(_, _, size)| size).sum::<u64>(),
        "breakdown": breakdown,
    }))
}

fn orphan_files(tx: &mut dyn Tx, config: &Config) -> DbResult<Vec<DiskFile>> {
    let referenced = referenced_files(tx)?;
    let mut orphans: Vec<DiskFile> = disk_files(&config.upload_dir)
        .into_iter()
        .filter(|file| !referenced.contains(&file.name))
        .collect();
    orphans.sort_by_key(|file| std::cmp::Reverse(file.size));
    Ok(orphans)
}

/// Файлы, на которые не ссылается ни одна запись (`getOrphanedFiles`).
///
/// # Errors
/// Ошибка базы.
pub fn orphans(tx: &mut dyn Tx, config: &Config) -> DbResult<Vec<Value>> {
    Ok(orphan_files(tx, config)?
        .iter()
        .map(|file| json!({"filename": file.name, "size": file.size, "modifiedAt": file.modified_ms}))
        .collect())
}

/// Удаляет файл загрузки и его миниатюру; отсутствие файла — не ошибка.
fn remove_upload(config: &Config, name: &str) -> Result<(), String> {
    let safe = basename(name);
    let _ = std::fs::remove_file(config.upload_dir.join(thumb_filename(safe)));
    match std::fs::remove_file(config.upload_dir.join(safe)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
        _ => Ok(()),
    }
}

/// Идентификатор картинки из текста сообщения-стикера или реакции.
fn sticker_id(value: &str) -> Option<String> {
    let url = embeds::sticker_url(value)?;
    url.rsplit('/')
        .next()?
        .strip_suffix(".webp")
        .map(str::to_owned)
}

/// Убирает картинки стикеров, на которые больше никто не ссылается
/// (`cleanupUnusedStickerAssets`).
fn cleanup_stickers(
    tx: &mut dyn Tx,
    config: &Config,
    dry_run: bool,
    result: &mut Cleanup,
) -> DbResult<()> {
    let mut used: HashSet<String> = HashSet::new();
    for row in tx.query("SELECT sticker_id FROM personal_stickers", params![])? {
        used.insert(row.text("sticker_id")?);
    }
    for (table, column) in [
        ("messages", "content"),
        ("dm_messages", "content"),
        ("reactions", "emoji"),
        ("dm_reactions", "emoji"),
    ] {
        for row in tx.query(
            &format!("SELECT {column} AS value FROM {table} WHERE {column} LIKE 'sticker:%'"),
            params![],
        )? {
            if let Some(id) = row.opt_text("value")?.as_deref().and_then(sticker_id) {
                used.insert(id);
            }
        }
    }
    let now = now_ms();
    let dir = config.upload_dir.join("stickers");
    for row in tx.query(
        "SELECT id FROM sticker_assets WHERE created_at < ?",
        params![now - STICKER_GRACE_MS],
    )? {
        let id = row.text("id")?;
        if used.contains(&id) || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let path = dir.join(format!("{id}.webp"));
        let size = std::fs::metadata(&path).map_or(0, |meta| meta.len());
        if !dry_run {
            if let Err(error) = std::fs::remove_file(&path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                result
                    .errors
                    .push(format!("Failed to delete sticker {id}: {error}"));
                continue;
            }
            tx.execute("DELETE FROM sticker_assets WHERE id = ?", params![&id])?;
        }
        result.deleted_files += u64::from(size > 0);
        result.freed_bytes += size;
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "миллисекунды помещаются в f64 без потерь"
    )]
    let temp_cutoff = (now - STICKER_TEMP_AGE_MS) as f64;
    for file in disk_files(&dir) {
        let temporary = Path::new(&file.name)
            .extension()
            .is_some_and(|ext| ext == "tmp");
        if !temporary || file.modified_ms > temp_cutoff {
            continue;
        }
        if !dry_run && let Err(error) = std::fs::remove_file(dir.join(&file.name)) {
            result
                .errors
                .push(format!("Failed to delete {}: {error}", file.name));
            continue;
        }
        result.deleted_files += 1;
        result.freed_bytes += file.size;
    }
    Ok(())
}

/// Общая уборка (`cleanupStorage`): файлы без записей, неприкреплённые и
/// осиротевшие вложения, ненужные стикеры.
///
/// # Errors
/// Ошибка базы.
pub fn cleanup(tx: &mut dyn Tx, config: &Config, dry_run: bool) -> DbResult<Cleanup> {
    let mut result = Cleanup::default();
    let profile = profile_files(tx)?;
    let stray = orphan_files(tx, config)?;
    let loose = unlinked(tx, &profile)?;
    for file in stray {
        if !dry_run && let Err(error) = remove_upload(config, &file.name) {
            result
                .errors
                .push(format!("Failed to delete {}: {error}", file.name));
            continue;
        }
        result.deleted_files += 1;
        result.freed_bytes += file.size;
    }
    for att in loose {
        if !dry_run {
            if let Err(error) = remove_upload(config, &att.filename) {
                result
                    .errors
                    .push(format!("Failed to clean up attachment {}: {error}", att.id));
                continue;
            }
            tx.execute("DELETE FROM attachments WHERE id = ?", params![&att.id])?;
        }
        result.deleted_records += 1;
        result.freed_bytes += att.size;
    }
    for att in dangling(tx)? {
        if !dry_run {
            if let Err(error) = remove_upload(config, &att.filename) {
                result.errors.push(format!(
                    "Failed to clean up dangling attachment {}: {error}",
                    att.id
                ));
                continue;
            }
            tx.execute("DELETE FROM attachments WHERE id = ?", params![&att.id])?;
        }
        result.deleted_files += 1;
        result.freed_bytes += att.size;
        result.deleted_records += 1;
    }
    cleanup_stickers(tx, config, dry_run, &mut result)?;
    crate::routes::game_icons::cleanup(tx, config, dry_run, &mut result)?;
    Ok(result)
}

/// Удаляет вложения сообщений старше `max_age_days` (`cleanupOldMedia`).
///
/// # Errors
/// Ошибка базы.
pub fn cleanup_old_media(
    tx: &mut dyn Tx,
    config: &Config,
    max_age_days: f64,
    dry_run: bool,
) -> DbResult<Cleanup> {
    let mut result = Cleanup::default();
    let profile = profile_files(tx)?;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "срок в миллисекундах"
    )]
    let cutoff = now_ms() - (max_age_days * 86_400_000.0) as i64;
    let old = attachments(
        tx,
        "SELECT id, filename, thumbnail_filename, size FROM attachments \
         WHERE (message_id IS NOT NULL OR dm_message_id IS NOT NULL) AND created_at < ?",
        params![cutoff],
    )?;
    for att in old {
        if profile.contains(basename(&att.filename)) {
            continue;
        }
        if !dry_run {
            if let Err(error) = remove_upload(config, &att.filename) {
                result
                    .errors
                    .push(format!("Failed to delete old media {}: {error}", att.id));
                continue;
            }
            tx.execute("DELETE FROM attachments WHERE id = ?", params![&att.id])?;
        }
        result.deleted_files += 1;
        result.freed_bytes += att.size;
        result.deleted_records += 1;
    }
    Ok(result)
}

/// Удаляет файлы незавершённых загрузок старше порога (`cleanupStaleTusSessions`).
#[must_use]
pub fn cleanup_stale_uploads(config: &Config, threshold_ms: f64, dry_run: bool) -> Cleanup {
    let mut result = Cleanup::default();
    for (path, name, size) in stale_uploads(config, threshold_ms) {
        if !dry_run && let Err(error) = std::fs::remove_file(&path) {
            result
                .errors
                .push(format!("Failed to delete {name}: {error}"));
            continue;
        }
        result.deleted_files += 1;
        result.freed_bytes += size;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_sticker_ids() {
        assert_eq!(classify("a.JPG"), "image");
        assert_eq!(classify("b.mkv"), "video");
        assert_eq!(classify("c.opus"), "audio");
        assert_eq!(classify("d.xlsx"), "document");
        assert_eq!(classify("noext"), "other");
        let id = "ab".repeat(32);
        assert_eq!(
            sticker_id(&format!(
                "sticker:https://h.example/api/stickers/assets/{id}.webp"
            )),
            Some(id)
        );
        assert_eq!(sticker_id("sticker:что-то"), None);
    }
}
