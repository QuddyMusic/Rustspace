//! Голосовая сессия: подключение к комнате LiveKit и обработка событий.

use std::sync::Arc;

use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use tokio::sync::{RwLock, broadcast};
use tracing::{debug, info};

use crate::audio::{AudioEngine, map_track_kind, mix_source};
use crate::config::VoiceConfig;
use crate::error::{VoiceError, VoiceResult};
use crate::events::VoiceEvent;
use crate::mixer::SourceKind;
use crate::state::{ParticipantInfo, RoomSnapshot, RoomState, user_id_of};
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

        let (room, mut room_events) = Room::connect(url, token, RoomOptions::default())
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
        };

        if let Err(err) = session.audio.start_playback() {
            tracing::warn!(error = %err, "воспроизведение не открылось");
        }
        session.audio.set_deafened(config.start_deafened);

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

    /// Отключается от комнаты.
    pub async fn disconnect(&self) -> VoiceResult<()> {
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
