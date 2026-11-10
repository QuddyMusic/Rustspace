//! Голосовая сессия: подключение к комнате LiveKit и обработка событий.

use std::sync::Arc;

use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use tokio::sync::{RwLock, broadcast};
use tracing::{debug, info};

use crate::audio::{AudioEngine, map_track_kind, mix_source};
use crate::camera::{CameraConfig, start_camera};
use crate::capture::{CaptureConfig, ScreenCapture, ScreenSource};
use crate::config::VoiceConfig;
use crate::error::{VoiceError, VoiceResult};
use crate::events::VoiceEvent;
use crate::mixer::SourceKind;
use crate::signal::{DataMessage, watch_key};
use crate::stage::{LocalCapture, VideoFeed, VideoStage};
use crate::state::{ParticipantInfo, RoomSnapshot, RoomState, TrackKind, user_id_of};
use crate::sysaudio::{self, SystemAudio};
use crate::video::VideoPublishConfig;
use crate::volume::{Volume, VolumeMap};

/// Ёмкость канала событий.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Активная голосовая сессия.
#[derive(Debug, Clone)]
pub struct VoiceSession {
    room: Arc<Room>,
    snapshot: Arc<RwLock<RoomSnapshot>>,
    events: broadcast::Sender<VoiceEvent>,
    audio: Arc<AudioEngine>,
    volumes: Arc<RwLock<VolumeMap>>,
    identity: String,
    mic_publication: Arc<RwLock<Option<LocalTrackPublication>>>,
    muted: Arc<RwLock<bool>>,
    deafened: Arc<RwLock<bool>>,
    /// Почему микрофон не опубликован (сессия при этом работает на приём).
    mic_error: Arc<std::sync::Mutex<Option<String>>>,
    /// Видеопотоки звонка.
    stage: Arc<VideoStage>,
    /// Системный звук демонстрации экрана.
    screen_audio: Arc<tokio::sync::Mutex<Option<ScreenAudio>>>,
    /// Чьи трансляции смотрим (identity ведущих).
    watched: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Подписываться ли на голос и камеры автоматически.
    auto_subscribe: bool,
}

/// Идущая трансляция системного звука.
#[derive(Debug)]
struct ScreenAudio {
    capture: SystemAudio,
    publication: LocalTrackPublication,
    forward: tokio::task::JoinHandle<()>,
}

impl VoiceSession {
    /// Подключается к комнате.
    ///
    /// `url` — `wss://…` LiveKit, `token` — JWT, выданный сервером Backspace.
    pub async fn connect(
        url: &str,
        token: &str,
        config: VoiceConfig,
        audio: AudioEngine,
        identity: impl Into<String>,
        display_name: impl Into<String>,
    ) -> VoiceResult<Self> {
        let identity = identity.into();
        let display_name = display_name.into();

        // Трансляции принимаем только у тех, кого смотрим (как оригинал):
        // остальное подписываем сами в `apply_subscription`.
        let mut options = RoomOptions::default();
        options.auto_subscribe = false;
        let (room, mut room_events) = Room::connect(url, token, options)
            .await
            .map_err(|e| VoiceError::Connect(e.to_string()))?;

        let mut snapshot = RoomSnapshot {
            room: room.name(),
            state: RoomState::Connecting,
            ..RoomSnapshot::default()
        };
        let mut local = ParticipantInfo::local(identity.clone(), display_name);
        local.muted = config.start_muted;
        local.deafened = config.start_deafened;
        snapshot.upsert(local);

        for participant in room.remote_participants().into_values() {
            snapshot.upsert(remote_info(&participant));
        }

        let snapshot = Arc::new(RwLock::new(snapshot));
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let volumes = Arc::new(RwLock::new(VolumeMap::default()));
        let mic_publication = Arc::new(RwLock::new(None));

        let session = Self {
            room: Arc::new(room),
            snapshot: Arc::clone(&snapshot),
            events: events.clone(),
            audio: Arc::new(audio),
            volumes,
            identity,
            mic_publication: Arc::clone(&mic_publication),
            muted: Arc::new(RwLock::new(config.start_muted)),
            deafened: Arc::new(RwLock::new(config.start_deafened)),
            mic_error: Arc::new(std::sync::Mutex::new(None)),
            stage: Arc::new(VideoStage::default()),
            screen_audio: Arc::new(tokio::sync::Mutex::new(None)),
            watched: Arc::default(),
            auto_subscribe: config.auto_subscribe,
        };

        if let Err(err) = session.audio.start_playback() {
            tracing::warn!(error = %err, "воспроизведение не открылось");
        }
        session.audio.set_deafened(config.start_deafened);

        for participant in session.room.remote_participants().into_values() {
            for publication in participant.track_publications().into_values() {
                session.apply_subscription(&participant, &publication);
            }
        }

        let pump = session.clone();
        tokio::spawn(async move {
            while let Some(event) = room_events.recv().await {
                pump.handle_room_event(event).await;
            }
            debug!("поток событий LiveKit завершён");
        });

        if config.auto_publish_mic
            && let Err(err) = session.publish_mic().await
        {
            // Без микрофона можно слушать других: не рвём соединение.
            tracing::warn!(error = %err, "микрофон недоступен, работаем только на приём");
            *session
                .mic_error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(err.to_string());
        }
        if config.start_deafened {
            session.set_deafened(true).await?;
        }
        Ok(session)
    }

    /// Identity локального участника.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Причина, по которой микрофон не заработал (`None` — всё в порядке).
    #[must_use]
    pub fn mic_error(&self) -> Option<String> {
        self.mic_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Текущий снимок комнаты.
    pub async fn snapshot(&self) -> RoomSnapshot {
        self.snapshot.read().await.clone()
    }

    /// Подписка на события.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<VoiceEvent> {
        self.events.subscribe()
    }

    /// Публикует микрофон, если ещё не опубликован.
    pub async fn publish_mic(&self) -> VoiceResult<()> {
        if self.mic_publication.read().await.is_some() {
            return Ok(());
        }
        self.audio.start()?;
        let track = self.audio.create_mic_track("mic")?;
        let options = TrackPublishOptions {
            source: TrackSource::Microphone,
            ..Default::default()
        };
        let publication = self
            .room
            .local_participant()
            .publish_track(LocalTrack::Audio(track), options)
            .await
            .map_err(VoiceError::LiveKit)?;
        if *self.muted.read().await {
            publication.mute();
        }
        *self.mic_publication.write().await = Some(publication);
        info!("микрофон опубликован");
        Ok(())
    }

    /// Снимает микрофон с публикации.
    pub async fn unpublish_mic(&self) -> VoiceResult<()> {
        let publication = self.mic_publication.write().await.take();
        let Some(publication) = publication else {
            return Ok(());
        };
        self.room
            .local_participant()
            .unpublish_track(&publication.sid())
            .await
            .map_err(VoiceError::LiveKit)?;
        self.audio.release_mic_track();
        self.audio.stop()?;
        Ok(())
    }

    /// Включает или выключает микрофон.
    pub async fn set_muted(&self, muted: bool) -> VoiceResult<()> {
        *self.muted.write().await = muted;
        if let Some(publication) = self.mic_publication.read().await.as_ref() {
            if muted {
                publication.mute();
            } else {
                publication.unmute();
            }
        }
        self.update_local(|p| p.muted = muted).await;
        Ok(())
    }

    /// Включён ли микрофон.
    pub async fn is_muted(&self) -> bool {
        *self.muted.read().await
    }

    /// Переключает микрофон.
    pub async fn toggle_mute(&self) -> VoiceResult<bool> {
        let next = !self.is_muted().await;
        self.set_muted(next).await?;
        Ok(next)
    }

    /// Оглушает или возвращает звук: при оглушении отключается подписка
    /// на все удалённые аудиотреки.
    pub async fn set_deafened(&self, deafened: bool) -> VoiceResult<()> {
        *self.deafened.write().await = deafened;
        self.audio.set_deafened(deafened);
        // Как utils/voice.ts оригинала: остальные узнают о выключенном звуке.
        self.send_data(&DataMessage::Deafen { deafened }).await;
        for participant in self.room.remote_participants().into_values() {
            for publication in participant.track_publications().into_values() {
                if publication.kind() == TrackKindLk::Audio {
                    publication.set_enabled(!deafened);
                }
            }
        }
        self.update_local(|p| p.deafened = deafened).await;
        if deafened {
            self.set_muted(true).await?;
        }
        Ok(())
    }

    /// Оглушён ли звук.
    pub async fn is_deafened(&self) -> bool {
        *self.deafened.read().await
    }

    /// Устанавливает громкость микрофона участника (0 — локально заглушён).
    ///
    /// Принимает identity или id пользователя: громкость хранится по id.
    pub async fn set_volume(&self, identity: &str, volume: Volume) -> VoiceResult<()> {
        self.volumes.write().await.set(identity, volume);
        self.audio
            .set_volume(user_id_of(identity), SourceKind::Voice, volume);
        Ok(())
    }

    /// Громкость звука трансляции участника.
    pub fn set_stream_volume(&self, identity: &str, volume: Volume) {
        self.audio
            .set_volume(user_id_of(identity), SourceKind::Screen, volume);
    }

    /// Звуковой движок сессии (общая громкость, приглушение и т. п.).
    #[must_use]
    pub fn audio(&self) -> &AudioEngine {
        &self.audio
    }

    /// Громкость участника.
    pub async fn volume(&self, identity: &str) -> Volume {
        self.volumes.read().await.get(identity)
    }

    /// Переключает микрофон на другое устройство.
    pub fn switch_input(&self, device_id: &str) -> VoiceResult<()> {
        self.audio.set_input(device_id)
    }

    /// Переключает устройство воспроизведения.
    pub fn switch_output(&self, device_id: &str) -> VoiceResult<()> {
        self.audio.set_output(device_id)
    }

    /// Отправляет сообщение в канал данных (надёжно, всем участникам).
    async fn send_data(&self, message: &DataMessage) {
        let packet = DataPacket {
            payload: message.encode(),
            reliable: true,
            ..DataPacket::default()
        };
        if let Err(err) = self.room.local_participant().publish_data(packet).await {
            debug!(error = %err, "сообщение канала данных не отправлено");
        }
    }

    /// Видеопотоки звонка (свои — как предпросмотр).
    #[must_use]
    pub fn video_feeds(&self) -> Vec<VideoFeed> {
        self.stage.feeds()
    }

    /// Identity локального участника в LiveKit (`userId:имя`).
    #[must_use]
    pub fn local_identity(&self) -> String {
        self.room.local_participant().identity().to_string()
    }

    /// Начинает демонстрацию экрана.
    ///
    /// # Errors
    /// Источник недоступен, неверные настройки или ошибка публикации.
    pub async fn start_screen_share(
        &self,
        source: ScreenSource,
        capture: CaptureConfig,
        publish: VideoPublishConfig,
    ) -> VoiceResult<()> {
        capture.validate()?;
        let size = (capture.max_width, capture.max_height);
        let (handle, frames) = ScreenCapture::start(&source, capture)?;
        self.stage
            .start_local(
                &self.room,
                &self.local_identity(),
                LocalCapture::Screen(handle),
                frames,
                size,
                &publish,
            )
            .await
    }

    /// Останавливает демонстрацию экрана и сообщает зрителям, что она окончена.
    ///
    /// # Errors
    /// Ошибка LiveKit при снятии публикации.
    pub async fn stop_screen_share(&self) -> VoiceResult<()> {
        self.stop_screen_audio().await?;
        if self
            .stage
            .stop_local(&self.room, TrackKind::ScreenShare)
            .await?
        {
            self.send_data(&DataMessage::StreamStop).await;
        }
        Ok(())
    }

    /// Начинает транслировать системный звук вместе с экраном
    /// (`ScreenshareAudio`, стерео 48 кГц, без обработки голоса).
    ///
    /// # Errors
    /// Звуковой сервер недоступен или ошибка публикации LiveKit.
    pub async fn start_screen_audio(&self) -> VoiceResult<()> {
        self.stop_screen_audio().await?;
        let (capture, mut frames) = tokio::task::spawn_blocking(SystemAudio::start)
            .await
            .map_err(|err| VoiceError::Device(err.to_string()))??;
        let source = NativeAudioSource::new(
            AudioSourceOptions {
                echo_cancellation: false,
                noise_suppression: false,
                auto_gain_control: false,
            },
            sysaudio::RATE,
            sysaudio::CHANNELS,
            100,
        );
        let track = LocalAudioTrack::create_audio_track(
            "screen_audio",
            RtcAudioSource::Native(source.clone()),
        );
        let options = TrackPublishOptions {
            source: TrackSource::ScreenshareAudio,
            // musicHighQualityStereo оригинала.
            audio_encoding: Some(AudioEncoding {
                max_bitrate: 128_000,
            }),
            dtx: false,
            red: false,
            ..Default::default()
        };
        let publication = self
            .room
            .local_participant()
            .publish_track(LocalTrack::Audio(track), options)
            .await
            .map_err(VoiceError::LiveKit)?;
        let forward = tokio::spawn(async move {
            while let Some(samples) = frames.recv().await {
                let frame = AudioFrame {
                    data: samples.into(),
                    sample_rate: sysaudio::RATE,
                    num_channels: sysaudio::CHANNELS,
                    #[allow(clippy::cast_possible_truncation, reason = "480 сэмплов")]
                    samples_per_channel: sysaudio::FRAME as u32,
                };
                if let Err(err) = source.capture_frame(&frame).await {
                    debug!(error = %err.message, "кадр системного звука не принят");
                }
            }
        });
        *self.screen_audio.lock().await = Some(ScreenAudio {
            capture,
            publication,
            forward,
        });
        info!("системный звук транслируется");
        Ok(())
    }

    /// Прекращает трансляцию системного звука.
    ///
    /// # Errors
    /// Ошибка LiveKit при снятии публикации.
    pub async fn stop_screen_audio(&self) -> VoiceResult<()> {
        let Some(audio) = self.screen_audio.lock().await.take() else {
            return Ok(());
        };
        audio.forward.abort();
        let sid = audio.publication.sid();
        // Захват останавливается в своём потоке: не держим им рантайм.
        tokio::task::spawn_blocking(move || drop(audio.capture))
            .await
            .map_err(|err| VoiceError::Device(err.to_string()))?;
        self.room
            .local_participant()
            .unpublish_track(&sid)
            .await
            .map_err(VoiceError::LiveKit)?;
        Ok(())
    }

    /// Идёт ли трансляция системного звука.
    pub async fn is_screen_audio_on(&self) -> bool {
        self.screen_audio.lock().await.is_some()
    }

    /// Идёт ли наша демонстрация экрана.
    pub async fn is_screen_sharing(&self) -> bool {
        self.stage.is_local_running(TrackKind::ScreenShare).await
    }

    /// Включает камеру.
    ///
    /// # Errors
    /// Камера не открылась, сборка без камеры или ошибка публикации.
    pub async fn start_camera(
        &self,
        index: u32,
        config: CameraConfig,
        publish: VideoPublishConfig,
    ) -> VoiceResult<()> {
        let (handle, frames) = start_camera(index, config)?;
        let size = (config.width.max(2) & !1, config.height.max(2) & !1);
        self.stage
            .start_local(
                &self.room,
                &self.local_identity(),
                LocalCapture::Camera(handle),
                frames,
                size,
                &publish,
            )
            .await
    }

    /// Выключает камеру.
    ///
    /// # Errors
    /// Ошибка LiveKit при снятии публикации.
    pub async fn stop_camera(&self) -> VoiceResult<()> {
        self.stage.stop_local(&self.room, TrackKind::Video).await?;
        Ok(())
    }

    /// Включена ли наша камера.
    pub async fn is_camera_on(&self) -> bool {
        self.stage.is_local_running(TrackKind::Video).await
    }

    /// Сообщает ведущему, что мы смотрим (или нет) его трансляцию.
    pub async fn set_watching(&self, sharer_identity: &str, watching: bool) {
        {
            let mut watched = self
                .watched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if watching {
                watched.insert(sharer_identity.to_owned());
            } else {
                watched.remove(sharer_identity);
            }
        }
        let sharer = self
            .room
            .remote_participants()
            .into_values()
            .find(|participant| participant.identity().as_str() == sharer_identity);
        if let Some(sharer) = sharer {
            for publication in sharer.track_publications().into_values() {
                self.apply_subscription(&sharer, &publication);
            }
        }
        let message = DataMessage::watch(user_id_of(sharer_identity), sharer_identity, watching);
        self.send_data(&message).await;
    }

    /// Кто смотрит нашу трансляцию (identity зрителей).
    #[must_use]
    pub fn my_watchers(&self) -> Vec<String> {
        self.stage.watchers_of(&self.local_identity())
    }

    async fn on_data(&self, payload: &[u8], participant: Option<&RemoteParticipant>) {
        let (Some(message), Some(participant)) = (DataMessage::decode(payload), participant) else {
            return;
        };
        let sender = participant.identity().to_string();
        match message {
            DataMessage::StreamWatch {
                target,
                target_identity,
                watching,
            } => {
                let mut identities: Vec<String> = self
                    .room
                    .remote_participants()
                    .keys()
                    .map(ToString::to_string)
                    .collect();
                identities.push(self.local_identity());
                if let Some(sharer) = watch_key(
                    &target,
                    target_identity.as_deref(),
                    identities.iter().map(String::as_str),
                ) {
                    self.stage.record_watch(sharer, &sender, watching);
                }
            }
            DataMessage::Deafen { deafened } => {
                let _ = self
                    .snapshot
                    .write()
                    .await
                    .update(&sender, |p| p.deafened = deafened);
            }
            DataMessage::StreamStop => self.stage.detach(&sender, TrackKind::ScreenShare),
            DataMessage::StreamRepublish | DataMessage::StreamResume => {}
        }
    }

    /// Отключается от комнаты.
    pub async fn disconnect(&self) -> VoiceResult<()> {
        let _ = self.stop_screen_share().await;
        let _ = self.stop_camera().await;
        self.stage.clear();
        self.audio.release_mic_track();
        self.audio.stop().ok();
        self.audio.stop_playback();
        self.room.close().await.map_err(VoiceError::LiveKit)?;
        self.snapshot.write().await.state = RoomState::Disconnected;
        Ok(())
    }

    async fn update_local<F: FnOnce(&mut ParticipantInfo)>(&self, f: F) {
        let mut snapshot = self.snapshot.write().await;
        if let Some(local) = snapshot.participants.get_mut(&self.identity) {
            f(local);
        }
    }

    async fn handle_room_event(&self, event: RoomEvent) {
        match event {
            RoomEvent::ParticipantConnected(participant)
            | RoomEvent::ParticipantActive(participant) => {
                self.on_participant_joined(&participant).await;
            }
            RoomEvent::ParticipantDisconnected(participant) => {
                self.on_participant_left(&participant).await;
            }
            RoomEvent::ActiveSpeakersChanged { speakers } => {
                self.on_active_speakers(&speakers).await;
            }
            RoomEvent::TrackPublished {
                publication,
                participant,
            } => self.apply_subscription(&participant, &publication),
            RoomEvent::TrackUnpublished {
                publication,
                participant,
            } => {
                if is_stream(publication.source()) {
                    self.stage
                        .detach(participant.identity().as_str(), TrackKind::ScreenShare);
                }
            }
            RoomEvent::TrackSubscribed {
                track,
                publication,
                participant,
            } => {
                self.on_track_subscribed(&track, &publication, &participant)
                    .await;
            }
            RoomEvent::TrackUnsubscribed {
                publication,
                participant,
                ..
            } => {
                self.on_track_unsubscribed(&publication, &participant);
            }
            RoomEvent::TrackMuted {
                participant,
                publication,
            } => {
                self.on_track_mute_changed(&participant, &publication, true)
                    .await;
            }
            RoomEvent::TrackUnmuted {
                participant,
                publication,
            } => {
                self.on_track_mute_changed(&participant, &publication, false)
                    .await;
            }
            RoomEvent::DataReceived {
                payload,
                participant,
                ..
            } => {
                self.on_data(&payload, participant.as_ref()).await;
            }
            RoomEvent::ConnectionStateChanged(state) => {
                debug!(?state, "состояние соединения изменилось");
            }
            RoomEvent::Connected { .. } => {
                self.on_connected().await;
            }
            RoomEvent::Reconnecting => {
                self.set_state(RoomState::Reconnecting).await;
                self.emit(VoiceEvent::Reconnecting);
            }
            RoomEvent::Reconnected => {
                self.set_state(RoomState::Connected).await;
                self.emit(VoiceEvent::Reconnected);
            }
            RoomEvent::Disconnected { reason } => {
                self.set_state(RoomState::Disconnected).await;
                self.emit(VoiceEvent::Disconnected {
                    reason: format!("{reason:?}"),
                });
            }
            other => {
                debug!(event = ?std::mem::discriminant(&other), "событие LiveKit проигнорировано");
            }
        }
    }

    async fn on_track_subscribed(
        &self,
        track: &RemoteTrack,
        publication: &RemoteTrackPublication,
        participant: &RemoteParticipant,
    ) {
        let identity = participant.identity().to_string();
        let Some(kind) = map_track_kind(publication.kind(), publication.source()) else {
            return;
        };
        self.emit(VoiceEvent::TrackSubscribed {
            identity: identity.clone(),
            sid: publication.sid().to_string(),
            kind,
        });
        if let RemoteTrack::Video(video) = track {
            self.stage.attach_remote(&identity, kind, video);
        }
        if let (Some(source), RemoteTrack::Audio(audio)) = (mix_source(kind), track) {
            let deafened = *self.deafened.read().await;
            publication.set_enabled(!deafened);
            self.audio
                .attach_remote(user_id_of(&identity), source, audio.rtc_track());
        }
    }

    fn on_track_unsubscribed(
        &self,
        publication: &RemoteTrackPublication,
        participant: &RemoteParticipant,
    ) {
        if let Some(kind) = map_track_kind(publication.kind(), publication.source()) {
            if let Some(source) = mix_source(kind) {
                self.audio
                    .detach_remote(user_id_of(participant.identity().as_str()), source);
            } else if kind == TrackKind::ScreenShare
                && participant
                    .track_publications()
                    .contains_key(&publication.sid())
            {
                // Перестали смотреть — плитка остаётся, чтобы можно было вернуться.
                self.stage
                    .park_remote(participant.identity().as_str(), kind);
            } else {
                self.stage.detach(participant.identity().as_str(), kind);
            }
            self.emit(VoiceEvent::TrackUnsubscribed {
                identity: participant.identity().to_string(),
                sid: publication.sid().to_string(),
                kind,
            });
        }
    }

    async fn on_track_mute_changed(
        &self,
        participant: &Participant,
        publication: &TrackPublication,
        muted: bool,
    ) {
        if publication.kind() != TrackKindLk::Audio {
            return;
        }
        let identity = participant.identity().to_string();
        let _ = self
            .snapshot
            .write()
            .await
            .update(&identity, |p| p.muted = muted);
        self.emit(VoiceEvent::MuteChanged { identity, muted });
    }

    async fn set_state(&self, state: RoomState) {
        self.snapshot.write().await.state = state;
    }

    async fn on_participant_joined(&self, participant: &RemoteParticipant) {
        let info = remote_info(participant);
        let identity = info.identity.clone();
        let name = info.name.clone();
        self.snapshot.write().await.upsert(info);
        self.emit(VoiceEvent::ParticipantJoined { identity, name });
    }

    async fn on_participant_left(&self, participant: &RemoteParticipant) {
        let identity = participant.identity().to_string();
        self.snapshot.write().await.remove(&identity);
        self.volumes.write().await.forget(&identity);
        self.stage.forget_participant(&identity);
        self.audio
            .detach_remote(user_id_of(&identity), SourceKind::Voice);
        self.audio
            .detach_remote(user_id_of(&identity), SourceKind::Screen);
        self.emit(VoiceEvent::ParticipantLeft { identity });
    }

    async fn on_active_speakers(&self, speakers: &[Participant]) {
        let identities: Vec<String> = speakers.iter().map(|p| p.identity().to_string()).collect();
        let me = self.room.local_participant().identity().to_string();
        self.audio
            .set_someone_speaking(identities.iter().any(|id| *id != me));
        {
            let mut snapshot = self.snapshot.write().await;
            for info in snapshot.participants.values_mut() {
                info.speaking = identities.contains(&info.identity);
            }
        }
        self.emit(VoiceEvent::ActiveSpeakers { identities });
    }

    /// Подписка на трек: голос и камеры — всегда (если включено),
    /// трансляции и их звук — только у тех, кого смотрим.
    fn apply_subscription(
        &self,
        participant: &RemoteParticipant,
        publication: &RemoteTrackPublication,
    ) {
        let identity = participant.identity().to_string();
        let source = publication.source();
        let wanted = if is_stream(source) {
            if source == TrackSource::Screenshare {
                self.stage
                    .announce_remote(&identity, TrackKind::ScreenShare);
            }
            self.watched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&identity)
        } else {
            self.auto_subscribe
        };
        if publication.is_subscribed() != wanted {
            publication.set_subscribed(wanted);
        }
    }

    async fn on_connected(&self) {
        let room = {
            let mut snapshot = self.snapshot.write().await;
            snapshot.state = RoomState::Connected;
            snapshot.room.clone()
        };
        self.emit(VoiceEvent::Connected { room });
    }

    fn emit(&self, event: VoiceEvent) {
        if self.events.send(event).is_err() {
            debug!("нет подписчиков на голосовые события");
        }
    }
}

use livekit::prelude::TrackKind as TrackKindLk;

fn remote_info(participant: &RemoteParticipant) -> ParticipantInfo {
    let mut info = ParticipantInfo {
        identity: participant.identity().to_string(),
        name: participant.name(),
        speaking: participant.is_speaking(),
        audio_level: 0.0,
        muted: false,
        deafened: false,
        is_local: false,
    };
    info.set_audio_level(participant.audio_level());
    info.muted = participant
        .track_publications()
        .values()
        .any(|p| p.kind() == TrackKindLk::Audio && p.is_muted());
    info
}

/// Приводит громкость в порядок при выходе участника.
#[must_use]
pub fn cleanup_volume(volumes: &mut VolumeMap, identity: &str) -> Option<Volume> {
    volumes.forget(identity)
}

/// Трансляция или её звук: принимаются только у тех, кого смотрим.
fn is_stream(source: TrackSource) -> bool {
    matches!(
        source,
        TrackSource::Screenshare | TrackSource::ScreenshareAudio
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_removes_entry() {
        let mut map = VolumeMap::default();
        map.set("u1", Volume::new(40));
        assert_eq!(cleanup_volume(&mut map, "u1"), Some(Volume::new(40)));
        assert!(map.is_empty());
    }

    #[test]
    fn missing_participant_is_not_an_error_for_update() {
        let mut snapshot = RoomSnapshot::default();
        let err = snapshot.update("nobody", |_| {}).unwrap_err();
        assert!(matches!(err, VoiceError::ParticipantNotFound(_)));
    }
}
