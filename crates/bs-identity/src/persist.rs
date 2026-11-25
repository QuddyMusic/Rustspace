//! Файловое хранилище токенов: переживает перезапуск клиента.
//!
//! Формат — JSON `{ "origin": "token", ... }`, где ключ origin в веб-форме
//! (`''` для домашнего инстанса, `https://...` для удалённых) — ровно как
//! [`Origin::as_web_str`]. На Unix файл создаётся с правами `0600`.
//!
//! Токены — секреты: не логируйте содержимое файла и не кладите его в git.

use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

use bs_types::Origin;

use crate::error::IdentityError;
use crate::secret::AccessToken;
use crate::store::CredentialStore;

/// Ошибка файлового хранилища токенов.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PersistError {
    /// Ошибка ввода-вывода.
    #[error("ошибка файла токенов: {0}")]
    Io(#[from] std::io::Error),

    /// Файл повреждён или имеет чужой формат.
    #[error("файл токенов повреждён: {0}")]
    Json(#[from] serde_json::Error),

    /// В файле встретился origin, который не удалось разобрать.
    #[error("некорректный origin в файле токенов: {origin}")]
    BadOrigin {
        /// Строка origin из файла.
        origin: String,
    },
}

impl From<PersistError> for IdentityError {
    fn from(err: PersistError) -> Self {
        match err {
            PersistError::Io(io) => Self::Io(io),
            PersistError::Json(json) => Self::TokenFileCorrupted(json.to_string()),
            PersistError::BadOrigin { origin } => {
                Self::TokenFileCorrupted(format!("origin: {origin}"))
            }
        }
    }
}

impl CredentialStore {
    /// Сериализует токены в JSON-объект.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let map: BTreeMap<String, String> = self
            .origins()
            .filter_map(|origin| {
                let token = self.get(origin).ok()?;
                Some((origin.as_web_str(), token.expose().to_owned()))
            })
            .collect();
        serde_json::to_value(map)
            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()))
    }

    /// Восстанавливает хранилище из JSON-объекта.
    ///
    /// Пустой origin (`''`) — домашний инстанс. Неизвестные ключи не мешают:
    /// любая строка, не разбирающаяся как URL, считается ошибкой формата.
    pub fn from_json(value: &serde_json::Value) -> Result<Self, PersistError> {
        let mut store = Self::new();
        let Some(map) = value.as_object() else {
            return Ok(store);
        };
        for (origin_str, token_value) in map {
            let Some(token) = token_value.as_str() else {
                continue;
            };
            if token.is_empty() {
                continue;
            }
            let origin =
                Origin::parse_web_str(origin_str).map_err(|_| PersistError::BadOrigin {
                    origin: origin_str.clone(),
                })?;
            store.insert(origin, AccessToken::new(token));
        }
        Ok(store)
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Сохраняет токены в файл (атомарно, через временный файл).
    ///
    /// На Unix итоговый файл получает права `0600`; каталог создаётся, если
    /// его нет.
    pub async fn save_to_path(&self, path: &Path) -> Result<(), PersistError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent).await?;
        }
        let json = zeroize::Zeroizing::new(serde_json::to_vec_pretty(&self.to_json())?);
        // Имя временного файла непредсказуемо, а сам файл создаётся сразу
        // с правами 0600 и `create_new`: ни окна, когда токены читаются всеми,
        // ни подмены через заранее созданную символическую ссылку.
        let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
        let write = async {
            use tokio::io::AsyncWriteExt as _;
            let mut file = open_private(&tmp).await?;
            file.write_all(&json).await?;
            file.sync_all().await?;
            drop(file);
            tokio::fs::rename(&tmp, path).await
        };
        if let Err(err) = write.await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(err.into());
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Загружает токены из файла. Отсутствующий файл — пустое хранилище.
    pub async fn load_from_path(path: &Path) -> Result<Self, PersistError> {
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::new()),
            Err(err) => return Err(err.into()),
        };
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        Self::from_json(&value)
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Создаёт новый файл, доступный только владельцу (на Unix — `0600`).
async fn open_private(path: &Path) -> Result<tokio::fs::File, std::io::Error> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(url: &str) -> Origin {
        Origin::parse_web_str(url).unwrap()
    }

    #[test]
    fn json_roundtrip_keeps_origins() {
        let mut store = CredentialStore::new();
        store.insert(Origin::Home, AccessToken::new("home-token"));
        store.insert(
            remote("https://nova.ddns.net"),
            AccessToken::new("remote-token"),
        );

        let json = store.to_json();
        let restored = CredentialStore::from_json(&json).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.get(&Origin::Home).unwrap().expose(), "home-token");
        assert_eq!(
            restored
                .get(&remote("https://nova.ddns.net"))
                .unwrap()
                .expose(),
            "remote-token"
        );
    }

    #[test]
    fn json_skips_empty_and_non_string_values() {
        let value = serde_json::json!({
            "": "home-token",
            "https://a.example": "",
            "https://b.example": 42,
        });
        let store = CredentialStore::from_json(&value).unwrap();
        assert_eq!(store.len(), 1);
        assert!(store.contains(&Origin::Home));
    }

    #[test]
    fn json_rejects_bad_origin() {
        let value = serde_json::json!({ "not a url": "token" });
        assert!(matches!(
            CredentialStore::from_json(&value),
            Err(PersistError::BadOrigin { .. })
        ));
    }

    #[tokio::test]
    async fn save_and_load_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("bs-identity-test-{}", std::process::id()));
        let path = dir.join("tokens.json");
        let mut store = CredentialStore::new();
        store.insert(Origin::Home, AccessToken::new("abc"));
        store.save_to_path(&path).await.unwrap();

        let loaded = CredentialStore::load_from_path(&path).await.unwrap();
        assert_eq!(loaded.get(&Origin::Home).unwrap().expose(), "abc");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "token file must not be world-readable");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn load_missing_file_is_empty_store() {
        let path =
            std::env::temp_dir().join(format!("bs-identity-missing-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = CredentialStore::load_from_path(&path).await.unwrap();
        assert!(store.is_empty());
    }
}
