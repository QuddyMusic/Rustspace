//! Файлы настроек. На десктопе это обычная файловая система; в браузере
//! дисков нет, и те же пути хранятся в `localStorage` страницы (ключ —
//! `rustspace:` и путь). Остальные операции в браузере честно отвечают
//! «не поддерживается».

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use std::fs::*;

#[cfg(target_arch = "wasm32")]
pub(crate) use web::*;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::io::{Error, ErrorKind, Result};
    use std::path::Path;

    use base64::Engine as _;

    pub(crate) use std::fs::{
        File, OpenOptions, canonicalize, metadata, read_dir, read_link, set_permissions,
        symlink_metadata,
    };

    /// Префикс ключей в `localStorage`.
    const PREFIX: &str = "rustspace:";

    fn storage() -> Result<web_sys::Storage> {
        web_sys::window()
            .and_then(|window| window.local_storage().ok().flatten())
            .ok_or_else(|| Error::new(ErrorKind::Unsupported, "localStorage недоступен"))
    }

    fn key(path: &Path) -> String {
        format!("{PREFIX}{}", path.to_string_lossy())
    }

    fn failed(err: &wasm_bindgen::JsValue) -> Error {
        Error::other(format!("{err:?}"))
    }

    /// Все ключи, которые лежат «внутри» каталога.
    fn keys_under(storage: &web_sys::Storage, dir: &Path) -> Vec<String> {
        let prefix = format!("{}/", key(dir).trim_end_matches('/'));
        let count = storage.length().unwrap_or(0);
        (0..count)
            .filter_map(|index| storage.key(index).ok().flatten())
            .filter(|name| name.starts_with(&prefix))
            .collect()
    }

    pub(crate) fn read<P: AsRef<Path>>(path: P) -> Result<Vec<u8>> {
        let value = storage()?
            .get_item(&key(path.as_ref()))
            .map_err(|err| failed(&err))?
            .ok_or_else(|| Error::from(ErrorKind::NotFound))?;
        match value.split_at_checked(1) {
            Some(("t", text)) => Ok(text.as_bytes().to_vec()),
            Some(("b", packed)) => base64::engine::general_purpose::STANDARD
                .decode(packed)
                .map_err(|err| Error::new(ErrorKind::InvalidData, err)),
            _ => Err(Error::from(ErrorKind::InvalidData)),
        }
    }

    pub(crate) fn read_to_string<P: AsRef<Path>>(path: P) -> Result<String> {
        String::from_utf8(read(path)?).map_err(|err| Error::new(ErrorKind::InvalidData, err))
    }

    pub(crate) fn write<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, contents: C) -> Result<()> {
        let bytes = contents.as_ref();
        let value = std::str::from_utf8(bytes).map_or_else(
            |_| {
                format!(
                    "b{}",
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                )
            },
            |text| format!("t{text}"),
        );
        storage()?
            .set_item(&key(path.as_ref()), &value)
            .map_err(|err| failed(&err))
    }

    #[allow(clippy::unnecessary_wraps, reason = "как у std::fs")]
    pub(crate) fn create_dir_all<P: AsRef<Path>>(_path: P) -> Result<()> {
        Ok(())
    }

    pub(crate) fn remove_file<P: AsRef<Path>>(path: P) -> Result<()> {
        storage()?
            .remove_item(&key(path.as_ref()))
            .map_err(|err| failed(&err))
    }

    pub(crate) fn remove_dir_all<P: AsRef<Path>>(path: P) -> Result<()> {
        let storage = storage()?;
        for name in keys_under(&storage, path.as_ref()) {
            storage.remove_item(&name).map_err(|err| failed(&err))?;
        }
        Ok(())
    }

    pub(crate) fn rename<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> Result<()> {
        let storage = storage()?;
        let (from, to) = (key(from.as_ref()), key(to.as_ref()));
        if let Some(value) = storage.get_item(&from).map_err(|err| failed(&err))? {
            storage.set_item(&to, &value).map_err(|err| failed(&err))?;
            storage.remove_item(&from).map_err(|err| failed(&err))?;
            return Ok(());
        }
        let moved = keys_under(&storage, Path::new(from.trim_start_matches(PREFIX)));
        if moved.is_empty() {
            return Err(Error::from(ErrorKind::NotFound));
        }
        for name in moved {
            if let Some(value) = storage.get_item(&name).map_err(|err| failed(&err))? {
                storage
                    .set_item(&name.replacen(&from, &to, 1), &value)
                    .map_err(|err| failed(&err))?;
                storage.remove_item(&name).map_err(|err| failed(&err))?;
            }
        }
        Ok(())
    }
}

/// Записывает файл, читать который может только владелец (0600), через
/// временный файл. В браузере — обычная запись в хранилище страницы.
///
/// # Errors
/// Файл не записался.
#[cfg(target_arch = "wasm32")]
pub(crate) fn write_private(path: &std::path::Path, body: &[u8]) -> std::io::Result<()> {
    write(path, body)
}

/// Записывает файл, читать который может только владелец (0600), через
/// временный файл.
///
/// # Errors
/// Файл не записался.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn write_private(path: &std::path::Path, body: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let _ = remove_file(&tmp);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let written = (|| {
        let mut file = options.open(&tmp)?;
        std::io::Write::write_all(&mut file, body)?;
        file.sync_all()?;
        drop(file);
        rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = remove_file(&tmp);
    }
    written
}

/// Номер процесса для имён временных файлов (в браузере процесса нет).
#[must_use]
pub(crate) fn pid() -> u32 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::process::id()
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}
