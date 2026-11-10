//! Медиа-слой голоса.
//!
//! С фичей `voice` работает через `bs-voice` (LiveKit); без неё — заглушка с тем
//! же интерфейсом, которая лишь сообщает, что звук недоступен. Присутствие в
//! канале и флаги микрофона ведёт `voice.rs` через WebSocket независимо от этого.

use std::collections::HashSet;

/// Состояние медиа-соединения.
#[cfg_attr(
    not(feature = "voice"),
    allow(dead_code, reason = "строится только с LiveKit")
)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum MediaStatus {
    /// Не подключены.
    #[default]
    Idle,
    /// Клиент собран без поддержки LiveKit.
    #[cfg_attr(
        feature = "voice",
        allow(dead_code, reason = "строится только заглушкой")
    )]
    Unavailable,
    /// Идёт подключение.
    Connecting,
    /// Звук идёт.
    Live,
    /// Слушаем других, но микрофон не заработал (причина внутри).
    #[cfg_attr(
        not(feature = "voice"),
        allow(dead_code, reason = "строится только с LiveKit")
    )]
    ListenOnly(String),
    /// Не удалось или соединение оборвалось.
    Failed(String),
}

impl MediaStatus {
    /// Соединение с комнатой установлено (микрофон может не работать).
    pub(crate) const fn is_live(&self) -> bool {
        matches!(self, Self::Live | Self::ListenOnly(_))
    }
}

/// Направление устройства.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceKind {
    /// Микрофон.
    Input,
    /// Колонки или наушники.
    Output,
}

/// Аудиоустройство.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Device {
    /// Идентификатор.
    pub(crate) id: String,
    /// Название.
    pub(crate) name: String,
}

/// Настройки голоса пользователя.
#[allow(clippy::struct_excessive_bools, reason = "независимые переключатели")]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct VoicePrefs {
    /// Выбранный микрофон; `None` — системный.
    pub(crate) input: Option<String>,
    /// Выбранное устройство воспроизведения; `None` — системное.
    pub(crate) output: Option<String>,
    /// Эхоподавление.
    pub(crate) echo_cancellation: bool,
    /// Шумоподавление.
    pub(crate) noise_suppression: bool,
    /// Автоматическая регулировка усиления.
    pub(crate) auto_gain_control: bool,
    /// Громкость микрофона, 0–200 (100 — без изменений).
    pub(crate) input_volume: u16,
    /// Громкость собеседников, 0–200 (100 — без изменений).
    pub(crate) output_volume: u16,
}

impl Default for VoicePrefs {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            echo_cancellation: true,
            noise_suppression: true,
            auto_gain_control: true,
            input_volume: 100,
            output_volume: 100,
        }
    }
}

/// Параметры подключения к комнате.
#[cfg_attr(
    not(feature = "voice"),
    allow(dead_code, reason = "читается только с LiveKit")
)]
#[derive(Debug, Clone)]
pub(crate) struct ConnectRequest {
    /// Адрес LiveKit (`wss://…`).
    pub(crate) url: String,
    /// JWT от сервера Backspace.
    pub(crate) token: String,
    /// Идентичность в комнате — id пользователя.
    pub(crate) identity: String,
    /// Отображаемое имя.
    pub(crate) name: String,
    /// Микрофон выключен на старте.
    pub(crate) muted: bool,
    /// Звук выключен на старте.
    pub(crate) deafened: bool,
}

#[cfg(feature = "voice")]
pub(crate) use real::MediaEngine;
#[cfg(not(feature = "voice"))]
pub(crate) use stub::MediaEngine;

/// Список говорящих (для подсветки в сайдбаре).
pub(crate) type Speaking = HashSet<String>;

#[cfg(not(feature = "voice"))]
mod stub {
    use super::{ConnectRequest, Device, DeviceKind, MediaStatus, Speaking, VoicePrefs};

    /// Заглушка: звука нет.
    #[derive(Debug, Default)]
    pub(crate) struct MediaEngine;

    #[allow(
        clippy::unused_self,
        clippy::needless_pass_by_ref_mut,
        reason = "тот же интерфейс, что у настоящего движка"
    )]
    impl MediaEngine {
        pub(crate) fn new(_handle: tokio::runtime::Handle) -> Self {
            Self
        }

        pub(crate) const fn available() -> bool {
            false
        }

        pub(crate) fn devices(&mut self, _kind: DeviceKind, _prefs: &VoicePrefs) -> Vec<Device> {
            Vec::new()
        }

        pub(crate) fn apply_prefs(&mut self, _prefs: &VoicePrefs) {}

        pub(crate) fn connect(&mut self, _request: ConnectRequest, _prefs: &VoicePrefs) {}

        pub(crate) fn set_flags(&self, _muted: bool, _deafened: bool) {}

        pub(crate) fn disconnect(&mut self) {}

        pub(crate) fn status(&self) -> MediaStatus {
            MediaStatus::Unavailable
        }

        pub(crate) fn speaking(&self) -> Speaking {
            Speaking::new()
        }

        pub(crate) fn start_mic_test(&mut self, _prefs: &VoicePrefs) -> Result<(), String> {
            Err("сборка без голоса".to_owned())
        }

        pub(crate) fn stop_mic_test(&mut self) {}

        pub(crate) fn mic_level(&self) -> Option<f32> {
            None
        }
    }
}

#[cfg(feature = "voice")]
mod real {
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use bs_voice::{
        AudioEngine, AudioProcessingSettings, MicMonitor, VoiceConfig, VoiceEvent, VoiceSession,
        device::DeviceKind as LkKind,
    };
    use tokio::runtime::Handle;
    use tokio::sync::broadcast::error::RecvError;
    use tokio::task::JoinHandle;

    use super::{ConnectRequest, Device, DeviceKind, MediaStatus, Speaking, VoicePrefs};

    #[derive(Default)]
    struct Shared {
        status: MediaStatus,
        speaking: Speaking,
        session: Option<VoiceSession>,
        generation: u64,
    }

    fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
        shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn processing(prefs: &VoicePrefs) -> AudioProcessingSettings {
        AudioProcessingSettings {
            echo_cancellation: prefs.echo_cancellation,
            noise_suppression: prefs.noise_suppression,
            auto_gain_control: prefs.auto_gain_control,
            ..AudioProcessingSettings::default()
        }
    }

    /// Медиа-движок поверх `bs-voice`.
    pub(crate) struct MediaEngine {
        handle: Handle,
        shared: Arc<Mutex<Shared>>,
        task: Option<JoinHandle<()>>,
        audio: Option<AudioEngine>,
        /// Идущая проверка микрофона в настройках.
        mic_test: Option<MicMonitor>,
    }

    impl std::fmt::Debug for MediaEngine {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MediaEngine")
                .field("status", &lock(&self.shared).status)
                .finish_non_exhaustive()
        }
    }

    impl MediaEngine {
        pub(crate) fn new(handle: Handle) -> Self {
            Self {
                handle,
                shared: Arc::new(Mutex::new(Shared::default())),
                task: None,
                audio: None,
                mic_test: None,
            }
        }

        /// Запускает проверку микрофона (уровень сигнала без звонка).
        pub(crate) fn start_mic_test(&mut self, prefs: &VoicePrefs) -> Result<(), String> {
            let audio = self.audio(prefs)?;
            self.apply_prefs(prefs);
            let monitor = MicMonitor::start(&audio, &self.handle).map_err(|err| err.to_string())?;
            self.mic_test = Some(monitor);
            Ok(())
        }

        /// Останавливает проверку микрофона.
        pub(crate) fn stop_mic_test(&mut self) {
            self.mic_test = None;
        }

        /// Уровень сигнала идущей проверки, 0.0–1.0.
        pub(crate) fn mic_level(&self) -> Option<f32> {
            self.mic_test.as_ref().map(MicMonitor::level)
        }

        #[allow(dead_code, reason = "для проверки возможностей сборки")]
        pub(crate) const fn available() -> bool {
            true
        }

        /// Аудиодвижок создаётся при первой необходимости и живёт дальше.
        fn audio(&mut self, prefs: &VoicePrefs) -> Result<AudioEngine, String> {
            if let Some(audio) = &self.audio {
                return Ok(audio.clone());
            }
            let audio = AudioEngine::new(processing(prefs)).map_err(|err| err.to_string())?;
            self.audio = Some(audio.clone());
            Ok(audio)
        }

        pub(crate) fn devices(&mut self, kind: DeviceKind, prefs: &VoicePrefs) -> Vec<Device> {
            let Ok(audio) = self.audio(prefs) else {
                return Vec::new();
            };
            let kind = match kind {
                DeviceKind::Input => LkKind::Input,
                DeviceKind::Output => LkKind::Output,
            };
            audio
                .devices(kind)
                .into_iter()
                .map(|d| Device {
                    id: d.id,
                    name: d.name,
                })
                .collect()
        }

        /// Применяет обработку звука и выбранные устройства к живому движку.
        pub(crate) fn apply_prefs(&self, prefs: &VoicePrefs) {
            let Some(audio) = &self.audio else {
                return;
            };
            if let Err(err) = audio.apply_processing(processing(prefs)) {
                tracing::warn!("не удалось применить обработку звука: {err}");
            }
            if let Some(id) = &prefs.input
                && let Err(err) = audio.set_input(id)
            {
                tracing::warn!("не удалось выбрать микрофон: {err}");
            }
            if let Some(id) = &prefs.output
                && let Err(err) = audio.set_output(id)
            {
                tracing::warn!("не удалось выбрать устройство воспроизведения: {err}");
            }
        }

        pub(crate) fn connect(&mut self, request: ConnectRequest, prefs: &VoicePrefs) {
            self.disconnect();
            // Проверка микрофона не должна остановить захват посреди звонка.
            self.mic_test = None;
            let audio = match self.audio(prefs) {
                Ok(audio) => audio,
                Err(err) => {
                    lock(&self.shared).status = MediaStatus::Failed(err);
                    return;
                }
            };
            self.apply_prefs(prefs);
            let generation = {
                let mut shared = lock(&self.shared);
                shared.generation += 1;
                shared.status = MediaStatus::Connecting;
                shared.generation
            };
            let config = VoiceConfig {
                auto_publish_mic: true,
                start_muted: request.muted,
                start_deafened: request.deafened,
                processing: processing(prefs),
                auto_subscribe: true,
            };
            let shared = Arc::clone(&self.shared);
            self.task = Some(self.handle.spawn(async move {
                let connected = VoiceSession::connect(
                    &request.url,
                    &request.token,
                    config,
                    audio,
                    request.identity,
                    request.name,
                )
                .await;
                let session = match connected {
                    Ok(session) => session,
                    Err(err) => {
                        {
                            let mut shared = lock(&shared);
                            if shared.generation == generation {
                                shared.status = MediaStatus::Failed(err.to_string());
                            }
                        }
                        return;
                    }
                };
                let stale = {
                    let mut shared = lock(&shared);
                    if shared.generation == generation {
                        shared.session = Some(session.clone());
                        shared.status = session
                            .mic_error()
                            .map_or(MediaStatus::Live, MediaStatus::ListenOnly);
                        false
                    } else {
                        true
                    }
                };
                if stale {
                    let _ = session.disconnect().await;
                    return;
                }
                let mut events = session.subscribe();
                loop {
                    match events.recv().await {
                        Ok(VoiceEvent::ActiveSpeakers { identities }) => {
                            let mut shared = lock(&shared);
                            if shared.generation == generation {
                                shared.speaking = identities.into_iter().collect();
                            }
                        }
                        Ok(VoiceEvent::Disconnected { reason }) => {
                            {
                                let mut shared = lock(&shared);
                                if shared.generation == generation {
                                    shared.status = MediaStatus::Failed(reason);
                                    shared.speaking.clear();
                                    shared.session = None;
                                }
                            }
                            break;
                        }
                        Ok(_) | Err(RecvError::Lagged(_)) => {}
                        Err(RecvError::Closed) => break,
                    }
                }
            }));
        }

        /// Передаёт флаги микрофона и звука живой сессии.
        pub(crate) fn set_flags(&self, muted: bool, deafened: bool) {
            let session = lock(&self.shared).session.clone();
            if let Some(session) = session {
                self.handle.spawn(async move {
                    let _ = session.set_deafened(deafened).await;
                    let _ = session.set_muted(muted).await;
                });
            }
        }

        pub(crate) fn disconnect(&mut self) {
            if let Some(task) = self.task.take() {
                task.abort();
            }
            let session = {
                let mut shared = lock(&self.shared);
                shared.generation += 1;
                shared.status = MediaStatus::Idle;
                shared.speaking.clear();
                shared.session.take()
            };
            if let Some(session) = session {
                self.handle.spawn(async move {
                    let _ = session.disconnect().await;
                });
            }
        }

        pub(crate) fn status(&self) -> MediaStatus {
            lock(&self.shared).status.clone()
        }

        pub(crate) fn speaking(&self) -> Speaking {
            lock(&self.shared).speaking.clone()
        }
    }
}
