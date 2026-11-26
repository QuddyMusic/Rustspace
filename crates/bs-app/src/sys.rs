//! Системные службы, которые в браузере устроены иначе или которых там нет:
//! открыть ссылку или файл, системный диалог выбора файла.

/// Открывает ссылку в браузере (в веб-версии — в новой вкладке).
///
/// # Errors
/// Система или браузер отказались открыть ссылку.
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        opener::open(url).map_err(|err| err.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let window = web_sys::window().ok_or_else(|| "нет окна браузера".to_owned())?;
        window
            .open_with_url_and_target(url, "_blank")
            .map(drop)
            .map_err(|err| format!("{err:?}"))
    }
}

/// Открывает файл на диске приложением по умолчанию.
///
/// # Errors
/// Система не смогла открыть файл (в браузере файлов на диске нет).
pub(crate) fn open_path(path: &std::path::Path) -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        opener::open(path).map_err(|err| err.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        Err(crate::i18n::tr("В браузере файл открывается из загрузок браузера").to_owned())
    }
}

/// Показывает файл в файловом менеджере.
///
/// # Errors
/// Система не смогла показать файл (в браузере файлов на диске нет).
pub(crate) fn reveal_path(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        reveal_linux(path)
    }
    #[cfg(not(any(target_os = "linux", target_arch = "wasm32")))]
    {
        opener::reveal(path).map_err(|err| err.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        Err(crate::i18n::tr("В браузере файл открывается из загрузок браузера").to_owned())
    }
}

/// Linux: файловый менеджер, который уже запущен и умеет
/// `org.freedesktop.FileManager1`, выделяет файл; иначе папка открывается
/// приложением по умолчанию (xdg-open). Вызов FileManager1 «на авось»
/// (через D-Bus-активацию) на Hyprland и других оболочках без такого
/// менеджера отвечал успехом, но ничего не показывал.
#[cfg(target_os = "linux")]
fn reveal_linux(path: &std::path::Path) -> Result<(), String> {
    let path = path.canonicalize().map_err(|err| err.to_string())?;
    #[cfg(any(feature = "presence", feature = "global-hotkeys"))]
    if show_items(&path) {
        return Ok(());
    }
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("/"));
    if has_folder_handler() {
        return opener::open(parent).map_err(|err| err.to_string());
    }
    // Для папок в mimeapps.list ничего не назначено (частый случай на
    // тайлинговых оболочках) — xdg-open тогда молча ничего не делает.
    for manager in FILE_MANAGERS {
        if std::process::Command::new(manager).arg(parent).spawn().is_ok() {
            return Ok(());
        }
    }
    opener::open(parent).map_err(|err| err.to_string())
}

/// Известные файловые менеджеры: пробуем по порядку, если система не
/// знает, чем открывать папки.
#[cfg(target_os = "linux")]
const FILE_MANAGERS: [&str; 8] = [
    "nautilus", "dolphin", "thunar", "nemo", "pcmanfm-qt", "pcmanfm", "caja", "spacefm",
];

/// Назначено ли приложение для `inode/directory`.
#[cfg(target_os = "linux")]
fn has_folder_handler() -> bool {
    std::process::Command::new("xdg-mime")
        .args(["query", "default", "inode/directory"])
        .output()
        .is_ok_and(|out| out.status.success() && !out.stdout.trim_ascii().is_empty())
}

/// `ShowItems` у работающего файлового менеджера.
#[cfg(all(target_os = "linux", any(feature = "presence", feature = "global-hotkeys")))]
fn show_items(path: &std::path::Path) -> bool {
    const NAME: &str = "org.freedesktop.FileManager1";
    let Ok(bus) = zbus::blocking::Connection::session() else {
        return false;
    };
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&bus) else {
        return false;
    };
    let running = NAME
        .try_into()
        .ok()
        .and_then(|name| dbus.name_has_owner(name).ok())
        .unwrap_or(false);
    let Some(uri) = running
        .then(|| url::Url::from_file_path(path).ok())
        .flatten()
    else {
        return false;
    };
    bus.call_method(
        Some(NAME),
        "/org/freedesktop/FileManager1",
        Some(NAME),
        "ShowItems",
        &(vec![uri.as_str()], ""),
    )
    .is_ok()
}

/// Сохраняет файл: на десктопе — по пути `dest`, в браузере — обычной
/// загрузкой браузера с именем из `dest`.
///
/// # Errors
/// Файл не записался.
pub(crate) async fn save_file(dest: &std::path::Path, bytes: Vec<u8>) -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::fs::write(dest, bytes)
            .await
            .map_err(|err| err.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let name = dest
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("download");
        browser_download(name, &bytes).map_err(|err| format!("{err:?}"))
    }
}

/// Отдаёт байты браузеру как скачанный файл.
#[cfg(target_arch = "wasm32")]
fn browser_download(name: &str, bytes: &[u8]) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast as _;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)?;
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("нет документа"))?;
    let link: web_sys::HtmlAnchorElement = document.create_element("a")?.dyn_into()?;
    link.set_href(&url);
    link.set_download(name);
    link.click();
    web_sys::Url::revoke_object_url(&url)?;
    Ok(())
}

/// HTTP-клиент для внешних сервисов: на десктопе со своим User-Agent и
/// сроками, в браузере их задаёт сам браузер.
pub(crate) fn http(timeout: Option<std::time::Duration>) -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();
    #[cfg(not(target_arch = "wasm32"))]
    let builder = {
        let builder = builder
            .user_agent(concat!("rustspace/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(20));
        match timeout {
            Some(timeout) => builder.timeout(timeout),
            None => builder,
        }
    };
    #[cfg(target_arch = "wasm32")]
    let _ = timeout;
    builder
}

/// Адрес сервера, который отдал страницу (только в веб-версии).
#[must_use]
pub(crate) fn page_origin() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()?.location().origin().ok()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// Файл, выбранный пользователем: имя и содержимое (или почему оно не
/// прочитано).
#[derive(Debug)]
pub(crate) struct PickedFile {
    pub(crate) name: String,
    pub(crate) bytes: Result<Vec<u8>, String>,
}

/// Что выбирать.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PickSpec {
    /// Подпись и расширения фильтра (`None` — любые файлы).
    pub(crate) filter: Option<(&'static str, &'static [&'static str])>,
    /// Несколько файлов разом.
    pub(crate) many: bool,
    /// Больше файл не читается.
    pub(crate) limit: u64,
}

impl PickSpec {
    /// Одна картинка не больше `limit` байт.
    pub(crate) const fn image(limit: u64) -> Self {
        Self {
            filter: Some(("Изображения", &["png", "jpg", "jpeg", "gif", "webp"])),
            many: false,
            limit,
        }
    }
}

/// Почему файл не прочитан, по размеру; `None` — размер подходит.
fn size_problem(size: u64, limit: u64) -> Option<String> {
    if size == 0 {
        return Some(crate::i18n::tr("Файл пустой").to_owned());
    }
    (size > limit).then(|| crate::tf!("Файл больше {a0} МБ", a0 = (limit / (1024 * 1024)).max(1)))
}

/// Читает файлы с диска (перетащенные на окно, выбранные в диалоге).
pub(crate) fn read_files(paths: Vec<std::path::PathBuf>, limit: u64) -> Vec<PickedFile> {
    paths
        .into_iter()
        .map(|path| {
            let name = path.file_name().map_or_else(
                || crate::i18n::tr("файл").to_owned(),
                |n| n.to_string_lossy().into_owned(),
            );
            let bytes = match std::fs::metadata(&path) {
                Ok(meta) => size_problem(meta.len(), limit).map_or_else(
                    || crate::files::read(&path).map_err(|err| err.to_string()),
                    Err,
                ),
                Err(err) => Err(err.to_string()),
            };
            PickedFile { name, bytes }
        })
        .collect()
}

/// Открывает выбор файлов и отдаёт прочитанное в `done` (пусто — выбор
/// закрыли). На десктопе — системный диалог в отдельном потоке, в браузере —
/// выбор файла страницы.
pub(crate) fn pick_files(
    ctx: &egui::Context,
    spec: PickSpec,
    done: impl FnOnce(Vec<PickedFile>) + Send + 'static,
) {
    let ctx = ctx.clone();
    #[cfg(not(target_arch = "wasm32"))]
    background(move || {
        let mut dialog = FileDialog::new();
        if let Some((name, extensions)) = spec.filter {
            dialog = dialog.add_filter(crate::i18n::tr(name), extensions);
        }
        let paths = if spec.many {
            dialog.pick_files().unwrap_or_default()
        } else {
            dialog.pick_file().into_iter().collect()
        };
        done(read_files(paths, spec.limit));
        ctx.request_repaint();
    });
    #[cfg(target_arch = "wasm32")]
    if let Err(err) = browser_pick(ctx, spec, Box::new(done)) {
        tracing::warn!(?err, "выбор файла в браузере не открылся");
    }
}

/// Ящик, куда придут выбранные файлы (для окон, которые ждут выбор
/// между кадрами).
pub(crate) type PickInbox = std::sync::mpsc::Receiver<Vec<PickedFile>>;

/// Открывает выбор файлов; результат — в возвращённом ящике.
pub(crate) fn pick_inbox(ctx: &egui::Context, spec: PickSpec) -> PickInbox {
    let (tx, rx) = std::sync::mpsc::channel();
    pick_files(ctx, spec, move |files| {
        let _ = tx.send(files);
    });
    rx
}

/// Что пришло в ящик: `Some` — выбор закончен (пусто — закрыли).
pub(crate) fn take_picked(inbox: &mut Option<PickInbox>) -> Option<Vec<PickedFile>> {
    let result = match inbox.as_ref()?.try_recv() {
        Ok(files) => files,
        Err(std::sync::mpsc::TryRecvError::Empty) => return None,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Vec::new(),
    };
    *inbox = None;
    Some(result)
}

/// Отдаёт браузеру файл как скачанный (только веб-версия).
///
/// # Errors
/// Браузер отказал.
#[cfg(target_arch = "wasm32")]
pub(crate) fn offer_download(name: &str, bytes: &[u8]) -> Result<(), String> {
    browser_download(name, bytes).map_err(|err| format!("{err:?}"))
}

/// `<input type="file">` страницы: по `change` читает файлы, по `cancel`
/// отдаёт пустой список.
#[cfg(target_arch = "wasm32")]
fn browser_pick(
    ctx: egui::Context,
    spec: PickSpec,
    done: Box<dyn FnOnce(Vec<PickedFile>)>,
) -> Result<(), wasm_bindgen::JsValue> {
    use std::cell::RefCell;
    use std::rc::Rc;

    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::closure::Closure;

    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("нет документа"))?;
    let input: web_sys::HtmlInputElement = document.create_element("input")?.dyn_into()?;
    input.set_type("file");
    input.set_multiple(spec.many);
    if let Some((_, extensions)) = spec.filter {
        let accept: Vec<String> = extensions.iter().map(|ext| format!(".{ext}")).collect();
        input.set_accept(&accept.join(","));
    }
    let done = Rc::new(RefCell::new(Some(done)));
    let finish = {
        let done = Rc::clone(&done);
        move |files: Vec<PickedFile>| {
            if let Some(done) = done.borrow_mut().take() {
                done(files);
            }
            ctx.request_repaint();
        }
    };
    let finish = Rc::new(finish);
    let on_change = {
        let input = input.clone();
        let finish = Rc::clone(&finish);
        Closure::once_into_js(move || {
            wasm_bindgen_futures::spawn_local(async move {
                let mut picked = Vec::new();
                if let Some(list) = input.files() {
                    for index in 0..list.length() {
                        let Some(file) = list.get(index) else {
                            continue;
                        };
                        #[allow(
                            clippy::cast_possible_truncation,
                            clippy::cast_sign_loss,
                            reason = "размер файла в байтах"
                        )]
                        let size = file.size() as u64;
                        let bytes = if let Some(problem) = size_problem(size, spec.limit) {
                            Err(problem)
                        } else {
                            wasm_bindgen_futures::JsFuture::from(file.array_buffer())
                                .await
                                .map(|buffer| js_sys::Uint8Array::new(&buffer).to_vec())
                                .map_err(|err| format!("{err:?}"))
                        };
                        picked.push(PickedFile {
                            name: file.name(),
                            bytes,
                        });
                    }
                }
                finish(picked);
            });
        })
    };
    let on_cancel = {
        let finish = Rc::clone(&finish);
        Closure::once_into_js(move || finish(Vec::new()))
    };
    input.add_event_listener_with_callback("change", on_change.unchecked_ref())?;
    input.add_event_listener_with_callback("cancel", on_cancel.unchecked_ref())?;
    input.click();
    Ok(())
}

/// Системный диалог выбора файла.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use rfd::FileDialog;

/// Диалог выбора файла в браузере: путей к файлам там нет, файлы приходят
/// через загрузку на странице, поэтому диалог ничего не выбирает.
#[cfg(target_arch = "wasm32")]
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct FileDialog;

#[cfg(target_arch = "wasm32")]
#[allow(clippy::unused_self, reason = "повторяет crate::sys::FileDialog")]
impl FileDialog {
    pub(crate) const fn new() -> Self {
        Self
    }

    pub(crate) fn add_filter(
        self,
        _name: impl Into<String>,
        _extensions: &[impl ToString],
    ) -> Self {
        self
    }

    pub(crate) fn set_file_name(self, _name: impl Into<String>) -> Self {
        self
    }

    pub(crate) const fn pick_file(self) -> Option<std::path::PathBuf> {
        None
    }

    pub(crate) const fn pick_files(self) -> Option<Vec<std::path::PathBuf>> {
        None
    }

    pub(crate) const fn pick_folder(self) -> Option<std::path::PathBuf> {
        None
    }

    pub(crate) const fn save_file(self) -> Option<std::path::PathBuf> {
        None
    }
}

/// Работа, которая не должна держать кадр интерфейса (диалог выбора файла,
/// чтение файлов): на десктопе — в отдельном потоке, в браузере потоков
/// нет, и она выполняется сразу.
pub(crate) fn background(work: impl FnOnce() + Send + 'static) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let spawned = std::thread::Builder::new()
            .name("background".to_owned())
            .spawn(work);
        if let Err(err) = spawned {
            tracing::warn!(%err, "фоновый поток не запустился");
        }
    }
    #[cfg(target_arch = "wasm32")]
    work();
}
