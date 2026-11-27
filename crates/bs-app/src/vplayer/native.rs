//! Десктоп: кадры декодирует отдельный поток (`bs-media`), звук — symphonia
//! через rodio; часы — позиция звука, а без звука — обычное время.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::time::Instant;

use bs_media::video::{VideoFrame, VideoStream};
use egui::{ColorImage, Context, TextureOptions};

use super::{Frame, VideoHub};
use crate::i18n::tr;

#[cfg(feature = "sounds")]
mod audio;

/// Сколько готовых кадров держим впереди.
const AHEAD: usize = 6;

enum Control {
    /// Новое поколение кадров с этого места (мс).
    Seek(u64, u64),
}

/// Поток декодирования: кадры с номером поколения; `None` — видео кончилось.
struct Worker {
    control: Sender<Control>,
    frames: Receiver<(u64, Option<VideoFrame>)>,
}

fn spawn_worker(mut stream: VideoStream) -> Worker {
    let (control, commands) = mpsc::channel::<Control>();
    let (out, frames): (SyncSender<_>, _) = mpsc::sync_channel(AHEAD);
    std::thread::Builder::new()
        .name("bs-video".to_owned())
        .spawn(move || {
            let mut generation = 0;
            loop {
                match commands.try_recv() {
                    Ok(Control::Seek(next, ms)) => {
                        generation = next;
                        stream.seek(ms);
                        continue;
                    }
                    Err(TryRecvError::Disconnected) => return,
                    Err(TryRecvError::Empty) => {}
                }
                let frame = stream.next_frame();
                let ended = frame.is_none();
                if out.send((generation, frame)).is_err() {
                    return;
                }
                if ended {
                    // Ждём перемотки (или конца плеера).
                    match commands.recv() {
                        Ok(Control::Seek(next, ms)) => {
                            generation = next;
                            stream.seek(ms);
                        }
                        Err(_) => return,
                    }
                }
            }
        })
        .ok();
    Worker { control, frames }
}

/// Часы без звука.
#[derive(Default)]
struct Clock {
    base: f32,
    since: Option<Instant>,
}

impl Clock {
    fn now(&self) -> f32 {
        self.base
            + self
                .since
                .map_or(0.0, |since| since.elapsed().as_secs_f32())
    }

    fn pause(&mut self) {
        self.base = self.now();
        self.since = None;
    }

    fn play(&mut self) {
        if self.since.is_none() {
            self.since = Some(Instant::now());
        }
    }

    fn set(&mut self, at: f32) {
        self.base = at;
        if self.since.is_some() {
            self.since = Some(Instant::now());
        }
    }
}

#[derive(Default)]
pub(super) struct Backend {
    worker: Option<Worker>,
    generation: u64,
    queue: VecDeque<VideoFrame>,
    ended: bool,
    /// После старта или перемотки первый кадр показываем, не дожидаясь часов.
    fresh: bool,
    len: Option<f32>,
    clock: Clock,
    #[cfg(feature = "sounds")]
    audio: Option<audio::Output>,
    thumbs: Vec<Receiver<(String, Option<VideoFrame>)>>,
}

impl Backend {
    pub(super) fn start(&mut self, bytes: Arc<[u8]>, _name: &str) -> Result<(), String> {
        self.stop();
        let stream = VideoStream::open(Arc::clone(&bytes)).map_err(|err| match err {
            bs_media::video::VideoError::Codec => {
                tr("Кодек видео не поддерживается — скачайте файл").to_owned()
            }
            other => crate::tf!("Видео не открылось: {err}", err = other.to_string()),
        })?;
        self.len = Some(stream.info().duration as f32).filter(|len| *len > 0.0);
        self.worker = Some(spawn_worker(stream));
        #[cfg(feature = "sounds")]
        {
            self.audio = audio::Output::open(bytes);
        }
        self.clock = Clock::default();
        self.clock.play();
        self.fresh = true;
        Ok(())
    }

    pub(super) fn pause(&mut self) {
        self.clock.pause();
        #[cfg(feature = "sounds")]
        if let Some(audio) = &self.audio {
            audio.pause();
        }
    }

    pub(super) fn resume(&mut self) {
        self.clock.play();
        #[cfg(feature = "sounds")]
        if let Some(audio) = &self.audio {
            audio.play();
        }
    }

    pub(super) fn seek(&mut self, secs: f32) {
        let secs = secs.max(0.0);
        self.generation += 1;
        if let Some(worker) = &self.worker {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "секунды ≥ 0"
            )]
            let ms = (secs * 1000.0) as u64;
            let _ = worker.control.send(Control::Seek(self.generation, ms));
            // Освобождаем очередь, чтобы поток увидел перемотку.
            while worker.frames.try_recv().is_ok() {}
        }
        self.queue.clear();
        self.ended = false;
        self.fresh = true;
        self.clock.set(secs);
        #[cfg(feature = "sounds")]
        if let Some(audio) = &self.audio {
            audio.seek(secs);
        }
    }

    /// После конца: в начало, на паузе.
    pub(super) fn rewind(&mut self) {
        self.seek(0.0);
        self.pause();
    }

    pub(super) fn stop(&mut self) {
        self.worker = None;
        self.queue.clear();
        self.ended = false;
        self.len = None;
        #[cfg(feature = "sounds")]
        {
            self.audio = None;
        }
    }

    pub(super) fn position(&self) -> f32 {
        #[cfg(feature = "sounds")]
        if let Some(audio) = &self.audio
            && !audio.finished()
        {
            return audio.position();
        }
        self.clock.now()
    }

    pub(super) const fn len(&self) -> Option<f32> {
        self.len
    }

    /// Последний кадр, которому пора на экран к `pos`.
    pub(super) fn frame(&mut self, pos: f32) -> Option<Frame> {
        if let Some(worker) = &self.worker {
            while self.queue.len() < AHEAD && !self.ended {
                match worker.frames.try_recv() {
                    Ok((generation, _)) if generation != self.generation => {}
                    Ok((_, Some(frame))) => self.queue.push_back(frame),
                    Ok((_, None)) => self.ended = true,
                    Err(_) => break,
                }
            }
        }
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "секунды ≥ 0"
        )]
        let now = (pos.max(0.0) * 1000.0) as u64;
        let mut shown = None;
        while self.queue.front().is_some_and(|frame| frame.pts_ms <= now) {
            shown = self.queue.pop_front();
        }
        // Первый кадр после старта и перемотки показываем сразу.
        if shown.is_none() && self.fresh {
            shown = self.queue.pop_front();
        }
        if shown.is_some() {
            self.fresh = false;
        }
        shown.map(|frame| Frame {
            width: frame.width,
            height: frame.height,
            rgba: frame.rgba,
        })
    }

    pub(super) fn finished(&self, pos: f32) -> bool {
        self.ended && self.queue.is_empty() && self.len.is_none_or(|len| pos >= len - 0.1)
    }

    /// Превью из первого кадра — в фоне.
    pub(super) fn thumbnail(&mut self, file: String, bytes: Arc<[u8]>) {
        let (out, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("bs-video-thumb".to_owned())
            .spawn(move || {
                let frame = bs_media::video::first_frame(&bytes)
                    .ok()
                    .map(|(_, frame)| frame);
                let _ = out.send((file, frame));
            })
            .ok();
        self.thumbs.push(done);
    }

    pub(super) fn poll_thumbnails(&mut self, ctx: &Context, hub: &VideoHub) {
        self.thumbs.retain(|done| match done.try_recv() {
            Ok((file, frame)) => {
                let texture = frame.map(|frame| {
                    let image = ColorImage::from_rgba_unmultiplied(
                        [frame.width as usize, frame.height as usize],
                        &frame.rgba,
                    );
                    ctx.load_texture(
                        format!("bs-video-thumb-{file}"),
                        image,
                        TextureOptions::LINEAR,
                    )
                });
                hub.lock().thumbs.insert(file, texture);
                ctx.request_repaint();
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => false,
        });
    }
}
