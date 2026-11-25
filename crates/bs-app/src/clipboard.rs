//! Файлы из системного буфера обмена (скопированные в проводнике).

use std::path::PathBuf;

/// Читает список файлов; блокирует поток, поэтому вызывать не из потока
/// интерфейса.
pub(crate) fn files() -> Option<Vec<PathBuf>> {
    // В браузере файлы вставляются только самой страницей.
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut board = arboard::Clipboard::new().ok()?;
        let files = board.get().file_list().ok()?;
        (!files.is_empty()).then_some(files)
    }
}
