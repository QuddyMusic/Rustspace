//! Несколько аккаунтов: у каждого свой файл сессии в каталоге настроек
//! (токен — в системном хранилище ключей, как у единственной сессии), а
//! `accounts.json` помнит, какие они и какой из них активный.
//!
//! Переключение — выход без забывания и вход по файлу другого аккаунта.
//! Ключи шифрования переписок лежат рядом с файлом сессии и привязаны к
//! пользователю, поэтому у каждого аккаунта они свои.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Имя списка в каталоге настроек.
const INDEX: &str = "accounts.json";
/// Файл сессии первого (и единственного до этой функции) аккаунта.
const FIRST: &str = "session.json";

/// Сохранённый аккаунт.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Account {
    /// Имя файла сессии в каталоге настроек.
    pub(crate) file: String,
    /// Отображаемое имя.
    pub(crate) name: String,
    /// Логин.
    pub(crate) username: String,
    /// Адрес инстанса.
    pub(crate) base: String,
    /// Ключ и цвет аватара-заглушки (как в чате).
    #[serde(default)]
    pub(crate) face_key: String,
    #[serde(default)]
    pub(crate) avatar_color: Option<String>,
}

impl Account {
    /// Аватар-заглушка, как у этого пользователя в чате.
    pub(crate) fn face(&self) -> crate::theme::Face<'_> {
        crate::theme::Face {
            name: &self.name,
            key: (!self.face_key.is_empty()).then_some(self.face_key.as_str()),
            color: self.avatar_color.as_deref(),
        }
    }

    /// Подпись «логин · хост».
    pub(crate) fn subtitle(&self) -> String {
        let host = url::Url::parse(&self.base)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_else(|| self.base.clone());
        format!("@{} · {host}", self.username)
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Index {
    #[serde(default)]
    active: Option<String>,
    #[serde(default)]
    accounts: Vec<Account>,
}

/// Прочитанный список (чтобы не ходить на диск каждый кадр).
static CACHE: Mutex<Option<Index>> = Mutex::new(None);

fn dir() -> Option<PathBuf> {
    crate::saved_session::config_dir()
}

fn read(dir: &Path) -> Index {
    std::fs::read(dir.join(INDEX))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write(dir: &Path, index: &Index) {
    let result = std::fs::create_dir_all(dir).and_then(|()| {
        let json = serde_json::to_vec_pretty(index).map_err(std::io::Error::other)?;
        let tmp = dir.join(format!("{INDEX}.tmp"));
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, dir.join(INDEX))
    });
    if let Err(err) = result {
        tracing::warn!(%err, "список аккаунтов не записан");
    }
}

/// Читает список (из кэша), меняет и записывает, если `change` вернул `true`.
fn with_index<T>(change: impl FnOnce(&mut Index) -> (T, bool)) -> Option<T> {
    let dir = dir()?;
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let index = cache.get_or_insert_with(|| read(&dir));
    let (value, dirty) = change(index);
    if dirty {
        write(&dir, index);
    }
    drop(cache);
    Some(value)
}

/// Имя файла внутри каталога настроек (`None` — файл не оттуда).
fn file_name(path: &Path) -> Option<String> {
    let dir = dir()?;
    (path.parent()? == dir).then(|| path.file_name()?.to_str().map(str::to_owned))?
}

/// Файл сессии активного аккаунта.
pub(crate) fn active_path() -> Option<PathBuf> {
    let active = with_index(|index| (index.active.clone(), false)).flatten();
    dir().map(|dir| dir.join(active.as_deref().unwrap_or(FIRST)))
}

/// Все сохранённые аккаунты.
pub(crate) fn list() -> Vec<Account> {
    with_index(|index| (index.accounts.clone(), false)).unwrap_or_default()
}

/// Путь к файлу сессии аккаунта.
pub(crate) fn path_of(account: &Account) -> Option<PathBuf> {
    dir().map(|dir| dir.join(&account.file))
}

/// Делает аккаунт активным (с него начнётся следующий запуск).
pub(crate) fn set_active(path: &Path) {
    let Some(file) = file_name(path) else {
        return;
    };
    with_index(|index| {
        let dirty = index.active.as_deref() != Some(file.as_str());
        index.active = Some(file);
        ((), dirty)
    });
}

/// Запоминает вошедший аккаунт и делает его активным.
pub(crate) fn remember(path: &Path, me: &bs_types::User, base: &str) {
    let Some(file) = file_name(path) else {
        return;
    };
    let (face_key, avatar_color) = crate::screens::chat::face_of(me);
    let entry = Account {
        file: file.clone(),
        name: me
            .display_name
            .clone()
            .unwrap_or_else(|| me.username.clone()),
        username: me.username.clone(),
        base: base.to_owned(),
        face_key,
        avatar_color,
    };
    with_index(|index| {
        let before = (index.active.clone(), index.accounts.clone());
        // Тот же пользователь на том же инстансе в другом файле — дубль:
        // старый файл заменяется новым.
        index
            .accounts
            .retain(|a| a.file == file || !(a.username == entry.username && a.base == entry.base));
        match index.accounts.iter_mut().find(|a| a.file == file) {
            Some(existing) => *existing = entry,
            None => index.accounts.push(entry),
        }
        index.active = Some(file);
        let dirty = before != (index.active.clone(), index.accounts.clone());
        ((), dirty)
    });
}

/// Убирает аккаунт из списка (после явного выхода из него).
pub(crate) fn forget(path: &Path) {
    let Some(file) = file_name(path) else {
        return;
    };
    with_index(|index| {
        let count = index.accounts.len();
        index.accounts.retain(|a| a.file != file);
        if index.active.as_deref() == Some(file.as_str()) {
            index.active = index.accounts.first().map(|a| a.file.clone());
        }
        ((), count != index.accounts.len())
    });
}

/// Свободный файл сессии для нового аккаунта.
pub(crate) fn new_path() -> Option<PathBuf> {
    let dir = dir()?;
    let taken: Vec<String> = list().into_iter().map(|a| a.file).collect();
    (1..1000)
        .map(|n| {
            if n == 1 {
                FIRST.to_owned()
            } else {
                format!("session-{n}.json")
            }
        })
        .find(|name| !taken.contains(name) && !dir.join(name).exists())
        .map(|name| dir.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(file: &str, user: &str) -> Account {
        Account {
            file: file.to_owned(),
            name: user.to_owned(),
            username: user.to_owned(),
            base: "https://chat.example.org".to_owned(),
            face_key: String::new(),
            avatar_color: None,
        }
    }

    #[test]
    fn index_round_trips_and_subtitle_shows_host() {
        let dir = std::env::temp_dir().join(format!("rs-accounts-{}", std::process::id()));
        let index = Index {
            active: Some("session-2.json".to_owned()),
            accounts: vec![account(FIRST, "quddy"), account("session-2.json", "bob")],
        };
        write(&dir, &index);
        let back = read(&dir);
        assert_eq!(back.active.as_deref(), Some("session-2.json"));
        assert_eq!(back.accounts, index.accounts);
        assert_eq!(back.accounts[1].subtitle(), "@bob · chat.example.org");
        let _ = std::fs::remove_dir_all(dir);
    }
}
