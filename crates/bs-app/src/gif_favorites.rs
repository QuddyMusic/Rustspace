//! Избранные GIF: личная коллекция, которая хранится на этом устройстве.
//!
//! На сервере для GIF нет «избранного» (личные стикеры — отдельная вещь), поэтому
//! коллекция лежит в файле `gif-favorites.json` рядом с настройками. Она своя у
//! каждой пары «инстанс + пользователь», чтобы аккаунты не смешивались.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bs_api::GifItem;
use serde::{Deserialize, Serialize};

/// Имя файла в каталоге настроек.
const FILE_NAME: &str = "gif-favorites.json";
/// Сколько GIF помещается в избранное одного аккаунта.
pub(crate) const MAX_FAVORITES: usize = 500;

/// Запись в файле (`GifItem` сам не сериализуется).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: String,
    #[serde(default)]
    title: String,
    preview_url: String,
    url: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

impl From<&GifItem> for Entry {
    fn from(item: &GifItem) -> Self {
        Self {
            id: item.id.clone(),
            title: item.title.clone(),
            preview_url: item.preview_url.clone(),
            url: item.url.clone(),
            width: item.width,
            height: item.height,
        }
    }
}

impl From<Entry> for GifItem {
    fn from(entry: Entry) -> Self {
        Self {
            id: entry.id,
            title: entry.title,
            preview_url: entry.preview_url,
            url: entry.url,
            width: entry.width,
            height: entry.height,
        }
    }
}

/// Файл целиком: ключ аккаунта → список (новые первыми).
type Book = BTreeMap<String, Vec<Entry>>;

/// Ключ аккаунта.
#[must_use]
pub(crate) fn account_key(base: &str, user_id: &str) -> String {
    format!("{}#{user_id}", base.trim_end_matches('/'))
}

/// Избранное текущего аккаунта.
#[derive(Debug, Default)]
pub(crate) struct GifFavorites {
    path: Option<PathBuf>,
    key: Option<String>,
    items: Vec<GifItem>,
}

fn read_book(path: &Path) -> Book {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

impl GifFavorites {
    /// Подготавливает коллекцию аккаунта `key`; повторный вызов с тем же
    /// ключом ничего не читает.
    pub(crate) fn select(&mut self, key: &str) {
        if self.key.as_deref() == Some(key) {
            return;
        }
        if self.path.is_none() {
            self.path = crate::saved_session::config_dir().map(|dir| dir.join(FILE_NAME));
        }
        self.items = self
            .path
            .as_deref()
            .map(read_book)
            .and_then(|mut book| book.remove(key))
            .map(|entries| entries.into_iter().map(GifItem::from).collect())
            .unwrap_or_default();
        self.key = Some(key.to_owned());
    }

    /// Тот же выбор, но с явным файлом (тесты).
    #[cfg(test)]
    fn with_path(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            ..Self::default()
        }
    }

    /// Избранные GIF, новые первыми.
    pub(crate) fn items(&self) -> &[GifItem] {
        &self.items
    }

    /// Есть ли GIF в избранном.
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.items.iter().any(|item| item.id == id)
    }

    /// Есть ли в избранном GIF с таким адресом (так его знает лента сообщений).
    pub(crate) fn contains_url(&self, url: &str) -> bool {
        self.items.iter().any(|item| item.url == url)
    }

    /// Добавляет GIF из ленты по адресу или убирает его. `size` — размеры, если
    /// картинка уже загружена.
    ///
    /// # Errors
    /// Файл не удалось записать.
    pub(crate) fn toggle_url(
        &mut self,
        url: &str,
        size: Option<[u32; 2]>,
    ) -> std::io::Result<bool> {
        let [width, height] = size.unwrap_or([0, 0]);
        self.toggle(&GifItem {
            id: url.to_owned(),
            title: String::new(),
            preview_url: url.to_owned(),
            url: url.to_owned(),
            width,
            height,
        })
    }

    /// Добавляет GIF или убирает, если он уже там. `Ok(true)` — теперь в избранном.
    ///
    /// # Errors
    /// Файл не удалось записать (список в памяти при этом уже изменён).
    pub(crate) fn toggle(&mut self, item: &GifItem) -> std::io::Result<bool> {
        let added = if let Some(at) = self
            .items
            .iter()
            .position(|i| i.id == item.id || i.url == item.url)
        {
            self.items.remove(at);
            false
        } else {
            self.items.insert(0, item.clone());
            self.items.truncate(MAX_FAVORITES);
            true
        };
        self.save()?;
        Ok(added)
    }

    fn save(&self) -> std::io::Result<()> {
        let (Some(path), Some(key)) = (&self.path, &self.key) else {
            return Ok(());
        };
        // Читаем заново: другой экземпляр приложения мог сохранить чужой аккаунт.
        let mut book = read_book(path);
        book.insert(key.clone(), self.items.iter().map(Entry::from).collect());
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(&book).map_err(std::io::Error::other)?;
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gif(id: &str) -> GifItem {
        GifItem {
            id: id.to_owned(),
            title: format!("гиф {id}"),
            preview_url: format!("https://x/{id}-p.gif"),
            url: format!("https://x/{id}.gif"),
            width: 200,
            height: 100,
        }
    }

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bs-giffav-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("sub").join(FILE_NAME)
    }

    #[test]
    fn toggle_adds_newest_first_and_removes() {
        let path = temp_path("toggle");
        let mut favs = GifFavorites::with_path(path);
        favs.select("https://a#u1");
        assert!(favs.toggle(&gif("1")).unwrap());
        assert!(favs.toggle(&gif("2")).unwrap());
        assert_eq!(favs.items()[0].id, "2");
        assert!(favs.contains("1"));
        assert!(!favs.toggle(&gif("1")).unwrap());
        assert!(!favs.contains("1"));
        assert_eq!(favs.items().len(), 1);
    }

    #[test]
    fn survives_restart_and_keeps_accounts_apart() {
        let path = temp_path("restart");
        let mut first = GifFavorites::with_path(path.clone());
        first.select(&account_key("https://a/", "u1"));
        first.toggle(&gif("1")).unwrap();
        first.select(&account_key("https://a", "u2"));
        assert!(first.items().is_empty());
        first.toggle(&gif("2")).unwrap();

        let mut second = GifFavorites::with_path(path);
        second.select(&account_key("https://a", "u1"));
        assert_eq!(second.items().len(), 1);
        assert_eq!(second.items()[0].url, "https://x/1.gif");
        assert_eq!(second.items()[0].width, 200);
        second.select(&account_key("https://a", "u2"));
        assert_eq!(second.items()[0].id, "2");
    }

    #[test]
    fn chat_gif_and_search_gif_are_the_same_favorite() {
        let mut favs = GifFavorites::with_path(temp_path("url"));
        favs.select("k");
        assert!(favs.toggle(&gif("g1")).unwrap());
        assert!(favs.contains_url("https://x/g1.gif"));
        // В ленте тот же GIF известен только по адресу: повторное нажатие убирает его.
        assert!(!favs.toggle_url("https://x/g1.gif", None).unwrap());
        assert!(favs.items().is_empty());
        assert!(
            favs.toggle_url("https://x/new.gif", Some([320, 180]))
                .unwrap()
        );
        assert_eq!(favs.items()[0].width, 320);
        assert_eq!(favs.items()[0].preview_url, "https://x/new.gif");
    }

    #[test]
    fn broken_file_is_an_empty_collection() {
        let path = temp_path("broken");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ nope").unwrap();
        let mut favs = GifFavorites::with_path(path);
        favs.select("k");
        assert!(favs.items().is_empty());
        assert!(favs.toggle(&gif("1")).unwrap());
    }

    #[test]
    fn collection_is_capped() {
        let mut favs = GifFavorites::with_path(temp_path("cap"));
        favs.select("k");
        for n in 0..=MAX_FAVORITES {
            favs.toggle(&gif(&n.to_string())).unwrap();
        }
        assert_eq!(favs.items().len(), MAX_FAVORITES);
        assert!(!favs.contains("0"));
    }
}
