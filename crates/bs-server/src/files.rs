//! Файлы загрузок на диске и записи о вложениях.

use std::path::PathBuf;

use crate::config::Config;
use crate::db::{DbResult, Tx};
use crate::params;

/// Имя файла без каталогов (`path.basename`): из ссылки или имени нельзя
/// выйти за пределы каталога загрузок.
#[must_use]
pub fn basename(name: &str) -> &str {
    let name = name.trim_end_matches(['/', '\\']);
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// Путь файла в каталоге загрузок; `None` для имён, которые файлом быть не
/// могут.
#[must_use]
pub fn upload_path(config: &Config, name: &str) -> Option<PathBuf> {
    let name = basename(name);
    if name.is_empty() || name == "." || name == ".." || name.contains('\0') {
        return None;
    }
    Some(config.upload_dir.join(name))
}

/// Имя миниатюры для файла (`thumbFilename`).
#[must_use]
pub fn thumb_filename(original: &str) -> String {
    let name = basename(original);
    let stem = match name.rfind('.') {
        Some(0) | None => name,
        Some(dot) => &name[..dot],
    };
    format!("{stem}_thumb.webp")
}

/// Удаляет файл загрузки и его миниатюру; отсутствие файла — не ошибка.
pub async fn delete_upload_file(config: &Config, name: &str) {
    for name in [basename(name).to_owned(), thumb_filename(name)] {
        let Some(path) = upload_path(config, &name) else {
            continue;
        };
        if let Err(error) = tokio::fs::remove_file(&path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::error!(%error, file = %name, "не удалось удалить файл загрузки");
        }
    }
}

/// Удаляет запись о вложении по имени файла (сам файл остаётся: он стал
/// аватаром, значком или обложкой). Возвращает имя миниатюры, если она была.
///
/// # Errors
/// Ошибка базы.
pub fn delete_attachment_by_filename(tx: &mut dyn Tx, name: &str) -> DbResult<Option<String>> {
    let name = basename(name);
    let Some(row) = tx.query_one(
        "SELECT id, thumbnail_filename FROM attachments WHERE filename = ?",
        params![name],
    )?
    else {
        return Ok(None);
    };
    tx.execute(
        "DELETE FROM attachments WHERE id = ?",
        params![row.text("id")?],
    )?;
    row.opt_text("thumbnail_filename")
}

/// Какая картинка профиля приводится к размеру.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileImage {
    /// Аватар пользователя.
    Avatar,
    /// Значок пространства.
    Icon,
    /// Обложка.
    Banner,
}

/// Приводит загруженную картинку профиля к рабочему размеру
/// (`resizeProfileImage`). Внешние ссылки не трогает.
pub async fn resize_profile_image(config: &Config, url: Option<&str>, kind: ProfileImage) {
    let Some(url) = url.filter(|url| !url.is_empty() && !url.starts_with("http")) else {
        return;
    };
    let Some(path) = upload_path(config, url) else {
        return;
    };
    crate::media::resize_profile_image(path, kind).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cannot_escape_the_upload_directory() {
        assert_eq!(basename("/api/uploads/a.png"), "a.png");
        assert_eq!(basename("..\\..\\etc\\passwd"), "passwd");
        assert_eq!(basename("../../etc/passwd"), "passwd");
        assert_eq!(thumb_filename("/api/uploads/a.b.png"), "a.b_thumb.webp");
        assert_eq!(thumb_filename("noext"), "noext_thumb.webp");
    }
}
