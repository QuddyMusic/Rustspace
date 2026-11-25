//! `.desktop`-файл для запуска без установки пакета.
//!
//! Порталы XDG (глобальные клавиши и др.) принимают идентификатор
//! приложения, только если в каталогах данных есть `<APP_ID>.desktop`.
//! Пакеты (deb, AppImage) кладут его сами; при запуске собранного бинарника
//! его нет, и портал отвечает «App info not found». Тогда клиент кладёт
//! файл и значок в `~/.local/share` и указывает в нём путь к себе.

use std::path::{Path, PathBuf};

use crate::APP_ID;

/// Значок для `Icon=` (тот же, что в окне).
const ICON: &[u8] = include_bytes!("../../../packaging/linux/rustspace.png");
/// Имя значка в теме hicolor.
const ICON_NAME: &str = "rustspace";

/// Кладёт `.desktop`-файл, если его нет ни в одном каталоге данных или
/// наш собственный указывает на другой бинарник.
pub(crate) fn ensure() {
    if std::env::var_os("FLATPAK_ID").is_some() || std::env::var_os("APPIMAGE").is_some() {
        return;
    }
    let Some(home) = data_home() else {
        return;
    };
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let file_name = format!("{APP_ID}.desktop");
    let ours = home.join("applications").join(&file_name);
    let installed_elsewhere = data_dirs()
        .iter()
        .any(|dir| dir.join("applications").join(&file_name).exists());
    if installed_elsewhere && !ours.exists() {
        return;
    }
    let entry = entry(&exe);
    if crate::files::read_to_string(&ours).is_ok_and(|current| current == entry) {
        return;
    }
    if let Err(err) = write(&home, &ours, &entry) {
        tracing::debug!(%err, "не удалось записать .desktop-файл");
    } else {
        tracing::info!(path = %ours.display(), ".desktop-файл записан (нужен порталам)");
    }
}

fn write(home: &Path, ours: &Path, entry: &str) -> std::io::Result<()> {
    if let Some(dir) = ours.parent() {
        crate::files::create_dir_all(dir)?;
    }
    let icon_dir = home.join("icons/hicolor/256x256/apps");
    crate::files::create_dir_all(&icon_dir)?;
    let icon = icon_dir.join(format!("{ICON_NAME}.png"));
    if !icon.exists() {
        crate::files::write(icon, ICON)?;
    }
    crate::files::write(ours, entry)
}

/// Содержимое `.desktop`-файла для бинарника `exe`.
fn entry(exe: &Path) -> String {
    let exec = exe.display().to_string();
    // Пробелы и кавычки в пути: спецификация требует кавычки.
    let exec = if exec.contains([' ', '"', '\\', '\'']) {
        format!("\"{}\"", exec.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        exec
    };
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Rustspace\n\
         Comment=Unofficial Rust client compatible with Backspace\n\
         Comment[ru]=Неофициальный клиент на Rust, совместимый с Backspace\n\
         Exec={exec}\n\
         Icon={ICON_NAME}\n\
         Terminal=false\n\
         Categories=Network;InstantMessaging;Chat;\n\
         StartupWMClass={APP_ID}\n\
         X-Rustspace-Generated=true\n"
    )
}

fn data_home() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}

/// Системные каталоги данных (`XDG_DATA_DIRS`).
fn data_dirs() -> Vec<PathBuf> {
    std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned())
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_points_at_binary() {
        let entry = entry(Path::new("/home/me/bs/target/release/rustspace"));
        assert!(entry.contains("Exec=/home/me/bs/target/release/rustspace\n"));
        assert!(entry.contains(&format!("StartupWMClass={APP_ID}\n")));
        let spaced = super::entry(Path::new("/opt/my apps/rustspace"));
        assert!(spaced.contains("Exec=\"/opt/my apps/rustspace\"\n"));
    }
}
