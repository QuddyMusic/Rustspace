//! Windows: имя приложения для уведомлений и журнал в файл.
//!
//! Без своего AppUserModelID уведомления Windows приходят от имени
//! PowerShell (так их показывает notify-rust по умолчанию). Идентификатор
//! регистрируется в реестре пользователя с именем и значком; ярлык
//! установщика несёт тот же идентификатор.
//!
//! Окна консоли у программы нет, поэтому журнал пишется в файл рядом с
//! настройками (`rustspace.log`, перезаписывается при запуске).

use std::path::PathBuf;

/// Идентификатор для уведомлений и панели задач.
pub(crate) const AUMID: &str = "QuddyMusic.Rustspace";

/// Регистрирует идентификатор: имя «Rustspace» и значок.
pub(crate) fn register() {
    let icon = crate::saved_session::config_dir().map(|dir| dir.join("rustspace.png"));
    if let Some(icon) = &icon
        && !icon.exists()
    {
        let _ = crate::files::create_dir_all(icon.parent().unwrap_or(icon));
        let _ = crate::files::write(
            icon,
            include_bytes!("../../../packaging/linux/rustspace.png"),
        );
    }
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .create_subkey(format!("Software\\Classes\\AppUserModelId\\{AUMID}"));
    match key {
        Ok((key, _)) => {
            let _ = key.set_value("DisplayName", &crate::brand::NAME.to_owned());
            if let Some(icon) = icon {
                let _ = key.set_value("IconUri", &icon.to_string_lossy().into_owned());
            }
        }
        Err(err) => tracing::debug!(%err, "AppUserModelID не зарегистрирован"),
    }
}

/// Файл журнала.
pub(crate) fn log_file() -> Option<std::fs::File> {
    let dir: PathBuf = crate::saved_session::config_dir()?;
    crate::files::create_dir_all(&dir).ok()?;
    std::fs::File::create(dir.join("rustspace.log")).ok()
}
