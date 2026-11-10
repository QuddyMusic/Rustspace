//! Аудио: собственный тракт поверх cpal.
//!
//! Системный playout LiveKit (`PlatformAudio`) не даёт ни громкости
//! микрофона, ни громкости отдельных участников, а его локальный трек не
//! отдаёт PCM — индикатор проверки микрофона на нём молчит. Поэтому звук
//! идёт своим путём, как цепочка Web Audio в оригинале:
//!
//! ```text
//! микрофон (cpal) → 48 кГц моно → APM (эхо/шум/АРУ) → усиление 0–200 % → NativeAudioSource → комната
//! комната → NativeAudioStream каждого трека → Mixer (громкости) → cpal → колонки
//!                                                           └→ опорный сигнал для эхоподавления
//! ```
//!
//! Потоки cpal не `Send` на части платформ, поэтому живут в отдельном
//! потоке ввода-вывода; обработка (APM, отправка кадров) — задача tokio.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle as ThreadHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit::webrtc::native::apm::AudioProcessingModule;
use livekit::webrtc::prelude::RtcAudioTrack;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio_stream::StreamExt as _;
use tracing::{debug, info, warn};

use crate::config::AudioProcessingSettings;
use crate::device::{AudioDevice, DeviceKind};
use crate::error::{VoiceError, VoiceResult};
use crate::mixer::{FRAME_SAMPLES, Mixer, Resampler, SAMPLE_RATE, SourceKind, apply_gain, to_i16};
use crate::state::TrackKind;
use crate::volume::Volume;

/// Доля, на которую уровень микрофона падает за кадр без сигнала.
const LEVEL_DECAY: f32 = 0.9;
/// Оценка задержки «колонки → микрофон» для эхоподавления.
const ECHO_DELAY_MS: i32 = 40;
/// Частота тракта для API libwebrtc, где она знаковая.
const RATE_I32: i32 = 48_000;
/// Очередь `NativeAudioSource`, мс.
const SOURCE_QUEUE_MS: u32 = 100;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn store_f32(cell: &AtomicU32, value: f32) {
    cell.store(value.to_bits(), Ordering::Relaxed);
}

fn load_f32(cell: &AtomicU32) -> f32 {
    f32::from_bits(cell.load(Ordering::Relaxed))
}

/// Сообщения задаче обработки.
enum DspMsg {
    /// Микрофон, 48 кГц моно.
    Capture(Vec<f32>),
    /// То, что ушло в колонки, 48 кГц моно (опорный сигнал AEC).
    Render(Vec<f32>),
    Settings(AudioProcessingSettings),
    Sink(Option<NativeAudioSource>),
}

/// Команды потоку ввода-вывода.
enum IoCmd {
    OpenInput(Option<String>, std_mpsc::Sender<VoiceResult<()>>),
    CloseInput,
    OpenOutput(Option<String>, std_mpsc::Sender<VoiceResult<()>>),
    CloseOutput,
}

struct Shared {
    mixer: Mutex<Mixer>,
    input_gain: AtomicU32,
    level: AtomicU32,
    capturing: AtomicBool,
    playing: AtomicBool,
    dsp: Mutex<Option<UnboundedSender<DspMsg>>>,
    settings: Mutex<AudioProcessingSettings>,
    input_device: Mutex<Option<String>>,
    output_device: Mutex<Option<String>>,
    remotes: Mutex<HashMap<(String, SourceKind), JoinHandle<()>>>,
}

impl Shared {
    fn send(&self, msg: DspMsg) {
        if let Some(tx) = lock(&self.dsp).as_ref() {
            let _ = tx.send(msg);
        }
    }
}

struct IoThread {
    commands: Mutex<Option<std_mpsc::Sender<IoCmd>>>,
    thread: Mutex<Option<ThreadHandle<()>>>,
}

impl IoThread {
    fn command(&self, cmd: IoCmd) {
        if let Some(tx) = lock(&self.commands).as_ref() {
            let _ = tx.send(cmd);
        }
    }

    fn open(
        &self,
        make: impl FnOnce(std_mpsc::Sender<VoiceResult<()>>) -> IoCmd,
    ) -> VoiceResult<()> {
        let (tx, rx) = std_mpsc::channel();
        self.command(make(tx));
        rx.recv()
            .unwrap_or_else(|_| Err(VoiceError::Device("поток звука остановлен".into())))
    }
}

impl Drop for IoThread {
    fn drop(&mut self) {
        // Закрытый канал завершает поток, а с ним и потоки cpal.
        lock(&self.commands).take();
        let thread = lock(&self.thread).take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

/// Звуковой движок: микрофон, воспроизведение, громкости.
#[derive(Clone)]
pub struct AudioEngine {
    shared: Arc<Shared>,
    io: Arc<IoThread>,
}

impl std::fmt::Debug for AudioEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioEngine")
            .field("recording", &self.is_recording())
            .field("playing", &self.shared.playing.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl AudioEngine {
    /// Создаёт движок. Устройства открываются позже, по требованию.
    ///
    /// # Errors
    /// Не удалось запустить поток ввода-вывода.
    pub fn new(settings: AudioProcessingSettings) -> VoiceResult<Self> {
        let shared = Arc::new(Shared {
            mixer: Mutex::new(Mixer::default()),
            input_gain: AtomicU32::new(1.0_f32.to_bits()),
            level: AtomicU32::new(0),
            capturing: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            dsp: Mutex::new(None),
            settings: Mutex::new(settings),
            input_device: Mutex::new(None),
            output_device: Mutex::new(None),
            remotes: Mutex::new(HashMap::new()),
        });
        let (tx, rx) = std_mpsc::channel();
        let io_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("bs-audio-io".into())
            .spawn(move || io_loop(&rx, &io_shared))
            .map_err(|err| VoiceError::Device(err.to_string()))?;
        Ok(Self {
            shared,
            io: Arc::new(IoThread {
                commands: Mutex::new(Some(tx)),
                thread: Mutex::new(Some(thread)),
            }),
        })
    }

    /// Применяет эхоподавление, шумоподавление и АРУ.
    ///
    /// # Errors
    /// Сейчас не возвращает ошибок; сигнатура оставлена на будущее.
    pub fn apply_processing(&self, settings: AudioProcessingSettings) -> VoiceResult<()> {
        *lock(&self.shared.settings) = settings;
        self.shared.send(DspMsg::Settings(settings));
        Ok(())
    }

    /// Аппаратной обработки в собственном тракте нет: всё делает APM.
    #[must_use]
    pub const fn hardware_capabilities(&self) -> HardwareCapabilities {
        HardwareCapabilities {
            aec: false,
            agc: false,
            ns: false,
        }
    }

    /// Перечисляет устройства выбранного направления.
    #[must_use]
    pub fn devices(&self, kind: DeviceKind) -> Vec<AudioDevice> {
        let host = cpal::default_host();
        let devices = match kind {
            DeviceKind::Input => host.input_devices().map(Iterator::collect::<Vec<_>>),
            DeviceKind::Output => host.output_devices().map(Iterator::collect::<Vec<_>>),
        };
        let Ok(devices) = devices else {
            return Vec::new();
        };
        devices
            .iter()
            .enumerate()
            .filter_map(|(index, device)| {
                Some(AudioDevice {
                    id: device.id().ok()?.to_string(),
                    name: device.description().ok()?.name().to_owned(),
                    index,
                    kind,
                })
            })
            .collect()
    }

    /// Выбирает микрофон по id (`None` — системный по умолчанию).
    ///
    /// # Errors
    /// Идёт захват, а новое устройство не открылось.
    pub fn set_input(&self, device_id: &str) -> VoiceResult<()> {
        let wanted = (!device_id.is_empty()).then(|| device_id.to_owned());
        if *lock(&self.shared.input_device) == wanted {
            return Ok(());
        }
        lock(&self.shared.input_device).clone_from(&wanted);
        if self.is_recording() {
            self.io.open(|reply| IoCmd::OpenInput(wanted, reply))?;
        }
        info!(device = device_id, "микрофон выбран");
        Ok(())
    }

    /// Выбирает устройство воспроизведения по id.
    ///
    /// # Errors
    /// Идёт воспроизведение, а новое устройство не открылось.
    pub fn set_output(&self, device_id: &str) -> VoiceResult<()> {
        let wanted = (!device_id.is_empty()).then(|| device_id.to_owned());
        if *lock(&self.shared.output_device) == wanted {
            return Ok(());
        }
        lock(&self.shared.output_device).clone_from(&wanted);
        if self.shared.playing.load(Ordering::Relaxed) {
            self.io.open(|reply| IoCmd::OpenOutput(wanted, reply))?;
        }
        info!(device = device_id, "устройство воспроизведения выбрано");
        Ok(())
    }

    /// Запускает задачу обработки, если её ещё нет.
    fn ensure_dsp(&self) -> VoiceResult<()> {
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| VoiceError::Device("звук запущен вне tokio".into()))?;
        let settings = *lock(&self.shared.settings);
        let mut dsp = lock(&self.shared.dsp);
        if dsp.as_ref().is_some_and(|tx| !tx.is_closed()) {
            return Ok(());
        }
        let (tx, rx) = unbounded_channel();
        handle.spawn(dsp_loop(rx, Arc::clone(&self.shared), settings));
        *dsp = Some(tx);
        drop(dsp);
        Ok(())
    }

    /// Начинает захват микрофона. Вызывать внутри рантайма tokio.
    ///
    /// # Errors
    /// Микрофон не открылся или нет рантайма.
    pub fn start(&self) -> VoiceResult<()> {
        if self.is_recording() {
            return Ok(());
        }
        self.ensure_dsp()?;
        let device = lock(&self.shared.input_device).clone();
        self.io.open(|reply| IoCmd::OpenInput(device, reply))?;
        self.shared.capturing.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Останавливает захват.
    ///
    /// # Errors
    /// Сейчас не возвращает ошибок.
    pub fn stop(&self) -> VoiceResult<()> {
        self.shared.capturing.store(false, Ordering::Relaxed);
        self.io.command(IoCmd::CloseInput);
        store_f32(&self.shared.level, 0.0);
        Ok(())
    }

    /// Идёт ли захват.
    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.shared.capturing.load(Ordering::Relaxed)
    }

    /// Уровень микрофона после усиления, 0.0–1.0.
    #[must_use]
    pub fn level(&self) -> f32 {
        load_f32(&self.shared.level).clamp(0.0, 1.0)
    }

    /// Громкость микрофона, 0–200 %.
    pub fn set_input_volume(&self, volume: Volume) {
        store_f32(&self.shared.input_gain, volume.gain());
    }

    /// Создаёт трек микрофона; кадры в него идут, пока трек не отпущен.
    ///
    /// # Errors
    /// Нет рантайма tokio.
    pub fn create_mic_track(&self, name: &str) -> VoiceResult<LocalAudioTrack> {
        self.ensure_dsp()?;
        let source = NativeAudioSource::new(
            AudioSourceOptions::default(),
            SAMPLE_RATE,
            1,
            SOURCE_QUEUE_MS,
        );
        self.shared.send(DspMsg::Sink(Some(source.clone())));
        Ok(LocalAudioTrack::create_audio_track(
            name,
            RtcAudioSource::Native(source),
        ))
    }

    /// Перестаёт отдавать кадры в трек микрофона.
    pub fn release_mic_track(&self) {
        self.shared.send(DspMsg::Sink(None));
    }

    /// Открывает устройство воспроизведения.
    ///
    /// # Errors
    /// Устройство не открылось.
    pub fn start_playback(&self) -> VoiceResult<()> {
        if self.shared.playing.load(Ordering::Relaxed) {
            return Ok(());
        }
        let device = lock(&self.shared.output_device).clone();
        self.io.open(|reply| IoCmd::OpenOutput(device, reply))?;
        self.shared.playing.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Закрывает воспроизведение и отпускает все удалённые треки.
    pub fn stop_playback(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
        self.io.command(IoCmd::CloseOutput);
        for (key, task) in lock(&self.shared.remotes).drain() {
            task.abort();
            lock(&self.shared.mixer).remove(&key.0, key.1);
        }
    }

    /// Подключает удалённый аудиотрек к миксу. Вызывать внутри рантайма tokio.
    pub fn attach_remote(&self, identity: &str, kind: SourceKind, track: RtcAudioTrack) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            warn!("удалённый звук подключается вне tokio — пропускаем");
            return;
        };
        let shared = Arc::clone(&self.shared);
        let owner = identity.to_owned();
        let task = handle.spawn(async move {
            let mut stream = NativeAudioStream::new(track, RATE_I32, 1);
            while let Some(frame) = stream.next().await {
                lock(&shared.mixer).push(&owner, kind, &frame.data);
            }
        });
        let old = lock(&self.shared.remotes).insert((identity.to_owned(), kind), task);
        if let Some(old) = old {
            old.abort();
        }
    }

    /// Отключает удалённый трек от микса.
    pub fn detach_remote(&self, identity: &str, kind: SourceKind) {
        let task = lock(&self.shared.remotes).remove(&(identity.to_owned(), kind));
        if let Some(task) = task {
            task.abort();
        }
        lock(&self.shared.mixer).remove(identity, kind);
    }

    /// Громкость участника (микрофон или звук трансляции).
    pub fn set_volume(&self, identity: &str, kind: SourceKind, volume: Volume) {
        lock(&self.shared.mixer).set_volume(identity, kind, volume);
    }

    /// Общая громкость вывода, 0–200 %.
    pub fn set_output_volume(&self, volume: Volume) {
        lock(&self.shared.mixer).set_master(volume);
    }

    /// Оглушение.
    pub fn set_deafened(&self, deafened: bool) {
        lock(&self.shared.mixer).set_deafened(deafened);
    }

    /// Приглушение трансляций при разговоре: `None` — выключено.
    pub fn set_attenuation(&self, strength: Option<u8>) {
        lock(&self.shared.mixer).set_attenuation(strength);
    }

    /// Говорит ли кто-то из удалённых участников.
    pub fn set_someone_speaking(&self, speaking: bool) {
        lock(&self.shared.mixer).set_someone_speaking(speaking);
    }
}

fn make_apm(settings: AudioProcessingSettings) -> Option<AudioProcessingModule> {
    let any =
        settings.echo_cancellation || settings.noise_suppression || settings.auto_gain_control;
    any.then(|| {
        AudioProcessingModule::new(
            settings.echo_cancellation,
            settings.auto_gain_control,
            true,
            settings.noise_suppression,
        )
    })
}

/// Обработка микрофона: APM, усиление, уровень, отправка в трек.
async fn dsp_loop(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<DspMsg>,
    shared: Arc<Shared>,
    settings: AudioProcessingSettings,
) {
    let mut apm = make_apm(settings);
    let mut capture: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES * 4);
    let mut render: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES * 4);
    let mut sink: Option<NativeAudioSource> = None;
    while let Some(msg) = rx.recv().await {
        match msg {
            DspMsg::Capture(samples) => {
                capture.extend(samples.into_iter().map(to_i16));
                while capture.len() >= FRAME_SAMPLES {
                    let mut frame: Vec<i16> = capture.drain(..FRAME_SAMPLES).collect();
                    if let Some(apm) = apm.as_mut() {
                        let _ = apm.set_stream_delay_ms(ECHO_DELAY_MS);
                        if let Err(err) = apm.process_stream(&mut frame, RATE_I32, 1) {
                            debug!(error = %err.message, "APM не обработал кадр");
                        }
                    }
                    let peak = apply_gain(&mut frame, load_f32(&shared.input_gain));
                    let previous = load_f32(&shared.level);
                    store_f32(&shared.level, peak.max(previous * LEVEL_DECAY));
                    if let Some(source) = &sink {
                        let frame = AudioFrame {
                            data: frame.into(),
                            sample_rate: SAMPLE_RATE,
                            num_channels: 1,
                            samples_per_channel: SAMPLE_RATE / 100,
                        };
                        if let Err(err) = source.capture_frame(&frame).await {
                            debug!(error = %err.message, "кадр микрофона не принят");
                        }
                    }
                }
            }
            DspMsg::Render(samples) => {
                render.extend(samples.into_iter().map(to_i16));
                while render.len() >= FRAME_SAMPLES {
                    let mut frame: Vec<i16> = render.drain(..FRAME_SAMPLES).collect();
                    if let Some(apm) = apm.as_mut() {
                        let _ = apm.process_reverse_stream(&mut frame, RATE_I32, 1);
                    }
                }
            }
            DspMsg::Settings(settings) => apm = make_apm(settings),
            DspMsg::Sink(next) => {
                sink = next;
                capture.clear();
            }
        }
    }
}

/// Поток, которому принадлежат потоки cpal.
fn io_loop(rx: &std_mpsc::Receiver<IoCmd>, shared: &Arc<Shared>) {
    let mut input: Option<cpal::Stream> = None;
    let mut output: Option<cpal::Stream> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            IoCmd::OpenInput(device, reply) => {
                input = None;
                let result = open_input(device.as_deref(), shared);
                let _ = reply.send(result.map(|stream| input = Some(stream)));
            }
            IoCmd::CloseInput => input = None,
            IoCmd::OpenOutput(device, reply) => {
                output = None;
                let result = open_output(device.as_deref(), shared);
                let _ = reply.send(result.map(|stream| output = Some(stream)));
            }
            IoCmd::CloseOutput => output = None,
        }
    }
    drop((input, output));
    debug!("поток звука завершён");
}

fn find_device(wanted: Option<&str>, kind: DeviceKind) -> Option<cpal::Device> {
    let host = cpal::default_host();
    if let Some(wanted) = wanted {
        let devices = match kind {
            DeviceKind::Input => host.input_devices().ok().map(Iterator::collect::<Vec<_>>),
            DeviceKind::Output => host.output_devices().ok().map(Iterator::collect::<Vec<_>>),
        };
        let found = devices
            .into_iter()
            .flatten()
            .find(|device| device.id().is_ok_and(|id| id.to_string() == wanted));
        if found.is_some() {
            return found;
        }
        warn!(
            device = wanted,
            "сохранённое устройство не найдено, берём системное"
        );
    }
    match kind {
        DeviceKind::Input => host.default_input_device(),
        DeviceKind::Output => host.default_output_device(),
    }
}

/// Предпочитаем 48 кГц и f32/i16, иначе — системные настройки устройства.
fn pick_config(
    ranges: impl Iterator<Item = cpal::SupportedStreamConfigRange>,
    fallback: impl FnOnce() -> Result<cpal::SupportedStreamConfig, cpal::Error>,
) -> VoiceResult<cpal::SupportedStreamConfig> {
    let mut candidates: Vec<_> = ranges
        .filter(|range| {
            matches!(
                range.sample_format(),
                SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
            )
        })
        .filter_map(|range| range.try_with_sample_rate(SAMPLE_RATE))
        .collect();
    candidates.sort_by_key(|config| {
        (
            config.sample_format() != SampleFormat::F32,
            config.channels(),
        )
    });
    if let Some(config) = candidates.into_iter().next() {
        return Ok(config);
    }
    fallback().map_err(|err| VoiceError::Device(err.to_string()))
}

fn open_input(wanted: Option<&str>, shared: &Arc<Shared>) -> VoiceResult<cpal::Stream> {
    let device = find_device(wanted, DeviceKind::Input).ok_or(VoiceError::NoMicrophone)?;
    let ranges = device
        .supported_input_configs()
        .map_err(|err| VoiceError::Device(err.to_string()))?;
    let config = pick_config(ranges, || device.default_input_config())?;
    let stream = match config.sample_format() {
        SampleFormat::F32 => build_input::<f32>(&device, &config, shared),
        SampleFormat::I16 => build_input::<i16>(&device, &config, shared),
        SampleFormat::U16 => build_input::<u16>(&device, &config, shared),
        other => Err(VoiceError::Device(format!(
            "формат {other} не поддерживается"
        ))),
    }?;
    stream
        .play()
        .map_err(|err| VoiceError::Device(err.to_string()))?;
    info!(
        rate = config.sample_rate(),
        channels = config.channels(),
        format = %config.sample_format(),
        "микрофон открыт"
    );
    Ok(stream)
}

fn build_input<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    shared: &Arc<Shared>,
) -> VoiceResult<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels()).max(1);
    let mut resampler = Resampler::new(config.sample_rate(), SAMPLE_RATE);
    let shared = Arc::clone(shared);
    #[allow(clippy::cast_precision_loss)]
    let scale = 1.0 / channels as f32;
    device
        .build_input_stream::<T, _, _>(
            config.config(),
            move |data: &[T], _| {
                let mut mono = Vec::with_capacity(data.len() / channels + 8);
                for frame in data.chunks(channels) {
                    let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
                    resampler.push(sum * scale, |v| mono.push(v));
                }
                shared.send(DspMsg::Capture(mono));
            },
            |err| warn!(error = %err, "ошибка захвата микрофона"),
            None,
        )
        .map_err(|err| VoiceError::Device(err.to_string()))
}

fn open_output(wanted: Option<&str>, shared: &Arc<Shared>) -> VoiceResult<cpal::Stream> {
    let device = find_device(wanted, DeviceKind::Output)
        .ok_or_else(|| VoiceError::Device("нет устройства воспроизведения".into()))?;
    let ranges = device
        .supported_output_configs()
        .map_err(|err| VoiceError::Device(err.to_string()))?;
    let config = pick_config(ranges, || device.default_output_config())?;
    let stream = match config.sample_format() {
        SampleFormat::F32 => build_output::<f32>(&device, &config, shared),
        SampleFormat::I16 => build_output::<i16>(&device, &config, shared),
        SampleFormat::U16 => build_output::<u16>(&device, &config, shared),
        other => Err(VoiceError::Device(format!(
            "формат {other} не поддерживается"
        ))),
    }?;
    stream
        .play()
        .map_err(|err| VoiceError::Device(err.to_string()))?;
    info!(
        rate = config.sample_rate(),
        channels = config.channels(),
        "воспроизведение открыто"
    );
    Ok(stream)
}

fn build_output<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    shared: &Arc<Shared>,
) -> VoiceResult<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels()).max(1);
    let mut resampler = Resampler::new(SAMPLE_RATE, config.sample_rate());
    let shared = Arc::clone(shared);
    device
        .build_output_stream::<T, _, _>(
            config.config(),
            move |data: &mut [T], _| {
                let capturing = shared.capturing.load(Ordering::Relaxed);
                let mut reference = Vec::new();
                {
                    let mut mixer = lock(&shared.mixer);
                    for frame in data.chunks_mut(channels) {
                        let value = resampler.pull(|| {
                            let sample = mixer.next_sample();
                            if capturing {
                                reference.push(sample);
                            }
                            sample
                        });
                        frame.fill(T::from_sample(value));
                    }
                }
                if capturing {
                    shared.send(DspMsg::Render(reference));
                }
            },
            |err| warn!(error = %err, "ошибка воспроизведения"),
            None,
        )
        .map_err(|err| VoiceError::Device(err.to_string()))
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

/// Источник звука в миксе для доменного вида трека.
#[must_use]
pub const fn mix_source(kind: TrackKind) -> Option<SourceKind> {
    match kind {
        TrackKind::Audio => Some(SourceKind::Voice),
        TrackKind::ScreenShareAudio => Some(SourceKind::Screen),
        TrackKind::Video | TrackKind::ScreenShare => None,
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

    #[test]
    fn only_audio_tracks_go_to_the_mix() {
        assert_eq!(mix_source(TrackKind::Audio), Some(SourceKind::Voice));
        assert_eq!(
            mix_source(TrackKind::ScreenShareAudio),
            Some(SourceKind::Screen)
        );
        assert_eq!(mix_source(TrackKind::Video), None);
    }
}
