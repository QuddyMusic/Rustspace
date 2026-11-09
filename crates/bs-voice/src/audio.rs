//! Аудио: микрофон, обработка, устройства, воспроизведение.

use std::sync::Arc;

use livekit::prelude::*;
use livekit::{AudioProcessingOptions, PlatformAudio};
use tracing::{debug, info, warn};

use crate::config::AudioProcessingSettings;
use crate::device::{AudioDevice, DeviceKind};
use crate::error::{VoiceError, VoiceResult};
use crate::state::TrackKind;

/// Обёртка над `PlatformAudio` с безопасным жизненным циклом.
#[derive(Clone)]
pub struct AudioEngine {
    inner: Arc<PlatformAudio>,
}

impl std::fmt::Debug for AudioEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioEngine")
            .field("recording", &self.is_recording())
            .finish()
    }
}

impl AudioEngine {
    /// Создаёт движок и применяет настройки обработки звука.
    pub fn new(settings: AudioProcessingSettings) -> VoiceResult<Self> {
        let inner = PlatformAudio::new().map_err(VoiceError::Audio)?;
        let engine = Self {
            inner: Arc::new(inner),
        };
        engine.apply_processing(settings)?;
        Ok(engine)
    }

    /// Применяет AEC/NS/AGC. Ошибки не фатальны: логируем и продолжаем.
    pub fn apply_processing(&self, settings: AudioProcessingSettings) -> VoiceResult<()> {
        let options = AudioProcessingOptions {
            echo_cancellation: settings.echo_cancellation,
            noise_suppression: settings.noise_suppression,
            auto_gain_control: settings.auto_gain_control,
            prefer_hardware_processing: settings.prefer_hardware,
        };
        if let Err(e) = self.inner.configure_audio_processing(options) {
            warn!(error = %e, "не удалось применить обработку звука, работаем с дефолтом");
        }
        debug!(
            aec = ?self.inner.active_aec_type(),
            agc = ?self.inner.active_agc_type(),
            ns = ?self.inner.active_ns_type(),
            "активная обработка звука"
        );
        Ok(())
    }

    /// Доступна ли аппаратная обработка.
    #[must_use]
    pub fn hardware_capabilities(&self) -> HardwareCapabilities {
        HardwareCapabilities {
            aec: self.inner.is_hardware_aec_available(),
            agc: self.inner.is_hardware_agc_available(),
            ns: self.inner.is_hardware_ns_available(),
        }
    }

    /// Перечисляет устройства выбранного направления.
    #[must_use]
    pub fn devices(&self, kind: DeviceKind) -> Vec<AudioDevice> {
        match kind {
            DeviceKind::Input => self
                .inner
                .recording_devices()
                .map(|d| AudioDevice {
                    id: d.id.as_str().to_owned(),
                    name: d.name,
                    index: d.index,
                    kind: DeviceKind::Input,
                })
                .collect(),
            DeviceKind::Output => self
                .inner
                .playout_devices()
                .map(|d| AudioDevice {
                    id: d.id.as_str().to_owned(),
                    name: d.name,
                    index: d.index,
                    kind: DeviceKind::Output,
                })
                .collect(),
        }
    }

    /// Переключает микрофон по id.
    pub fn set_input(&self, device_id: &str) -> VoiceResult<()> {
        let id = RecordingDeviceId::from_unchecked_guid(device_id);
        self.inner
            .switch_recording_device(&id)
            .map_err(VoiceError::Audio)?;
        info!(device = device_id, "микрофон переключён");
        Ok(())
    }

    /// Переключает устройство воспроизведения по id.
    pub fn set_output(&self, device_id: &str) -> VoiceResult<()> {
        let id = PlayoutDeviceId::from_unchecked_guid(device_id);
        self.inner
            .switch_playout_device(&id)
            .map_err(VoiceError::Audio)?;
        info!(device = device_id, "устройство воспроизведения переключено");
        Ok(())
    }

    /// Начинает захват микрофона.
    pub fn start(&self) -> VoiceResult<()> {
        if self.inner.is_recording_initialized() {
            return Ok(());
        }
        let first = match self.inner.start_recording() {
            Ok(()) => return Ok(()),
            Err(err) => err,
        };
        // Системный микрофон «по умолчанию» иногда не открывается (PipeWire,
        // занятое устройство): перебираем остальные, пока какой-то не заведётся.
        let devices = self.devices(DeviceKind::Input);
        warn!(error = %first, devices = devices.len(), "захват не стартовал, пробуем другие микрофоны");
        if devices.is_empty() {
            return Err(VoiceError::NoMicrophone);
        }
        for device in &devices {
            debug!(device = %device.name, "пробуем микрофон");
            if self.set_input(&device.id).is_ok() && self.inner.start_recording().is_ok() {
                info!(device = %device.name, "захват запущен на запасном микрофоне");
                return Ok(());
            }
        }
        Err(VoiceError::Audio(first))
    }

    /// Останавливает захват.
    pub fn stop(&self) -> VoiceResult<()> {
        self.inner.stop_recording().map_err(VoiceError::Audio)
    }

    /// Идёт ли захват.
    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.inner.is_recording_initialized()
    }

    /// Источник для создания локального аудиотрека.
    #[must_use]
    pub fn rtc_source(&self) -> RtcAudioSource {
        self.inner.rtc_source()
    }

    /// Создаёт локальный микрофонный трек (без публикации).
    #[must_use]
    pub fn create_mic_track(&self, name: &str) -> LocalAudioTrack {
        LocalAudioTrack::create_audio_track(name, self.rtc_source())
    }
}

/// Какая аппаратная обработка доступна на устройстве.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareCapabilities {
    /// Эхоподавление.
    pub aec: bool,
    /// Автоматическая регулировка усиления.
    pub agc: bool,
    /// Шумоподавление.
    pub ns: bool,
}

/// Переводит `TrackSource`/`TrackKind` LiveKit в доменный тип.
#[must_use]
pub fn map_track_kind(kind: TrackKindLk, source: TrackSource) -> Option<TrackKind> {
    match source {
        TrackSource::Microphone => Some(TrackKind::Audio),
        TrackSource::Camera => Some(TrackKind::Video),
        TrackSource::Screenshare => Some(TrackKind::ScreenShare),
        TrackSource::ScreenshareAudio => Some(TrackKind::ScreenShareAudio),
        TrackSource::Unknown => match kind {
            TrackKindLk::Audio => Some(TrackKind::Audio),
            TrackKindLk::Video => Some(TrackKind::Video),
        },
    }
}

/// Псевдоним, чтобы не путать доменный `TrackKind` с livekit-овским.
use livekit::prelude::TrackKind as TrackKindLk;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_sources() {
        assert_eq!(
            map_track_kind(TrackKindLk::Audio, TrackSource::Microphone),
            Some(TrackKind::Audio)
        );
        assert_eq!(
            map_track_kind(TrackKindLk::Video, TrackSource::Camera),
            Some(TrackKind::Video)
        );
        assert_eq!(
            map_track_kind(TrackKindLk::Video, TrackSource::Screenshare),
            Some(TrackKind::ScreenShare)
        );
        assert_eq!(
            map_track_kind(TrackKindLk::Audio, TrackSource::ScreenshareAudio),
            Some(TrackKind::ScreenShareAudio)
        );
    }

    #[test]
    fn falls_back_to_kind_for_unknown_source() {
        assert_eq!(
            map_track_kind(TrackKindLk::Audio, TrackSource::Unknown),
            Some(TrackKind::Audio)
        );
        assert_eq!(
            map_track_kind(TrackKindLk::Video, TrackSource::Unknown),
            Some(TrackKind::Video)
        );
    }
}
