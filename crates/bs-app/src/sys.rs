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
    #[cfg(not(target_arch = "wasm32"))]
    {
        opener::reveal(path).map_err(|err| err.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        Err(crate::i18n::tr("В браузере файл открывается из загрузок браузера").to_owned())
    }
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
