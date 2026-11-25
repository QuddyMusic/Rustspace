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
/// Имя файла сессии (в тестах; список аккаунтов — в `accounts`).
#[cfg(test)]
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

/// Имя каталога настроек.
const DIR_NAME: &str = "rustspace";
/// Имя каталога настроек до переименования в Rustspace.
const LEGACY_DIR_NAME: &str = "backspace-rs";

/// Каталог настроек приложения (`None`, если домашний каталог неизвестен).
pub(crate) fn config_dir() -> Option<PathBuf> {
    config_base().map(|base| base.join(DIR_NAME))
}

/// Каталог настроек со старым именем.
fn legacy_config_dir() -> Option<PathBuf> {
    config_base().map(|base| base.join(LEGACY_DIR_NAME))
}

/// Переносит настройки из каталога со старым именем, если нового ещё нет.
pub(crate) fn migrate_legacy_dir() {
    let (Some(new), Some(old)) = (config_dir(), legacy_config_dir()) else {
        return;
    };
    if new.exists() || !old.is_dir() {
        return;
    }
    match crate::files::rename(&old, &new) {
        Ok(()) => {
            tracing::info!(from = %old.display(), to = %new.display(), "настройки перенесены");
        }
        Err(err) => {
            tracing::warn!(%err, "не удалось перенести настройки из старого каталога");
        }
    }
}

/// Запись хранилища ключей со старым именем каталога в пути (`None` — путь
/// не из каталога настроек).
#[cfg_attr(
    any(test, target_arch = "wasm32"),
    allow(dead_code, reason = "keyring в тестах отключён")
)]
fn legacy_account(account: &str) -> Option<String> {
    let new = config_dir()?.display().to_string();
    let old = legacy_config_dir()?.display().to_string();
    account
        .contains(&new)
        .then(|| account.replacen(&new, &old, 1))
}

fn config_base() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(target_arch = "wasm32") {
        // В браузере пути — ключи хранилища страницы.
        PathBuf::from("/")
    } else if cfg!(windows) {
        var("APPDATA")?
    } else if cfg!(target_os = "macos") {
        var("HOME")?.join("Library").join("Application Support")
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))?
    };
    Some(base)
}

/// Читает сохранённую сессию; повреждённый или пустой файл — `None`.
pub(crate) fn load(path: &Path) -> Option<SavedSession> {
    let bytes = crate::files::read(path).ok()?;
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
            from_keyring = keystore::get(&session_account(path))?;
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
    let value: serde_json::Value = serde_json::from_slice(&crate::files::read(path).ok()?).ok()?;
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
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        crate::files::create_dir_all(parent)?;
    }
    let insecure = transport == TransportPolicy::AllowInsecure;
    let body = if keystore::set(&session_account(path), token) {
        serde_json::json!({ "base": base, "tokenInKeyring": true, "allowInsecure": insecure })
    } else {
        serde_json::json!({ "base": base, "token": token, "allowInsecure": insecure })
    };
    let json =
        zeroize::Zeroizing::new(serde_json::to_vec_pretty(&body).map_err(std::io::Error::other)?);
    crate::files::write_private(path, &json)
}

/// Стирает сохранённую сессию (выход, отказ токена, удаление аккаунта).
pub(crate) fn clear(path: &Path) {
    keystore::delete(&session_account(path));
    if let Err(err) = crate::files::remove_file(path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%err, "не удалось удалить файл сессии");
    }
}

/// Запись сессии в системном хранилище: привязана к файлу сессии, чтобы
/// разные каталоги настроек (профили, тесты) не затирали друг друга.
fn session_account(path: &Path) -> String {
    format!("session:{}", path.display())
}

/// Системное хранилище ключей (Secret Service, Keychain, Credential Manager).
/// В тестах отключено: они не должны трогать настоящий keyring пользователя,
/// поэтому всегда идёт запасной путь — файл. В браузере keyring нет.
pub(crate) mod keystore {
    const SERVICE: &str = "rustspace";
    /// Имя службы до переименования: секреты оттуда переезжают при чтении.
    #[cfg(not(any(test, target_arch = "wasm32")))]
    const LEGACY_SERVICE: &str = "backspace-rs";

    #[cfg(not(any(test, target_arch = "wasm32")))]
    fn entry(account: &str) -> Option<keyring::Entry> {
        keyring::Entry::new(SERVICE, account)
            .map_err(|err| tracing::debug!(%err, "keyring недоступен"))
            .ok()
    }

    /// Секрет, сохранённый до переименования; переносится под новое имя.
    #[cfg(not(any(test, target_arch = "wasm32")))]
    fn take_legacy(account: &str) -> Option<zeroize::Zeroizing<String>> {
        let old = keyring::Entry::new(LEGACY_SERVICE, &super::legacy_account(account)?).ok()?;
        let secret = zeroize::Zeroizing::new(old.get_password().ok()?);
        if set(account, &secret) {
            let _ = old.delete_credential();
        }
        Some(secret)
    }

    /// Сохраняет секрет; `false` — хранилища нет, нужен запасной путь.
    #[cfg(not(any(test, target_arch = "wasm32")))]
    pub(crate) fn set(account: &str, secret: &str) -> bool {
        entry(account).is_some_and(|entry| match entry.set_password(secret) {
            Ok(()) => true,
            Err(err) => {
                tracing::info!(%err, "keyring недоступен — секрет сохранён в файл 0600");
                false
            }
        })
    }

    /// Читает секрет.
    #[cfg(not(any(test, target_arch = "wasm32")))]
    pub(crate) fn get(account: &str) -> Option<zeroize::Zeroizing<String>> {
        match entry(account)?.get_password() {
            Ok(secret) => Some(zeroize::Zeroizing::new(secret)),
            Err(err) => take_legacy(account).or_else(|| {
                tracing::warn!(%err, "секрет в keyring не найден");
                None
            }),
        }
    }

    /// Стирает секрет.
    #[cfg(not(any(test, target_arch = "wasm32")))]
    pub(crate) fn delete(account: &str) {
        if let Some(entry) = entry(account) {
            let _ = entry.delete_credential();
        }
    }

    #[cfg(any(test, target_arch = "wasm32"))]
    pub(crate) const fn set(_account: &str, _secret: &str) -> bool {
        let _ = SERVICE;
        false
    }

    #[cfg(any(test, target_arch = "wasm32"))]
    pub(crate) const fn get(_account: &str) -> Option<zeroize::Zeroizing<String>> {
        None
    }

    #[cfg(any(test, target_arch = "wasm32"))]
    pub(crate) const fn delete(_account: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyring_marker_without_keyring_is_no_session() {
        let path = temp_path("kr");
        crate::files::create_dir_all(path.parent().unwrap()).unwrap();
        crate::files::write(
            &path,
            br#"{"base":"https://x.example","tokenInKeyring":true}"#,
        )
        .unwrap();
        assert!(load(&path).is_none());
    }

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bs-saved-{name}-{}", std::process::id()));
        let _ = crate::files::remove_dir_all(&dir);
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
        crate::files::create_dir_all(path.parent().unwrap()).unwrap();
        crate::files::write(&path, b"{ not json").unwrap();
        assert!(load(&path).is_none());
        crate::files::write(&path, br#"{"base":"","token":"t"}"#).unwrap();
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
