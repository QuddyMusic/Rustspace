//! Сохранение вложений на диск: выбор места и безопасные имена файлов.
//!
//! Системные диалоги блокируют поток, поэтому открываются в отдельном потоке;
//! результат приходит по каналу, а сами файлы качает мост (см.
//! `BridgeRequest::Download`).

use crate::i18n::tr;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

/// Что сохраняем.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    /// Имя файла на сервере (значение поля вложения).
    pub(crate) value: String,
    /// Исходное имя файла, как его назвал автор.
    pub(crate) name: String,
}

/// Нажатая кнопка скачивания.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Click {
    /// Один файл: спросим имя и место.
    One(Target),
    /// Все файлы сообщения: спросим только папку.
    All(Vec<Target>),
}

/// Ожидаемый выбор пользователя: куда сохранить каждый файл (пусто — отмена).
pub(crate) type Pending = Receiver<Vec<(Target, PathBuf)>>;

/// Самая длинная часть имени, которую оставляем, символов.
const NAME_MAX: usize = 120;

/// Имя файла, безопасное для записи на диск: без каталогов и служебных символов.
pub(crate) fn safe_name(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let cleaned: String = last
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .take(NAME_MAX)
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_owned();
    if cleaned.is_empty() {
        tr("файл").to_owned()
    } else {
        cleaned
    }
}

/// Путь для файла `name` в `folder`, не занятый на диске и не выбранный ранее
/// (`taken`): при совпадении добавляется « (1)», « (2)»… перед расширением.
pub(crate) fn unique_path(folder: &Path, name: &str, taken: &mut HashSet<PathBuf>) -> PathBuf {
    let name = safe_name(name);
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name.as_str(), ""),
    };
    let mut candidate = folder.join(&name);
    let mut n = 1;
    while candidate.exists() || taken.contains(&candidate) {
        candidate = folder.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    taken.insert(candidate.clone());
    candidate
}

/// Открывает диалог в отдельном потоке и возвращает ожидание результата.
pub(crate) fn start(ctx: &egui::Context, click: Click) -> Pending {
    let (tx, rx) = channel();
    // В браузере файлы уходят в его загрузки: путь — только имя файла.
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
        let targets = match click {
            Click::One(target) => vec![target],
            Click::All(targets) => targets,
        };
        let picked = targets
            .into_iter()
            .map(|target| {
                let path = PathBuf::from(safe_name(&target.name));
                (target, path)
            })
            .collect();
        let _ = tx.send(picked);
        rx
    }
    #[cfg(not(target_arch = "wasm32"))]
    let ctx = ctx.clone();
    #[cfg(not(target_arch = "wasm32"))]
    crate::sys::background(move || {
        let picked = match click {
            Click::One(target) => crate::sys::FileDialog::new()
                .set_file_name(safe_name(&target.name))
                .save_file()
                .map(|path| vec![(target, path)])
                .unwrap_or_default(),
            Click::All(targets) => crate::sys::FileDialog::new()
                .pick_folder()
                .map(|folder| {
                    let mut taken = HashSet::new();
                    targets
                        .into_iter()
                        .map(|target| {
                            let path = unique_path(&folder, &target.name, &mut taken);
                            (target, path)
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
        let _ = tx.send(picked);
        ctx.request_repaint();
    });
    #[cfg(not(target_arch = "wasm32"))]
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_name_strips_directories_and_junk() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("C:\\Users\\a\\отчёт.pdf"), "отчёт.pdf");
        assert_eq!(safe_name("a:b*c?.txt"), "abc.txt");
        assert_eq!(safe_name("  ..  "), "файл");
        assert_eq!(safe_name(""), "файл");
        assert_eq!(safe_name("x\u{0}y\n.png"), "xy.png");
        assert_eq!(safe_name(&"я".repeat(500)).chars().count(), NAME_MAX);
    }

    #[test]
    fn unique_path_avoids_disk_and_batch_collisions() {
        let dir = std::env::temp_dir().join(format!("bs-uniq-{}", std::process::id()));
        crate::files::create_dir_all(&dir).unwrap();
        crate::files::write(dir.join("a.txt"), b"x").unwrap();
        let mut taken = HashSet::new();
        assert_eq!(
            unique_path(&dir, "a.txt", &mut taken),
            dir.join("a (1).txt")
        );
        assert_eq!(
            unique_path(&dir, "a.txt", &mut taken),
            dir.join("a (2).txt")
        );
        assert_eq!(unique_path(&dir, "b", &mut taken), dir.join("b"));
        assert_eq!(unique_path(&dir, "b", &mut taken), dir.join("b (1)"));
        assert_eq!(unique_path(&dir, ".hidden", &mut taken), dir.join("hidden"));
        let _ = crate::files::remove_dir_all(&dir);
    }
}
