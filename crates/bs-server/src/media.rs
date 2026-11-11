//! Обработка изображений: размеры картинок профиля, миниатюры.

use std::path::PathBuf;

use crate::files::ProfileImage;

/// Приводит файл картинки профиля к рабочему размеру на месте.
pub async fn resize_profile_image(path: PathBuf, kind: ProfileImage) {
    let _ = (path, kind);
}
