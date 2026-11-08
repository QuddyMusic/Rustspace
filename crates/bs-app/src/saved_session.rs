//! Сохранённая сессия домашнего инстанса: вход без пароля после перезапуска.
//!
//! В файле лежат адрес инстанса, флаг «разрешён http» и токен доступа. Токен —
//! секрет: файл создаётся с правами `0600` (на Unix), пишется атомарно через
//! временный файл, содержимое не попадает в логи.

use std::path::{Path, PathBuf};

use bs_types::TransportPolicy;

use crate::bridge::Secret;

/// Имя файла в каталоге настроек приложения.
const FILE_NAME: &str = "session.json";

/// Что нужно, чтобы войти без пароля.
pub(crate) struct SavedSession {
    /// Адрес домашнего инстанса.
    pub(crate) base: String,
    /// Токен доступа.
    pub(crate) token: Secret,
    /// Пользователь разрешал открытый `http://` для этого инстанса.
    pub(crate) transport: TransportPolicy,
}

/// Каталог настроек приложения (`None`, если домашний каталог неизвестен).
fn config_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        var("APPDATA")?
    } else if cfg!(target_os = "macos") {
        var("HOME")?.join("Library").join("Application Support")
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))?
    };
    Some(base.join("backspace-rs"))
}

/// Путь к файлу сессии по умолчанию.
pub(crate) fn default_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(FILE_NAME))
}

/// Читает сохранённую сессию; повреждённый или пустой файл — `None`.
pub(crate) fn load(path: &Path) -> Option<SavedSession> {
    let bytes = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let text = |key: &str| value.get(key).and_then(serde_json::Value::as_str);
    let base = text("base")?.trim();
    let token = text("token")?;
    if base.is_empty() || token.is_empty() {
        return None;
    }
    let insecure = value
        .get("allowInsecure")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Some(SavedSession {
        base: base.to_owned(),
        token: Secret::new(token),
        transport: if insecure {
            TransportPolicy::AllowInsecure
        } else {
            TransportPolicy::Strict
        },
    })
}

/// Сохраняет сессию (атомарно, права `0600`).
pub(crate) fn save(
    path: &Path,
    base: &str,
    token: &str,
    transport: TransportPolicy,
) -> std::io::Result<()> {
    use std::io::Write as _;

    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let json = zeroize::Zeroizing::new(
        serde_json::to_vec_pretty(&serde_json::json!({
            "base": base,
            "token": token,
            "allowInsecure": transport == TransportPolicy::AllowInsecure,
        }))
        .map_err(std::io::Error::other)?,
    );
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let write = || -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(&json)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    };
    let result = write();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Стирает сохранённую сессию (выход, отказ токена, удаление аккаунта).
pub(crate) fn clear(path: &Path) {
    if let Err(err) = std::fs::remove_file(path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%err, "не удалось удалить файл сессии");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bs-saved-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("nested").join(FILE_NAME)
    }

    #[test]
    fn save_then_load_round_trips() {
        let path = temp_path("rt");
        save(
            &path,
            "https://x.example",
            "tok",
            TransportPolicy::AllowInsecure,
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.base, "https://x.example");
        assert_eq!(loaded.token.expose(), "tok");
        assert_eq!(loaded.transport, TransportPolicy::AllowInsecure);
    }

    #[test]
    fn missing_or_broken_file_is_none() {
        let path = temp_path("bad");
        assert!(load(&path).is_none());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(load(&path).is_none());
        std::fs::write(&path, br#"{"base":"","token":"t"}"#).unwrap();
        assert!(load(&path).is_none());
    }

    #[test]
    fn clear_removes_file_and_tolerates_absence() {
        let path = temp_path("clr");
        save(&path, "https://x.example", "tok", TransportPolicy::Strict).unwrap();
        clear(&path);
        assert!(load(&path).is_none());
        clear(&path);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let path = temp_path("perm");
        save(&path, "https://x.example", "tok", TransportPolicy::Strict).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
