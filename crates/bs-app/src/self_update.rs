//! Обновление клиента изнутри: скачать релиз, проверить, поставить,
//! перезапуститься.
//!
//! Как ставить, зависит от того, как клиент установлен:
//!
//! - Linux, архив или `~/.local` (файл доступен на запись) — из архива
//!   `rustspace-linux-<arch>.tar.gz` берётся новый `rustspace` и атомарно
//!   подменяет текущий файл (переименование работает и у запущенного);
//! - Linux, AppImage (`$APPIMAGE`) — подменяется сам файл AppImage;
//! - Linux, пакет (`/usr/bin`) — обновляет пакетный менеджер, клиент только
//!   подсказывает команду;
//! - Windows — скачивается установщик и запускается в тихом режиме; он
//!   закрывает старую версию и запускает новую;
//! - macOS — архив с `Rustspace.app` распаковывается рядом и подменяет пакет.
//!
//! Каждый файл сверяется с `<файл>.sha256` из того же релиза; без суммы
//! обновление не ставится.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use sha2::Digest as _;

use crate::i18n::tr;
use crate::updates::Asset;

/// Как обновлять этот экземпляр клиента.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Method {
    /// Подменить исполняемый файл из архива `.tar.gz`.
    Tarball(PathBuf),
    /// Подменить AppImage.
    AppImage(PathBuf),
    /// Запустить установщик Windows.
    Installer,
    /// Подменить пакет `.app` на macOS.
    MacBundle(PathBuf),
    /// Только вручную: почему.
    Manual(String),
}

/// Состояние обновления.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Progress {
    /// Ничего не делаем.
    #[default]
    Idle,
    /// Качаем: сколько получено и всего (если известно).
    Downloading(u64, Option<u64>),
    /// Ставим.
    Installing,
    /// Готово — перезапустить, чтобы перейти на версию.
    Ready(String),
    /// Ошибка.
    Failed(String),
}

static PROGRESS: Mutex<Progress> = Mutex::new(Progress::Idle);

fn progress_lock() -> MutexGuard<'static, Progress> {
    PROGRESS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Текущее состояние.
pub(crate) fn progress() -> Progress {
    progress_lock().clone()
}

fn set(ctx: &egui::Context, state: Progress) {
    *progress_lock() = state;
    ctx.request_repaint();
}

/// Можно ли писать в каталог файла.
fn writable_dir(path: &Path) -> bool {
    let Some(dir) = path.parent() else {
        return false;
    };
    let probe = dir.join(format!(".rustspace-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Способ обновления этой установки.
#[must_use]
pub(crate) fn method() -> Method {
    let Ok(exe) = std::env::current_exe() else {
        return Method::Manual(tr("не удалось найти файл программы").to_owned());
    };
    if cfg!(target_os = "windows") {
        return Method::Installer;
    }
    if cfg!(target_os = "macos") {
        // …/Rustspace.app/Contents/MacOS/rustspace
        let bundle = exe
            .ancestors()
            .nth(3)
            .filter(|p| p.extension().is_some_and(|e| e == "app"))
            .map(Path::to_path_buf);
        return match bundle {
            Some(bundle) if writable_dir(&bundle) => Method::MacBundle(bundle),
            _ => Method::Manual(tr("нет прав на запись в папку программы").to_owned()),
        };
    }
    if let Some(image) = std::env::var_os("APPIMAGE").map(PathBuf::from) {
        return if writable_dir(&image) {
            Method::AppImage(image)
        } else {
            Method::Manual(tr("нет прав на запись в папку AppImage").to_owned())
        };
    }
    if exe.starts_with("/usr") || !writable_dir(&exe) {
        return Method::Manual(
            tr("клиент установлен пакетом — обновите его менеджером пакетов (pacman, apt)")
                .to_owned(),
        );
    }
    Method::Tarball(exe)
}

/// Файл релиза для этого способа и архитектуры.
#[must_use]
pub(crate) fn asset_for<'a>(method: &Method, assets: &'a [Asset]) -> Option<&'a Asset> {
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let wanted = |name: &str| -> bool {
        let name = name.to_lowercase();
        if name.contains("server") || !name.contains(arch) {
            return false;
        }
        let ext = std::path::Path::new(&name)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default();
        match method {
            Method::Tarball(_) => name.contains("linux") && name.contains(".tar.") && ext == "gz",
            Method::AppImage(_) => ext == "appimage",
            Method::Installer => name.contains("setup") && ext == "exe",
            Method::MacBundle(_) => name.contains("macos") && ext == "zip",
            Method::Manual(_) => false,
        }
    };
    assets.iter().find(|asset| wanted(&asset.name))
}

/// Начинает обновление до `version` (в фоне).
pub(crate) fn start(
    runtime: &tokio::runtime::Handle,
    ctx: &egui::Context,
    version: String,
    assets: &[Asset],
) {
    if matches!(progress(), Progress::Downloading(..) | Progress::Installing) {
        return;
    }
    let method = method();
    if let Method::Manual(why) = &method {
        set(ctx, Progress::Failed(why.clone()));
        return;
    }
    let Some(asset) = asset_for(&method, assets).cloned() else {
        set(
            ctx,
            Progress::Failed(tr("в релизе нет файла для этой системы").to_owned()),
        );
        return;
    };
    let checksum = assets
        .iter()
        .find(|a| a.name == format!("{}.sha256", asset.name))
        .cloned();
    let ctx = ctx.clone();
    set(&ctx, Progress::Downloading(0, None));
    runtime.spawn(async move {
        let result = run(&ctx, &method, &asset, checksum.as_ref()).await;
        set(
            &ctx,
            match result {
                Ok(()) => Progress::Ready(version),
                Err(err) => {
                    tracing::warn!(%err, "обновление не поставилось");
                    Progress::Failed(err)
                }
            },
        );
    });
}

async fn run(
    ctx: &egui::Context,
    method: &Method,
    asset: &Asset,
    checksum: Option<&Asset>,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("rustspace/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let Some(checksum) = checksum else {
        return Err(tr("в релизе нет контрольной суммы — обновите вручную").to_owned());
    };
    let expected = client
        .get(&checksum.url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let expected = expected
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let bytes = download(ctx, &client, &asset.url).await?;
    let actual = hex(&sha2::Sha256::digest(&bytes));
    if actual != expected {
        return Err(tr("контрольная сумма не совпала — файл повреждён").to_owned());
    }
    set(ctx, Progress::Installing);
    let method = method.clone();
    tokio::task::spawn_blocking(move || install(&method, &bytes))
        .await
        .map_err(|e| e.to_string())?
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

async fn download(
    ctx: &egui::Context,
    client: &reqwest::Client,
    url: &str,
) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.to_string())?;
    let total = response.content_length();
    let mut bytes = Vec::with_capacity(usize::try_from(total.unwrap_or(0)).unwrap_or(0));
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        bytes.extend_from_slice(&chunk);
        set(ctx, Progress::Downloading(bytes.len() as u64, total));
    }
    Ok(bytes)
}

/// Кладёт `data` рядом с `target` и атомарно подменяет его.
fn replace_file(target: &Path, data: &[u8]) -> Result<(), String> {
    let temp = target.with_file_name(format!(
        ".{}.new",
        target
            .file_name()
            .map_or_else(|| "rustspace".into(), |n| n.to_string_lossy())
    ));
    std::fs::write(&temp, data).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    std::fs::rename(&temp, target).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        e.to_string()
    })
}

/// Достаёт `rustspace` из архива релиза.
fn binary_from_tarball(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(data));
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let is_binary = entry
            .path()
            .ok()
            .and_then(|p| p.file_name().map(|n| n == "rustspace"))
            .unwrap_or(false);
        if is_binary && entry.header().entry_type().is_file() {
            let mut out = Vec::new();
            entry.read_to_end(&mut out).map_err(|e| e.to_string())?;
            return Ok(out);
        }
    }
    Err(tr("в архиве нет файла rustspace").to_owned())
}

fn install(method: &Method, data: &[u8]) -> Result<(), String> {
    match method {
        Method::Tarball(exe) => replace_file(exe, &binary_from_tarball(data)?),
        Method::AppImage(image) => replace_file(image, data),
        Method::Installer => {
            let path = std::env::temp_dir().join("rustspace-setup.exe");
            std::fs::write(&path, data).map_err(|e| e.to_string())?;
            *INSTALLER.lock().unwrap_or_else(PoisonError::into_inner) = Some(path);
            Ok(())
        }
        Method::MacBundle(bundle) => {
            let parent = bundle.parent().ok_or("нет папки")?;
            let staging = parent.join(".rustspace-update");
            let _ = std::fs::remove_dir_all(&staging);
            std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
            let zip = staging.join("update.zip");
            std::fs::write(&zip, data).map_err(|e| e.to_string())?;
            let status = std::process::Command::new("ditto")
                .args(["-x", "-k"])
                .arg(&zip)
                .arg(&staging)
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(tr("не удалось распаковать архив").to_owned());
            }
            let fresh = staging.join("Rustspace.app");
            let old = parent.join(".Rustspace.app.old");
            let _ = std::fs::remove_dir_all(&old);
            std::fs::rename(bundle, &old).map_err(|e| e.to_string())?;
            if let Err(err) = std::fs::rename(&fresh, bundle) {
                let _ = std::fs::rename(&old, bundle);
                return Err(err.to_string());
            }
            let _ = std::fs::remove_dir_all(&old);
            let _ = std::fs::remove_dir_all(&staging);
            Ok(())
        }
        Method::Manual(why) => Err(why.clone()),
    }
}

/// Скачанный установщик Windows: запускается при перезапуске.
static INSTALLER: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Запускает установщик или новую версию.
fn spawn_next(installer: Option<PathBuf>) -> std::io::Result<std::process::Child> {
    if let Some(setup) = installer {
        // Тихая установка закрывает эту версию и запускает новую.
        return std::process::Command::new(setup)
            .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
            .spawn();
    }
    let exe = std::env::current_exe()?;
    if cfg!(target_os = "macos") {
        let bundle = exe
            .ancestors()
            .nth(3)
            .ok_or_else(|| std::io::Error::other("bundle"))?;
        return std::process::Command::new("open")
            .arg("-n")
            .arg(bundle)
            .spawn();
    }
    let target = std::env::var_os("APPIMAGE").map_or(exe, PathBuf::from);
    std::process::Command::new(target)
        .args(std::env::args_os().skip(1))
        .spawn()
}

/// Перезапуск на новую версию: сохранить настройки, запустить новую
/// (или установщик) и выйти.
pub(crate) fn restart() {
    crate::prefs::flush(true);
    let installer = INSTALLER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    let spawned = spawn_next(installer);
    match spawned {
        Ok(_) => std::process::exit(0),
        Err(err) => tracing::warn!(%err, "не удалось перезапуститься"),
    }
}

#[cfg(test)]
mod tests {
    use super::{Method, asset_for, binary_from_tarball};
    use crate::updates::Asset;

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.to_owned(),
            url: format!("https://dl/{name}"),
        }
    }

    #[test]
    fn picks_the_file_for_the_method() {
        let assets = [
            asset("rustspace-server-linux-x86_64.tar.gz"),
            asset("rustspace-linux-x86_64.tar.gz"),
            asset("rustspace-linux-x86_64.tar.gz.sha256"),
            asset("rustspace-linux-aarch64.tar.gz"),
            asset("Rustspace-x86_64.AppImage"),
            asset("rustspace-setup-x86_64.exe"),
            asset("rustspace-windows-x86_64.zip"),
        ];
        if cfg!(target_arch = "x86_64") {
            let tar = asset_for(&Method::Tarball("/x".into()), &assets).map(|a| a.name.as_str());
            assert_eq!(tar, Some("rustspace-linux-x86_64.tar.gz"));
            let image = asset_for(&Method::AppImage("/x".into()), &assets).map(|a| a.name.as_str());
            assert_eq!(image, Some("Rustspace-x86_64.AppImage"));
            let setup = asset_for(&Method::Installer, &assets).map(|a| a.name.as_str());
            assert_eq!(setup, Some("rustspace-setup-x86_64.exe"));
        }
        assert!(asset_for(&Method::Manual(String::new()), &assets).is_none());
    }

    #[test]
    fn binary_comes_out_of_the_release_archive() {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (name, body) in [
            ("rustspace-linux-x86_64/README.md", b"readme".as_slice()),
            (
                "rustspace-linux-x86_64/rustspace",
                b"\x7fELF new".as_slice(),
            ),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, name, body).expect("tar");
        }
        let data = builder.into_inner().expect("tar").finish().expect("gz");
        assert_eq!(binary_from_tarball(&data).expect("binary"), b"\x7fELF new");
    }
}
