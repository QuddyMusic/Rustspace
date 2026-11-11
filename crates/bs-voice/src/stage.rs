//! «Сцена» звонка: все видеопотоки — свои и чужие — с последними кадрами.
//!
//! Интерфейс рисует сцену синхронно, поэтому кадры лежат за обычным
//! мьютексом и отдаются как `Arc`: копирования пикселей нет. У каждого
//! потока есть номер поколения — по нему интерфейс понимает, что пора
//! обновить текстуру.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use livekit::prelude::*;
use livekit::webrtc::prelude::VideoBuffer as _;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt as _;
use tracing::debug;

use crate::camera::CameraCapture;
use crate::capture::{CapturedFrame, ScreenCapture};
use crate::convert::{RgbaFrame, buffer_to_rgba_fit, fit_size, i420_to_rgba};
use crate::error::VoiceResult;
use crate::state::{TrackKind, user_id_of};
use crate::video::{VideoPublishConfig, VideoPublisher};

/// Свой поток в предпросмотре: каждый какой кадр и до какого размера.
const PREVIEW_EVERY: u64 = 3;
const PREVIEW_WIDTH: u32 = 640;
const PREVIEW_HEIGHT: u32 = 360;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Один видеопоток на сцене.
#[derive(Debug, Clone)]
pub struct VideoFeed {
    /// Identity участника в LiveKit.
    pub identity: String,
    /// Id пользователя (часть identity до `:`).
    pub user_id: String,
    /// Камера или демонстрация экрана.
    pub kind: TrackKind,
    /// Это наш собственный поток (предпросмотр).
    pub local: bool,
    /// Последний кадр.
    pub frame: Option<Arc<RgbaFrame>>,
    /// Растёт с каждым новым кадром.
    pub generation: u64,
}

#[derive(Debug)]
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Debug)]
struct Slot {
    feed: VideoFeed,
    _task: Option<AbortOnDrop>,
}

/// Что захватываем у себя.
#[derive(Debug)]
pub(crate) enum LocalCapture {
    Screen(ScreenCapture),
    Camera(CameraCapture),
}

impl LocalCapture {
    fn stop(&mut self) {
        match self {
            Self::Screen(capture) => capture.stop(),
            Self::Camera(capture) => capture.stop(),
        }
    }
}

#[derive(Debug)]
struct LocalShare {
    capture: LocalCapture,
    publisher: VideoPublisher,
    _forward: AbortOnDrop,
}

/// Все видеопотоки звонка.
#[derive(Debug, Default)]
pub struct VideoStage {
    slots: Mutex<HashMap<(String, TrackKind), Slot>>,
    screen: tokio::sync::Mutex<Option<LocalShare>>,
    camera: tokio::sync::Mutex<Option<LocalShare>>,
    /// identity ведущего → identity зрителей.
    watchers: Mutex<BTreeMap<String, BTreeSet<String>>>,
}

impl VideoStage {
    /// Снимок сцены: сначала демонстрации, потом камеры; внутри — по identity.
    #[must_use]
    pub fn feeds(&self) -> Vec<VideoFeed> {
        let mut feeds: Vec<VideoFeed> = lock(&self.slots)
            .values()
            .map(|slot| slot.feed.clone())
            .collect();
        feeds.sort_by(|a, b| {
            (a.kind != TrackKind::ScreenShare, &a.identity)
                .cmp(&(b.kind != TrackKind::ScreenShare, &b.identity))
        });
        feeds
    }

    fn insert(&self, identity: &str, kind: TrackKind, local: bool, task: Option<AbortOnDrop>) {
        lock(&self.slots).insert(
            (identity.to_owned(), kind),
            Slot {
                feed: VideoFeed {
                    identity: identity.to_owned(),
                    user_id: user_id_of(identity).to_owned(),
                    kind,
                    local,
                    frame: None,
                    generation: 0,
                },
                _task: task,
            },
        );
    }

    fn store(&self, identity: &str, kind: TrackKind, frame: RgbaFrame) {
        if let Some(slot) = lock(&self.slots).get_mut(&(identity.to_owned(), kind)) {
            slot.feed.frame = Some(Arc::new(frame));
            slot.feed.generation += 1;
        }
    }

    /// Начинает принимать удалённый видеотрек.
    pub(crate) fn attach_remote(
        self: &Arc<Self>,
        identity: &str,
        kind: TrackKind,
        track: &RemoteVideoTrack,
    ) {
        let mut stream = NativeVideoStream::new(track.rtc_track());
        let stage = Arc::downgrade(self);
        let owner = identity.to_owned();
        let task = tokio::spawn(async move {
            while let Some(frame) = stream.next().await {
                let Some(stage) = stage.upgrade() else {
                    break;
                };
                // Больше 1080p на экране всё равно не показать.
                match buffer_to_rgba_fit(frame.buffer.as_ref(), 1920, 1080) {
                    Ok(rgba) => stage.store(&owner, kind, rgba),
                    Err(err) => debug!(error = %err, "кадр видео пропущен"),
                }
            }
        });
        self.insert(identity, kind, false, Some(AbortOnDrop(task)));
    }

    /// Трансляция опубликована, но мы её не принимаем (не смотрим):
    /// плитка есть, кадров нет. Уже принимаемый поток не трогается.
    pub(crate) fn announce_remote(&self, identity: &str, kind: TrackKind) {
        if !lock(&self.slots).contains_key(&(identity.to_owned(), kind)) {
            self.insert(identity, kind, false, None);
        }
    }

    /// Перестали принимать поток, но он ещё опубликован: остаётся плитка
    /// без кадров, зрители ведущего не трогаются.
    pub(crate) fn park_remote(&self, identity: &str, kind: TrackKind) {
        self.insert(identity, kind, false, None);
    }

    /// Убирает видеопоток.
    pub(crate) fn detach(&self, identity: &str, kind: TrackKind) {
        lock(&self.slots).remove(&(identity.to_owned(), kind));
        if kind == TrackKind::ScreenShare {
            lock(&self.watchers).remove(identity);
        }
    }

    /// Участник ушёл: убираем его потоки и его самого из зрителей.
    pub(crate) fn forget_participant(&self, identity: &str) {
        lock(&self.slots).retain(|(owner, _), _| owner != identity);
        let mut watchers = lock(&self.watchers);
        watchers.remove(identity);
        for set in watchers.values_mut() {
            set.remove(identity);
        }
    }

    /// Публикует свой поток и пересылает в него кадры (и в предпросмотр).
    pub(crate) async fn start_local(
        self: &Arc<Self>,
        room: &Room,
        local_identity: &str,
        capture: LocalCapture,
        mut frames: mpsc::Receiver<CapturedFrame>,
        size: (u32, u32),
        config: &VideoPublishConfig,
    ) -> VoiceResult<()> {
        let (kind, source, name, screencast) = match capture {
            LocalCapture::Screen(_) => (
                TrackKind::ScreenShare,
                TrackSource::Screenshare,
                "screen",
                true,
            ),
            LocalCapture::Camera(_) => (TrackKind::Video, TrackSource::Camera, "camera", false),
        };
        self.stop_local(room, kind).await?;
        let publisher = VideoPublisher::new(name, size.0, size.1, screencast)?;
        publisher.publish(room, config, source).await?;
        let target = publisher.clone();
        let stage = Arc::downgrade(self);
        let owner = local_identity.to_owned();
        let forward = tokio::spawn(async move {
            let mut count = 0_u64;
            while let Some(frame) = frames.recv().await {
                let mut i420 = frame.i420;
                // Предпросмотр себе — маленький и не на каждом кадре.
                if count % PREVIEW_EVERY == 0 {
                    let Some(stage) = stage.upgrade() else {
                        break;
                    };
                    let (width, height) =
                        fit_size(i420.width(), i420.height(), PREVIEW_WIDTH, PREVIEW_HEIGHT);
                    if width > 0 {
                        #[allow(clippy::cast_possible_wrap, reason = "размер превью мал")]
                        let preview = i420.scale(width as i32, height as i32);
                        stage.store(&owner, kind, i420_to_rgba(&preview));
                    }
                }
                count += 1;
                if let Err(err) = target.push_i420(i420) {
                    debug!(error = %err, "кадр не отправлен");
                }
            }
        });
        self.insert(local_identity, kind, true, None);
        let share = LocalShare {
            capture,
            publisher,
            _forward: AbortOnDrop(forward),
        };
        *self.slot_for(kind).lock().await = Some(share);
        Ok(())
    }

    fn slot_for(&self, kind: TrackKind) -> &tokio::sync::Mutex<Option<LocalShare>> {
        if kind == TrackKind::ScreenShare {
            &self.screen
        } else {
            &self.camera
        }
    }

    /// Останавливает свой поток; `true`, если он шёл.
    pub(crate) async fn stop_local(&self, room: &Room, kind: TrackKind) -> VoiceResult<bool> {
        let share = self.slot_for(kind).lock().await.take();
        let Some(mut share) = share else {
            return Ok(false);
        };
        share.capture.stop();
        let identity = room.local_participant().identity().to_string();
        self.detach(&identity, kind);
        share.publisher.unpublish(room).await?;
        Ok(true)
    }

    /// Останавливает захват без снятия публикации — при выходе из звонка
    /// публикации снимет закрытие комнаты, а лишняя пересогласовка SDP
    /// гонялась бы с ним.
    pub(crate) async fn drop_local(&self, kind: TrackKind) {
        let share = self.slot_for(kind).lock().await.take();
        if let Some(mut share) = share {
            share.capture.stop();
        }
    }

    /// Идёт ли наш поток этого вида.
    pub(crate) async fn is_local_running(&self, kind: TrackKind) -> bool {
        self.slot_for(kind).lock().await.is_some()
    }

    /// Запоминает, что зритель смотрит (или нет) трансляцию ведущего.
    pub(crate) fn record_watch(&self, sharer: &str, watcher: &str, watching: bool) {
        let mut watchers = lock(&self.watchers);
        let set = watchers.entry(sharer.to_owned()).or_default();
        if watching {
            set.insert(watcher.to_owned());
        } else {
            set.remove(watcher);
        }
        if set.is_empty() {
            watchers.remove(sharer);
        }
    }

    /// Кто смотрит трансляцию ведущего (identity зрителей).
    #[must_use]
    pub fn watchers_of(&self, sharer: &str) -> Vec<String> {
        lock(&self.watchers)
            .get(sharer)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Сцена пуста и ничего не захватываем.
    pub(crate) fn clear(&self) {
        lock(&self.slots).clear();
        lock(&self.watchers).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> RgbaFrame {
        RgbaFrame {
            width: 2,
            height: 2,
            rgba: vec![0; 16],
        }
    }

    #[test]
    fn screen_shares_come_first_and_frames_bump_generation() {
        let stage = VideoStage::default();
        stage.insert("b:Bob", TrackKind::Video, false, None);
        stage.insert("a:Ann", TrackKind::ScreenShare, false, None);
        stage.store("b:Bob", TrackKind::Video, frame());
        stage.store("b:Bob", TrackKind::Video, frame());
        let feeds = stage.feeds();
        assert_eq!(feeds[0].kind, TrackKind::ScreenShare);
        assert_eq!(feeds[1].user_id, "b");
        assert_eq!(feeds[1].generation, 2);
        assert!(feeds[0].frame.is_none());
    }

    #[test]
    fn watchers_are_tracked_and_cleared_with_participant() {
        let stage = VideoStage::default();
        stage.record_watch("a:Ann", "b:Bob", true);
        stage.record_watch("a:Ann", "c:Cid", true);
        stage.record_watch("a:Ann", "c:Cid", false);
        assert_eq!(stage.watchers_of("a:Ann"), vec!["b:Bob".to_owned()]);
        stage.forget_participant("b:Bob");
        assert!(stage.watchers_of("a:Ann").is_empty());
    }

    #[test]
    fn ending_a_share_drops_its_watchers() {
        let stage = VideoStage::default();
        stage.insert("a:Ann", TrackKind::ScreenShare, false, None);
        stage.record_watch("a:Ann", "b:Bob", true);
        stage.detach("a:Ann", TrackKind::ScreenShare);
        assert!(stage.feeds().is_empty());
        assert!(stage.watchers_of("a:Ann").is_empty());
    }
}
