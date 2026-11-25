//! Переводчик на этом компьютере: llama.cpp и открытая модель.
//!
//! Всё ставится из настроек: сборка `llama-server` берётся из выпусков
//! llama.cpp на GitHub (MIT), модель в формате GGUF — с Hugging Face
//! (Qwen, Apache-2.0). Клиент сам запускает сервер, когда нужен перевод,
//! и гасит его после простоя и при выходе. Никаких ключей и сторонних
//! сервисов: текст не покидает компьютер.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;
use web_time::Instant;

use serde::{Deserialize, Serialize};

use crate::i18n::tr;

/// Выпуски llama.cpp.
/// Последние выпуски: в свежем сборка под нужную систему бывает ещё не готова.
const RELEASES: &str = "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=15";
/// Сервер гаснет после такого простоя.
const IDLE: Duration = Duration::from_secs(10 * 60);
/// Сколько ждать, пока модель загрузится в память.
const START_TIMEOUT: Duration = Duration::from_secs(120);

/// Модель на выбор: все — с открытой лицензией Apache-2.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModelChoice {
    pub(crate) id: &'static str,
    /// Репозиторий GGUF на Hugging Face.
    pub(crate) repo: &'static str,
    /// Примерный размер файла.
    pub(crate) size: &'static str,
}

/// Модели: быстрая, сбалансированная (по умолчанию), точная.
pub(crate) const MODELS: [ModelChoice; 3] = [
    ModelChoice {
        id: "qwen2.5-1.5b",
        repo: "Qwen/Qwen2.5-1.5B-Instruct-GGUF",
        size: "1.1",
    },
    ModelChoice {
        id: "qwen3-4b",
        repo: "lmstudio-community/Qwen3-4B-Instruct-2507-GGUF",
        size: "2.5",
    },
    ModelChoice {
        id: "qwen2.5-7b",
        repo: "bartowski/Qwen2.5-7B-Instruct-GGUF",
        size: "4.7",
    },
];

impl ModelChoice {
    /// Название для настроек.
    pub(crate) fn title(self) -> &'static str {
        match self.id {
            "qwen2.5-1.5b" => tr("Быстрая — Qwen2.5 1.5B"),
            "qwen2.5-7b" => tr("Точная — Qwen2.5 7B (лучше с видеокартой)"),
            _ => tr("Сбалансированная — Qwen3 4B"),
        }
    }

    /// Выбор по id (неизвестный — по умолчанию).
    pub(crate) fn by_id(id: &str) -> Self {
        MODELS
            .iter()
            .copied()
            .find(|m| m.id == id)
            .unwrap_or(MODELS[1])
    }
}

/// Настройки локального переводчика.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct LocalPrefs {
    /// Модель (id из [`MODELS`]).
    pub(crate) model: String,
    /// Считать на видеокарте (Vulkan, на macOS — Metal).
    pub(crate) gpu: bool,
}

impl Default for LocalPrefs {
    fn default() -> Self {
        Self {
            model: MODELS[1].id.to_owned(),
            gpu: gpu_available(),
        }
    }
}

/// Есть ли Vulkan (для сборки llama.cpp под видеокарту).
pub(crate) fn gpu_available() -> bool {
    if cfg!(target_os = "macos") {
        return true;
    }
    let candidates: &[&str] = if cfg!(windows) {
        &["C:\\Windows\\System32\\vulkan-1.dll"]
    } else {
        &[
            "/usr/lib/libvulkan.so.1",
            "/usr/lib64/libvulkan.so.1",
            "/usr/lib/x86_64-linux-gnu/libvulkan.so.1",
            "/usr/lib/aarch64-linux-gnu/libvulkan.so.1",
        ]
    };
    candidates.iter().any(|p| Path::new(p).exists())
}

/// Каталог: `engine/` (llama.cpp) и `models/`.
fn root() -> Option<PathBuf> {
    crate::saved_session::config_dir().map(|d| d.join("llama"))
}

fn server_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// `llama-server` из пакетов системы (`PATH`), если есть.
fn system_server() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(server_name()))
        .find(|p| p.is_file())
}

/// Найти файл по имени внутри каталога (архивы кладут его по-разному).
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == name) {
                return Some(path);
            }
        }
    }
    None
}

/// Что установлено.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Installed {
    /// Версия llama.cpp (тег выпуска), если стоит.
    pub(crate) engine: Option<String>,
    /// Сборка под видеокарту.
    pub(crate) gpu: bool,
    /// llama.cpp из пакетов системы или сборка без нужного ускорения: выбор
    /// GPU/CPU её не переустанавливает.
    pub(crate) fixed: bool,
    /// Файл модели, если скачан.
    pub(crate) model: Option<PathBuf>,
}

/// Что лежит на диске для этой модели.
pub(crate) fn installed(model: &str) -> Installed {
    let Some(root) = root() else {
        return Installed::default();
    };
    let engine_dir = root.join("engine");
    let tag = crate::files::read_to_string(engine_dir.join("VERSION")).ok();
    let engine = tag.filter(|_| find_file(&engine_dir, server_name()).is_some());
    let gpu = engine_dir.join("GPU").exists();
    let model = model_file(&root, model).filter(|p| p.exists());
    match engine {
        Some(tag) => Installed {
            engine: Some(tag.trim().to_owned()),
            gpu,
            fixed: engine_dir.join("CPU_FALLBACK").exists(),
            model,
        },
        None => Installed {
            engine: system_server().map(|_| tr("из системы").to_owned()),
            gpu: false,
            fixed: true,
            model,
        },
    }
}

fn model_file(root: &Path, model: &str) -> Option<PathBuf> {
    let dir = root.join("models").join(model);
    std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("gguf"))
        })
}

// ─── Установка ──────────────────────────────────────────────────────────────

/// Ход установки.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Progress {
    /// Ничего не делается.
    Idle,
    /// Ищем выпуск или файл модели.
    Looking(&'static str),
    /// Качаем: что, сколько, из скольки.
    Downloading(&'static str, u64, Option<u64>),
    /// Распаковываем.
    Unpacking,
    /// Готово.
    Done,
    /// Не вышло.
    Failed(String),
}

fn progress() -> MutexGuard<'static, Progress> {
    static PROGRESS: OnceLock<Mutex<Progress>> = OnceLock::new();
    PROGRESS
        .get_or_init(|| Mutex::new(Progress::Idle))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Текущий ход установки.
pub(crate) fn status() -> Progress {
    progress().clone()
}

fn set(ctx: &egui::Context, state: Progress) {
    *progress() = state;
    ctx.request_repaint();
}

/// Окончания имён архивов llama.cpp для системы, от лучшего к запасному:
/// сборка под видеокарту (если просили), затем обычная; `.tar.gz` и `.zip`
/// (Linux-сборки раньше выходили в zip).
pub(crate) fn asset_candidates(os: &str, arch: &str, gpu: bool) -> Vec<String> {
    let arch = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    match os {
        "linux" => {
            if gpu {
                out.push(format!("bin-ubuntu-vulkan-{arch}.tar.gz"));
                out.push(format!("bin-ubuntu-vulkan-{arch}.zip"));
            }
            out.push(format!("bin-ubuntu-{arch}.tar.gz"));
            out.push(format!("bin-ubuntu-{arch}.zip"));
        }
        "windows" => {
            if gpu {
                out.push(format!("bin-win-vulkan-{arch}.zip"));
            }
            out.push(format!("bin-win-cpu-{arch}.zip"));
        }
        "macos" => {
            out.push(format!("bin-macos-{arch}.tar.gz"));
            out.push(format!("bin-macos-{arch}.zip"));
        }
        _ => {}
    }
    out
}

/// Найденный архив llama.cpp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Asset {
    /// Тег выпуска.
    pub(crate) tag: String,
    /// Адрес архива.
    pub(crate) url: String,
    /// Окончание имени (по нему — вид архива и сборка под видеокарту).
    pub(crate) suffix: String,
}

/// Лучший архив из списка выпусков GitHub (новые первыми): сначала по
/// предпочтению сборки, затем по свежести выпуска.
pub(crate) fn pick_asset(releases: &serde_json::Value, candidates: &[String]) -> Option<Asset> {
    let releases: Vec<&serde_json::Value> = match releases {
        serde_json::Value::Array(list) => list.iter().collect(),
        one => vec![one],
    };
    candidates.iter().find_map(|suffix| {
        releases.iter().find_map(|release| {
            if release.get("draft").and_then(serde_json::Value::as_bool) == Some(true) {
                return None;
            }
            let tag = release.get("tag_name")?.as_str()?;
            release.get("assets")?.as_array()?.iter().find_map(|asset| {
                let name = asset.get("name")?.as_str()?;
                if !(name.starts_with("llama-") && name.ends_with(suffix.as_str())) {
                    return None;
                }
                Some(Asset {
                    tag: tag.to_owned(),
                    url: asset.get("browser_download_url")?.as_str()?.to_owned(),
                    suffix: suffix.clone(),
                })
            })
        })
    })
}

/// Файл модели в репозитории: GGUF с квантованием Q4_K_M.
pub(crate) fn pick_model_file(model: &serde_json::Value) -> Option<String> {
    model
        .get("siblings")?
        .as_array()?
        .iter()
        .filter_map(|s| s.get("rfilename")?.as_str())
        .find(|name| {
            let lower = name.to_lowercase();
            Path::new(&lower).extension().is_some_and(|x| x == "gguf")
                && lower.contains("q4_k_m")
                && !lower.contains("-of-")
        })
        .map(str::to_owned)
}

fn http() -> Result<reqwest::Client, String> {
    crate::sys::http(None).build().map_err(|e| e.to_string())
}

/// Скачать в файл (через `.part`), сообщая ход.
#[cfg(not(target_arch = "wasm32"))]
async fn download_to(
    ctx: &egui::Context,
    client: &reqwest::Client,
    what: &'static str,
    url: &str,
    target: &Path,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt as _;
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.to_string())?;
    let total = response.content_length();
    let part = target.with_extension("part");
    let mut file = tokio::fs::File::create(&part)
        .await
        .map_err(|e| e.to_string())?;
    let mut got = 0u64;
    let mut shown = Instant::now();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        got += chunk.len() as u64;
        if shown.elapsed() > Duration::from_millis(150) {
            shown = Instant::now();
            set(ctx, Progress::Downloading(what, got, total));
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    tokio::fs::rename(&part, target)
        .await
        .map_err(|e| e.to_string())
}

/// В браузере ничего не скачивается на диск.
#[cfg(target_arch = "wasm32")]
async fn download_to(
    _ctx: &egui::Context,
    _client: &reqwest::Client,
    _what: &'static str,
    _url: &str,
    _target: &Path,
) -> Result<(), String> {
    Err(tr("локальная модель недоступна в браузере").to_owned())
}

/// В браузере архивы не распаковываются.
#[cfg(target_arch = "wasm32")]
pub(crate) fn unpack(_archive: &Path, _dir: &Path) -> Result<(), String> {
    Err(tr("локальная модель недоступна в браузере").to_owned())
}

/// Распаковать архив llama.cpp в `dir`, убрав верхний каталог.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn unpack(archive: &Path, dir: &Path) -> Result<(), String> {
    let _ = crate::files::remove_dir_all(dir);
    crate::files::create_dir_all(dir).map_err(|e| e.to_string())?;
    let is_zip = archive
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("zip"));
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    if is_zip {
        let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
            let Some(path) = entry.enclosed_name() else {
                continue;
            };
            let target = dir.join(path);
            if entry.is_dir() {
                crate::files::create_dir_all(&target).map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(parent) = target.parent() {
                crate::files::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = std::fs::File::create(&target).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        }
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        tar.set_preserve_permissions(true);
        for entry in tar.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path().map_err(|e| e.to_string())?.into_owned();
            // `llama-b1234/llama-server` → `llama-server`; пути с `..` — мимо.
            let inner: PathBuf = path.components().skip(1).collect();
            if inner.as_os_str().is_empty()
                || inner
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                continue;
            }
            entry.unpack(dir.join(inner)).map_err(|e| e.to_string())?;
        }
    }
    let server =
        find_file(dir, server_name()).ok_or_else(|| tr("в архиве нет llama-server").to_owned())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755));
    }
    let _ = server;
    Ok(())
}

async fn install_engine(
    ctx: &egui::Context,
    client: &reqwest::Client,
    root: &Path,
    gpu: bool,
) -> Result<(), String> {
    set(ctx, Progress::Looking("llama.cpp"));
    let candidates = asset_candidates(std::env::consts::OS, std::env::consts::ARCH, gpu);
    if candidates.is_empty() {
        return Err(tr("для этой системы нет готовой сборки llama.cpp").to_owned());
    }
    let releases: serde_json::Value = client
        .get(RELEASES)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let Asset { tag, url, suffix } = pick_asset(&releases, &candidates).ok_or_else(|| {
        crate::tf!(
            "в последних выпусках llama.cpp нет архива {name}; поставьте llama.cpp из пакетов системы — Rustspace возьмёт его llama-server",
            name = candidates[0].clone()
        )
    })?;
    crate::files::create_dir_all(root).map_err(|e| e.to_string())?;
    let archive = root.join(
        if Path::new(&suffix).extension().is_some_and(|x| x == "zip") {
            "engine.zip"
        } else {
            "engine.tar.gz"
        },
    );
    download_to(ctx, client, "llama.cpp", &url, &archive).await?;
    set(ctx, Progress::Unpacking);
    let dir = root.join("engine");
    let (archive2, dir2) = (archive.clone(), dir.clone());
    bs_rt::spawn_blocking(move || unpack(&archive2, &dir2))
        .await
        .map_err(|e| e.to_string())??;
    let _ = crate::files::remove_file(&archive);
    crate::files::write(dir.join("VERSION"), &tag).map_err(|e| e.to_string())?;
    if suffix.contains("vulkan") {
        let _ = crate::files::write(dir.join("GPU"), "1");
    } else if gpu {
        // Под видеокарту сборки не нашлось: стоит обычная, без повторной
        // установки при каждом запуске.
        let _ = crate::files::write(dir.join("CPU_FALLBACK"), "1");
    }
    Ok(())
}

async fn install_model(
    ctx: &egui::Context,
    client: &reqwest::Client,
    root: &Path,
    choice: ModelChoice,
) -> Result<(), String> {
    set(ctx, Progress::Looking("model"));
    let info: serde_json::Value = client
        .get(format!("https://huggingface.co/api/models/{}", choice.repo))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let file = pick_model_file(&info)
        .ok_or_else(|| tr("в репозитории модели нет файла Q4_K_M").to_owned())?;
    let dir = root.join("models").join(choice.id);
    let _ = crate::files::remove_dir_all(&dir);
    crate::files::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let url = format!("https://huggingface.co/{}/resolve/main/{file}", choice.repo);
    let name = Path::new(&file)
        .file_name()
        .map_or_else(|| "model.gguf".into(), |n| n.to_string_lossy().into_owned());
    download_to(ctx, client, "model", &url, &dir.join(name)).await
}

/// Поставить недостающее: llama.cpp (нужной сборки) и выбранную модель.
pub(crate) fn install(runtime: &crate::rt::Handle, ctx: &egui::Context, prefs: &LocalPrefs) {
    if matches!(
        status(),
        Progress::Looking(_) | Progress::Downloading(..) | Progress::Unpacking
    ) {
        return;
    }
    let (ctx, prefs) = (ctx.clone(), prefs.clone());
    runtime.spawn(async move {
        let result = async {
            let root = root().ok_or_else(|| tr("нет каталога настроек").to_owned())?;
            let client = http()?;
            let have = installed(&prefs.model);
            if have.engine.is_none() || (!have.fixed && have.gpu != prefs.gpu) {
                stop();
                install_engine(&ctx, &client, &root, prefs.gpu).await?;
            }
            if have.model.is_none() {
                install_model(&ctx, &client, &root, ModelChoice::by_id(&prefs.model)).await?;
            }
            Ok::<(), String>(())
        }
        .await;
        set(
            &ctx,
            result.map_or_else(Progress::Failed, |()| Progress::Done),
        );
    });
}

/// Удалить всё скачанное.
pub(crate) fn remove() {
    stop();
    if let Some(root) = root() {
        let _ = crate::files::remove_dir_all(root);
    }
    *progress() = Progress::Idle;
}

// ─── Сервер ─────────────────────────────────────────────────────────────────

struct Running {
    child: Child,
    port: u16,
    model: PathBuf,
    used: Instant,
}

fn running() -> MutexGuard<'static, Option<Running>> {
    static RUNNING: OnceLock<Mutex<Option<Running>>> = OnceLock::new();
    RUNNING
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Погасить сервер (при выходе, смене модели, удалении).
pub(crate) fn stop() {
    let server = running().take();
    if let Some(mut server) = server {
        let _ = server.child.kill();
        let _ = server.child.wait();
    }
}

fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

fn spawn_server(
    server: &Path,
    model: &Path,
    port: u16,
    gpu: bool,
    bundled: bool,
) -> Result<Child, String> {
    let dir = server.parent().unwrap_or_else(|| Path::new("."));
    // Журнал — в своём каталоге: у llama-server из пакетов системы каталог
    // программы (/usr/bin) закрыт на запись.
    let log_path = root().map_or_else(
        || std::env::temp_dir().join("llama-server.log"),
        |r| r.join("llama-server.log"),
    );
    if let Some(parent) = log_path.parent() {
        let _ = crate::files::create_dir_all(parent);
    }
    let log = std::fs::File::create(&log_path).map_err(|e| {
        crate::tf!(
            "не удалось открыть журнал llama-server: {e}",
            e = e.to_string()
        )
    })?;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().clamp(2, 8));
    let mut command = Command::new(server);
    command
        .arg("-m")
        .arg(model)
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "-c",
            "4096",
        ])
        .args(["-t", &threads.to_string()])
        .args(["-ngl", if gpu { "99" } else { "0" }])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    // Своя сборка ищет библиотеки рядом с собой; системной это не нужно.
    if bundled {
        command.current_dir(dir);
        #[cfg(target_os = "linux")]
        command.env("LD_LIBRARY_PATH", dir);
    }
    command
        .spawn()
        .map_err(|e| crate::tf!("не удалось запустить llama-server: {e}", e = e.to_string()))
}

/// Порт сервера с этой моделью: уже работающего (`true`) или только что
/// запущенного (`false` — ещё грузит модель).
fn start_or_reuse(
    server: &Path,
    model: &Path,
    gpu: bool,
    bundled: bool,
) -> Result<(u16, bool), String> {
    let mut guard = running();
    if let Some(current) = guard.as_mut() {
        let alive = matches!(current.child.try_wait(), Ok(None));
        if alive && current.model == model {
            current.used = Instant::now();
            return Ok((current.port, true));
        }
        let _ = current.child.kill();
        let _ = current.child.wait();
        *guard = None;
    }
    let port = free_port()?;
    let child = spawn_server(server, model, port, gpu, bundled)?;
    *guard = Some(Running {
        child,
        port,
        model: model.to_path_buf(),
        used: Instant::now(),
    });
    drop(guard);
    watch_idle();
    Ok((port, false))
}

/// Адрес работающего сервера (запустит при необходимости и дождётся,
/// пока модель загрузится).
///
/// # Errors
/// Не установлено, не запустилось или не загрузилось.
pub(crate) async fn ensure(prefs: &LocalPrefs) -> Result<String, String> {
    let root = root().ok_or_else(|| tr("нет каталога настроек").to_owned())?;
    let have = installed(&prefs.model);
    let model = have.model.ok_or_else(|| {
        tr("локальная модель не установлена — установите её в настройках перевода").to_owned()
    })?;
    let bundled = find_file(&root.join("engine"), server_name());
    let is_bundled = bundled.is_some();
    let server = bundled.or_else(system_server).ok_or_else(|| {
        tr("llama.cpp не установлен — установите его в настройках перевода").to_owned()
    })?;
    // Системная сборка: видеокарта — как выбрано в настройках (у сборки без
    // неё llama.cpp просто посчитает на процессоре).
    let gpu = if is_bundled { have.gpu } else { prefs.gpu };
    let (port, ready) = start_or_reuse(&server, &model, gpu, is_bundled)?;
    if ready {
        return Ok(format!("http://127.0.0.1:{port}/v1"));
    }
    let health = format!("http://127.0.0.1:{port}/health");
    let client = http()?;
    let started = Instant::now();
    loop {
        if client
            .get(&health)
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            return Ok(format!("http://127.0.0.1:{port}/v1"));
        }
        let exited = running()
            .as_mut()
            .is_none_or(|r| !matches!(r.child.try_wait(), Ok(None)));
        if exited {
            stop();
            return Err(
                tr("llama-server завершился — подробности в llama/llama-server.log").to_owned(),
            );
        }
        if started.elapsed() > START_TIMEOUT {
            stop();
            return Err(tr("модель не загрузилась за две минуты").to_owned());
        }
        bs_rt::time::sleep(Duration::from_millis(400)).await;
    }
}

/// Сторож простоя: гасит сервер, если переводов давно не было.
fn watch_idle() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("llama-idle".into())
            .spawn(|| {
                loop {
                    std::thread::sleep(Duration::from_secs(60));
                    let idle = running().as_ref().is_some_and(|r| r.used.elapsed() > IDLE);
                    if idle {
                        tracing::info!("llama-server: долго без дела — останавливаю");
                        stop();
                    }
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assets_follow_the_release_naming() {
        assert_eq!(
            asset_candidates("linux", "x86_64", true),
            [
                "bin-ubuntu-vulkan-x64.tar.gz",
                "bin-ubuntu-vulkan-x64.zip",
                "bin-ubuntu-x64.tar.gz",
                "bin-ubuntu-x64.zip"
            ]
        );
        assert_eq!(
            asset_candidates("windows", "x86_64", false),
            ["bin-win-cpu-x64.zip"]
        );
        assert!(asset_candidates("linux", "riscv64", false).is_empty());
        // В свежем выпуске нет сборки под Vulkan: берём её из прошлого.
        let releases = serde_json::json!([
            {"tag_name": "b7001", "assets": [
                {"name": "llama-b7001-bin-ubuntu-x64.tar.gz", "browser_download_url": "https://x/c1"},
            ]},
            {"tag_name": "b7000", "assets": [
                {"name": "llama-b7000-bin-ubuntu-vulkan-x64.tar.gz", "browser_download_url": "https://x/v0"},
            ]},
        ]);
        let gpu = asset_candidates("linux", "x86_64", true);
        assert_eq!(
            pick_asset(&releases, &gpu).map(|a| (a.tag, a.url)),
            Some(("b7000".into(), "https://x/v0".into()))
        );
        let cpu = asset_candidates("linux", "x86_64", false);
        assert_eq!(
            pick_asset(&releases, &cpu).map(|a| a.tag),
            Some("b7001".into())
        );
        let none = serde_json::json!([{"tag_name": "b1", "assets": []}]);
        assert_eq!(pick_asset(&none, &cpu), None);
        let model = serde_json::json!({"siblings": [
            {"rfilename": "README.md"},
            {"rfilename": "qwen2.5-7b-instruct-q4_k_m-00001-of-00002.gguf"},
            {"rfilename": "Qwen2.5-7B-Instruct-Q4_K_M.gguf"},
        ]});
        assert_eq!(
            pick_model_file(&model).as_deref(),
            Some("Qwen2.5-7B-Instruct-Q4_K_M.gguf")
        );
    }

    #[test]
    fn archives_are_unpacked_without_their_top_folder() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let archive = dir.path().join("a.tar.gz");
        {
            let file = std::fs::File::create(&archive).unwrap_or_else(|_| unreachable!());
            let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
                file,
                flate2::Compression::fast(),
            ));
            for (name, body) in [
                ("llama-b1/llama-server", "#!/bin/sh\n"),
                ("llama-b1/libggml.so", "x"),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder
                    .append_data(&mut header, name, body.as_bytes())
                    .unwrap_or_default();
            }
            builder
                .into_inner()
                .and_then(flate2::write::GzEncoder::finish)
                .map(drop)
                .unwrap_or_default();
        }
        let out = dir.path().join("engine");
        unpack(&archive, &out).unwrap_or_default();
        assert!(out.join("llama-server").exists());
        assert!(out.join("libggml.so").exists());
    }
}
