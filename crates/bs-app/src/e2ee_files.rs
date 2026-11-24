//! Ключи зашифрованных вложений личной переписки.
//!
//! Ключ файла приходит внутри зашифрованного текста сообщения; здесь он
//! хранится по имени файла на сервере, пока клиент запущен. Картинки и
//! скачивания по этому имени расшифровываются после загрузки.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};

static KEYS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Mutex::default);

/// Последняя часть пути: имя файла на сервере.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Запоминает ключ файла.
pub(crate) fn remember(filename: &str, key: &str) {
    KEYS.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(file_name(filename).to_owned(), key.to_owned());
}

/// Ключ файла по пути вложения, если он зашифрован.
pub(crate) fn key_for(path: &str) -> Option<String> {
    KEYS.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(file_name(path))
        .cloned()
}

/// Расшифровывает файл, если он зашифрован; иначе отдаёт как есть.
pub(crate) fn reveal(path: &str, bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    match key_for(path) {
        Some(key) => bs_e2ee::open_file(&bytes, &key).map_err(|err| err.to_string()),
        None => Ok(bytes),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn keys_by_file_name() {
        super::remember("abc.bin", "k");
        assert_eq!(super::key_for("/api/uploads/abc.bin").as_deref(), Some("k"));
        assert!(super::key_for("/api/uploads/other.bin").is_none());
    }
}
