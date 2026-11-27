//! Встроенный видеоплеер.
//!
//! Устроен как аудиопроигрыватель: виджеты читают [`View`] и складывают
//! команды в [`VideoHub`] (он в контексте egui), а кадры и звук ведёт
//! [`Engine`] в приложении. На десктопе видео декодирует `bs-media` (H.264 в
//! MP4/MOV на чистом Rust), звук — symphonia; в браузере — его `<video>`.

use std::sync::{Arc, Mutex, MutexGuard};

use egui::{ColorImage, Context, TextureHandle, TextureOptions};

use crate::i18n::tr;
use crate::images::{ImageHub, RawPurpose};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
use native::Backend;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
use web::Backend;

/// Крупнее не проигрываем: файл целиком лежит в памяти.
const MAX_BYTES: i64 = 300 * 1024 * 1024;
/// Без превью сервера кадр для превью делаем сами, но только у небольших файлов.
const MAX_THUMB_BYTES: i64 = 25 * 1024 * 1024;

const EXTENSIONS: [&str; 5] = ["mp4", "m4v", "mov", "webm", "mkv"];

/// Видео, которое можно проиграть прямо в ленте.
pub(crate) fn is_video(mimetype: &str, name: &str, size: i64) -> bool {
    if size > MAX_BYTES {
        return false;
    }
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    mimetype.starts_with("video/") || EXTENSIONS.contains(&ext.as_str())
}

/// Видеосообщение-«кружочек» (так их называет мост Telegram).
pub(crate) fn is_round(name: &str) -> bool {
    name.starts_with("video_note_")
}

/// Что происходит с видео.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    Loading,
    Playing,
    Paused,
    Failed(String),
}

/// Состояние текущего видео для виджета.
#[derive(Clone)]
pub(crate) struct View {
    pub(crate) file: String,
    pub(crate) status: Status,
    /// Позиция, секунды.
    pub(crate) pos: f32,
    pub(crate) len: Option<f32>,
    /// Последний показанный кадр.
    pub(crate) frame: Option<TextureHandle>,
    /// Развёрнуто на всё окно.
    pub(crate) expanded: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Command {
    Toggle(String),
    Seek(String, f32),
    Expand(bool),
    Thumbnail(String),
}

#[derive(Default)]
struct HubInner {
    view: Option<View>,
    commands: Vec<Command>,
    /// Превью, сделанные клиентом (у сервера нет ffmpeg и т. п.).
    thumbs: std::collections::HashMap<String, Option<TextureHandle>>,
}

/// Общая часть плеера.
#[derive(Clone, Default)]
pub(crate) struct VideoHub(Arc<Mutex<HubInner>>);

fn hub_id() -> egui::Id {
    egui::Id::unique("bs_video_hub")
}

impl VideoHub {
    pub(crate) fn of(ctx: &Context) -> Option<Self> {
        ctx.data(|d| d.get_temp::<Self>(hub_id()))
    }

    /// Состояние, если сейчас выбрано именно это видео.
    pub(crate) fn view(&self, file: &str) -> Option<View> {
        self.lock().view.clone().filter(|view| view.file == file)
    }

    /// Развёрнутое видео, если есть.
    pub(crate) fn expanded(&self) -> Option<View> {
        self.lock().view.clone().filter(|view| view.expanded)
    }

    pub(crate) fn toggle(&self, file: &str) {
        self.push(Command::Toggle(file.to_owned()));
    }

    pub(crate) fn seek(&self, file: &str, secs: f32) {
        self.push(Command::Seek(file.to_owned(), secs));
    }

    pub(crate) fn expand(&self, on: bool) {
        self.push(Command::Expand(on));
    }

    /// Превью, сделанное клиентом; `None` — ещё нет (и оно заказывается).
    pub(crate) fn thumbnail(&self, file: &str, size: i64) -> Option<TextureHandle> {
        let mut inner = self.lock();
        if let Some(known) = inner.thumbs.get(file) {
            return known.clone();
        }
        if size <= MAX_THUMB_BYTES && cfg!(not(target_arch = "wasm32")) {
            inner.thumbs.insert(file.to_owned(), None);
            inner.commands.push(Command::Thumbnail(file.to_owned()));
        }
        None
    }

    fn push(&self, command: Command) {
        self.lock().commands.push(command);
    }

    fn lock(&self) -> MutexGuard<'_, HubInner> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Плеер приложения.
#[derive(Default)]
pub(crate) struct Engine {
    hub: VideoHub,
    backend: Backend,
    view: Option<View>,
    texture: Option<TextureHandle>,
    /// Последние скачанные файлы: превью и показ не качают дважды.
    cache: Vec<(String, Arc<[u8]>)>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").finish_non_exhaustive()
    }
}

/// Сколько байт видео держим в памяти после показа.
const CACHE_BYTES: usize = 160 * 1024 * 1024;

impl Engine {
    /// Раз в кадр: команды, кадр под текущее время, конец видео.
    pub(crate) fn pump(&mut self, ctx: &Context, images: &ImageHub) {
        ctx.data_mut(|d| d.insert_temp(hub_id(), self.hub.clone()));
        let commands = std::mem::take(&mut self.hub.lock().commands);
        for command in commands {
            self.command(command, images);
        }
        self.backend.poll_thumbnails(ctx, &self.hub);
        if let Some(view) = &mut self.view
            && matches!(view.status, Status::Playing | Status::Paused)
        {
            view.pos = self.backend.position();
            view.len = self.backend.len().or(view.len);
            if let Some(frame) = self.backend.frame(view.pos) {
                let image = ColorImage::from_rgba_unmultiplied(
                    [frame.width as usize, frame.height as usize],
                    &frame.rgba,
                );
                match &mut self.texture {
                    Some(texture) => texture.set(image, TextureOptions::LINEAR),
                    None => {
                        self.texture =
                            Some(ctx.load_texture("bs-video", image, TextureOptions::LINEAR));
                    }
                }
                view.frame.clone_from(&self.texture);
            }
            if view.status == Status::Playing {
                if self.backend.finished(view.pos) {
                    self.backend.pause();
                    view.status = Status::Paused;
                    view.pos = 0.0;
                    self.backend.rewind();
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(15));
            }
        }
        self.hub.lock().view.clone_from(&self.view);
    }

    fn command(&mut self, command: Command, images: &ImageHub) {
        match command {
            Command::Toggle(file) => self.toggle(file, images),
            Command::Seek(file, secs) => {
                if self.view.as_ref().is_some_and(|view| view.file == file) {
                    self.backend.seek(secs);
                    if let Some(view) = &mut self.view {
                        view.pos = secs;
                    }
                }
            }
            Command::Expand(on) => {
                if let Some(view) = &mut self.view {
                    view.expanded = on;
                }
            }
            Command::Thumbnail(file) => {
                if let Some(bytes) = self.cached(&file) {
                    self.backend.thumbnail(file, bytes);
                } else {
                    images.request_raw(&file, RawPurpose::VideoThumb(file.clone()));
                }
            }
        }
    }

    fn cached(&self, file: &str) -> Option<Arc<[u8]>> {
        self.cache
            .iter()
            .find(|(name, _)| name == file)
            .map(|(_, bytes)| Arc::clone(bytes))
    }

    fn remember(&mut self, file: &str, bytes: Arc<[u8]>) {
        self.cache.retain(|(name, _)| name != file);
        self.cache.push((file.to_owned(), bytes));
        while self.cache.iter().map(|(_, b)| b.len()).sum::<usize>() > CACHE_BYTES
            && self.cache.len() > 1
        {
            self.cache.remove(0);
        }
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "имя уходит в новое состояние"
    )]
    fn toggle(&mut self, file: String, images: &ImageHub) {
        let status = self
            .view
            .as_ref()
            .filter(|view| view.file == file)
            .map(|view| view.status.clone());
        match status {
            Some(Status::Playing) => {
                self.backend.pause();
                self.set_status(Status::Paused);
            }
            Some(Status::Paused) => {
                self.backend.resume();
                self.set_status(Status::Playing);
            }
            Some(Status::Loading) => {}
            Some(Status::Failed(_)) | None => {
                self.backend.stop();
                self.texture = None;
                let expanded = self.view.as_ref().is_some_and(|view| view.expanded);
                self.view = Some(View {
                    file: file.clone(),
                    status: Status::Loading,
                    pos: 0.0,
                    len: None,
                    frame: None,
                    expanded: expanded && self.view.as_ref().is_some_and(|v| v.file == file),
                });
                if let Some(bytes) = self.cached(&file) {
                    self.start(&file, bytes);
                } else if !images.request_raw(&file, RawPurpose::Video(file.clone())) {
                    self.set_status(Status::Failed(tr("Сервер не подключён").to_owned()));
                }
            }
        }
    }

    fn set_status(&mut self, status: Status) {
        if let Some(view) = &mut self.view {
            view.status = status;
        }
    }

    fn start(&mut self, file: &str, bytes: Arc<[u8]>) {
        let status = match self.backend.start(Arc::clone(&bytes), file) {
            Ok(()) => Status::Playing,
            Err(err) => Status::Failed(err),
        };
        self.remember(file, bytes);
        if let Some(view) = &mut self.view {
            view.len = self.backend.len();
            view.status = status;
        }
    }

    /// Файл для показа скачан.
    pub(crate) fn loaded(&mut self, file: &str, result: Result<Vec<u8>, String>) {
        let waiting = self
            .view
            .as_ref()
            .is_some_and(|view| view.file == file && view.status == Status::Loading);
        if !waiting {
            return;
        }
        match result {
            Ok(bytes) => self.start(file, bytes.into()),
            Err(err) => self.set_status(Status::Failed(err)),
        }
    }

    /// Файл для превью скачан.
    pub(crate) fn thumb_loaded(&mut self, file: &str, result: Result<Vec<u8>, String>) {
        // Не вышло — больше не пробуем (до перезапуска).
        if let Ok(bytes) = result {
            let bytes: Arc<[u8]> = bytes.into();
            self.remember(file, Arc::clone(&bytes));
            self.backend.thumbnail(file.to_owned(), bytes);
        }
    }
}

/// Кадр для показа (общий для обоих бэкендов).
pub(crate) struct Frame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_videos() {
        assert!(is_video("video/mp4", "a.bin", 10));
        assert!(is_video("application/octet-stream", "clip.MOV", 10));
        assert!(!is_video("image/png", "a.png", 10));
        assert!(!is_video("video/mp4", "a.mp4", MAX_BYTES + 1));
        assert!(is_round("video_note_12.mp4"));
        assert!(!is_round("clip.mp4"));
    }
}
