//! Встроенный проигрыватель аудиовложений.
//!
//! Виджеты в ленте только читают [`View`] и складывают команды в [`AudioHub`]
//! (он живёт в контексте egui). Сам звук ведёт [`Engine`] в приложении:
//! устройство вывода и `<audio>` в браузере нельзя делить между потоками.
//! На десктопе — rodio/symphonia (WAV, MP3, FLAC, Ogg Vorbis, AAC/M4A) и
//! Ogg Opus на чистом Rust; в браузере — его собственный `<audio>`.

use std::sync::{Arc, Mutex, MutexGuard};

use egui::Context;

use crate::i18n::tr;
use crate::images::{ImageHub, RawPurpose};

#[cfg(all(feature = "sounds", not(target_arch = "wasm32")))]
mod native;
#[cfg(all(feature = "sounds", not(target_arch = "wasm32")))]
use native::Backend;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
use web::Backend;
#[cfg(not(any(feature = "sounds", target_arch = "wasm32")))]
mod none;
#[cfg(not(any(feature = "sounds", target_arch = "wasm32")))]
use none::Backend;

/// Крупнее не проигрываем: файл целиком лежит в памяти.
const MAX_BYTES: i64 = 200 * 1024 * 1024;

/// Расширения, которые узнаём, даже если сервер не знает тип файла.
const EXTENSIONS: [&str; 10] = [
    "wav", "mp3", "flac", "ogg", "oga", "opus", "m4a", "aac", "mp4a", "wave",
];

/// Можно ли проиграть вложение прямо в ленте.
pub(crate) fn is_playable(mimetype: &str, name: &str, size: i64) -> bool {
    if size > MAX_BYTES {
        return false;
    }
    let ext = extension(name);
    mimetype.starts_with("audio/") || EXTENSIONS.contains(&ext.as_str())
}

/// Расширение имени в нижнем регистре.
fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Что происходит с дорожкой.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Status {
    /// Файл скачивается.
    Loading,
    /// Играет.
    Playing,
    /// На паузе (или доиграла).
    Paused,
    /// Не получилось; текст для подсказки.
    Failed(String),
}

/// Состояние текущей дорожки для виджета.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct View {
    /// Имя файла на сервере.
    pub(crate) file: String,
    pub(crate) status: Status,
    /// Позиция, секунды.
    pub(crate) pos: f32,
    /// Длительность, если известна.
    pub(crate) len: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
enum Command {
    Toggle(String),
    Seek(String, f32),
}

#[derive(Debug, Default)]
struct HubInner {
    view: Option<View>,
    commands: Vec<Command>,
}

/// Общая часть проигрывателя: что показывать и что попросили.
#[derive(Debug, Clone, Default)]
pub(crate) struct AudioHub(Arc<Mutex<HubInner>>);

fn hub_id() -> egui::Id {
    egui::Id::unique("bs_audio_hub")
}

impl AudioHub {
    /// Хаб из контекста.
    pub(crate) fn of(ctx: &Context) -> Option<Self> {
        ctx.data(|d| d.get_temp::<Self>(hub_id()))
    }

    /// Состояние, если сейчас выбрана именно эта дорожка.
    pub(crate) fn view(&self, file: &str) -> Option<View> {
        self.lock().view.clone().filter(|view| view.file == file)
    }

    /// Играть/пауза.
    pub(crate) fn toggle(&self, file: &str) {
        self.lock().commands.push(Command::Toggle(file.to_owned()));
    }

    /// Перемотка, секунды.
    pub(crate) fn seek(&self, file: &str, secs: f32) {
        self.lock()
            .commands
            .push(Command::Seek(file.to_owned(), secs));
    }

    fn lock(&self) -> MutexGuard<'_, HubInner> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Проигрыватель приложения.
#[derive(Default)]
pub(crate) struct Engine {
    hub: AudioHub,
    backend: Backend,
    view: Option<View>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("view", &self.view)
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Раз в кадр: команды виджетов, позиция, конец дорожки.
    pub(crate) fn pump(&mut self, ctx: &Context, images: &ImageHub) {
        ctx.data_mut(|d| d.insert_temp(hub_id(), self.hub.clone()));
        let commands = std::mem::take(&mut self.hub.lock().commands);
        for command in commands {
            match command {
                Command::Toggle(file) => self.toggle(file, images),
                Command::Seek(file, secs) => {
                    if self.current(&file)
                        && self.backend.seek(secs)
                        && let Some(view) = &mut self.view
                    {
                        view.pos = secs;
                    }
                }
            }
        }
        if let Some(view) = &mut self.view
            && view.status == Status::Playing
        {
            view.pos = self.backend.position();
            view.len = self.backend.len().or(view.len);
            if self.backend.finished() {
                view.status = Status::Paused;
                view.pos = 0.0;
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.hub.lock().view.clone_from(&self.view);
    }

    fn current(&self, file: &str) -> bool {
        self.view.as_ref().is_some_and(|view| view.file == file)
    }

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
                let at = self.view.as_ref().map_or(0.0, |view| view.pos);
                match self.backend.resume(at) {
                    Ok(()) => self.set_status(Status::Playing),
                    Err(err) => self.set_status(Status::Failed(err)),
                }
            }
            Some(Status::Loading) => {}
            Some(Status::Failed(_)) | None => {
                self.backend.stop();
                let status = if images.request_raw(&file, RawPurpose::Audio(file.clone())) {
                    Status::Loading
                } else {
                    Status::Failed(tr("Сервер не подключён").to_owned())
                };
                self.view = Some(View {
                    file,
                    status,
                    pos: 0.0,
                    len: None,
                });
            }
        }
    }

    fn set_status(&mut self, status: Status) {
        if let Some(view) = &mut self.view {
            view.status = status;
        }
    }

    /// Файл скачан: начинаем играть, если его всё ещё ждут.
    pub(crate) fn loaded(&mut self, file: &str, result: Result<Vec<u8>, String>) {
        let waiting = self
            .view
            .as_ref()
            .is_some_and(|view| view.file == file && view.status == Status::Loading);
        if !waiting {
            return;
        }
        let status = match result.and_then(|bytes| self.backend.start(bytes, &extension(file))) {
            Ok(()) => Status::Playing,
            Err(err) => Status::Failed(err),
        };
        if let Some(view) = &mut self.view {
            view.len = self.backend.len();
            view.status = status;
        }
    }
}

/// `м:сс` (или `ч:мм:сс`).
pub(crate) fn clock(secs: f32) -> String {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "секунды дорожки неотрицательны и невелики"
    )]
    let total = secs.max(0.0) as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_audio() {
        assert!(is_playable("audio/x-wav", "a.bin", 10));
        assert!(is_playable("application/octet-stream", "Voice.OPUS", 10));
        assert!(!is_playable("application/pdf", "a.pdf", 10));
        assert!(!is_playable("audio/flac", "a.flac", MAX_BYTES + 1));
    }

    #[test]
    fn formats_clock() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(65.4), "1:05");
        assert_eq!(clock(3725.0), "1:02:05");
    }
}
