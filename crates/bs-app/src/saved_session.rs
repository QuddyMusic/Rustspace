//! Сохранённая сессия домашнего инстанса: вход без пароля после перезапуска.
//!
//! В файле лежат адрес инстанса и флаг «разрешён http». Токен доступа — секрет:
//! он хранится в системном хранилище ключей (Secret Service на Linux, Keychain
//! на macOS, Credential Manager в Windows), а в файле остаётся только пометка
//! `tokenInKeyring`. Если хранилища нет (минимальная система без демона
//! секретов), токен пишется в сам файл, как раньше: права `0600` (на Unix),
//! атомарная запись через временный файл, содержимое не попадает в логи.

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
pub(crate) fn config_dir() -> Option<PathBuf> {
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
    let in_keyring = value
        .get("tokenInKeyring")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let from_keyring;
    let token = match text("token") {
        Some(token) => token,
        None if in_keyring => {
            from_keyring = keystore::get(path)?;
            from_keyring.as_str()
        }
        None => return None,
    };
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

/// Адрес и флаг http из файла сессии, даже если токена уже нет (например,
/// хранилище ключей недоступно): чтобы форма входа была заполнена.
pub(crate) fn hint(path: &Path) -> Option<(String, bool)> {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let base = value.get("base")?.as_str()?.trim().to_owned();
    let insecure = value
        .get("allowInsecure")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    (!base.is_empty()).then_some((base, insecure))
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
    let insecure = transport == TransportPolicy::AllowInsecure;
    let body = if keystore::set(path, token) {
        serde_json::json!({ "base": base, "tokenInKeyring": true, "allowInsecure": insecure })
    } else {
        serde_json::json!({ "base": base, "token": token, "allowInsecure": insecure })
    };
    let json =
        zeroize::Zeroizing::new(serde_json::to_vec_pretty(&body).map_err(std::io::Error::other)?);
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
    keystore::delete(path);
    if let Err(err) = std::fs::remove_file(path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%err, "не удалось удалить файл сессии");
    }
}

/// Системное хранилище ключей. В тестах отключено: они не должны трогать
/// настоящий keyring пользователя, поэтому всегда идёт запасной путь — файл.
mod keystore {
    use std::path::Path;

    const SERVICE: &str = "backspace-rs";

    /// Запись привязана к файлу сессии: разные каталоги настроек (профили,
    /// тесты) не затирают друг друга.
    #[cfg(not(test))]
    fn entry(path: &Path) -> Option<keyring::Entry> {
        let account = format!("session:{}", path.display());
        keyring::Entry::new(SERVICE, &account)
            .map_err(|err| tracing::debug!(%err, "keyring недоступен"))
            .ok()
    }

    /// Сохраняет токен; `false` — хранилища нет, нужен запасной путь.
    #[cfg(not(test))]
    pub(super) fn set(path: &Path, token: &str) -> bool {
        entry(path).is_some_and(|entry| match entry.set_password(token) {
            Ok(()) => true,
            Err(err) => {
                tracing::info!(%err, "keyring недоступен — токен сохранён в файл 0600");
                false
            }
        })
    }

    #[cfg(not(test))]
    pub(super) fn get(path: &Path) -> Option<zeroize::Zeroizing<String>> {
        entry(path)?
            .get_password()
            .map(zeroize::Zeroizing::new)
            .map_err(|err| tracing::warn!(%err, "токен в keyring не найден"))
            .ok()
    }

    #[cfg(not(test))]
    pub(super) fn delete(path: &Path) {
        if let Some(entry) = entry(path) {
            let _ = entry.delete_credential();
        }
    }

    #[cfg(test)]
    pub(super) const fn set(_path: &Path, _token: &str) -> bool {
        let _ = SERVICE;
        false
    }

    #[cfg(test)]
    pub(super) const fn get(_path: &Path) -> Option<zeroize::Zeroizing<String>> {
        None
    }

    #[cfg(test)]
    pub(super) const fn delete(_path: &Path) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyring_marker_without_keyring_is_no_session() {
        let path = temp_path("kr");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            br#"{"base":"https://x.example","tokenInKeyring":true}"#,
        )
        .unwrap();
        assert!(load(&path).is_none());
    }

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
