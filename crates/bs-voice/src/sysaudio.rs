//! Системный звук для демонстрации экрана.
//!
//! * Linux (PulseAudio или PipeWire через pipewire-pulse): звук каждого
//!   приложения записывается отдельно (`direct_on_input` у монитора его
//!   выхода) и смешивается здесь же. Собственные потоки Backspace — голоса
//!   звонка и звуки интерфейса — в смесь не попадают, как у оригинала с
//!   venmic: зрители не слышат сами себя.
//! * Windows и macOS: loopback устройства вывода по умолчанию через cpal
//!   (WASAPI loopback, на macOS — process tap). Там звук звонка тоже
//!   слышен в трансляции: системного способа исключить своё приложение у
//!   cpal нет.
//!
//! Наружу идут кадры по 10 мс: 48 кГц, стерео, i16 вперемешку.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::error::{VoiceError, VoiceResult};
use crate::mixer::to_i16;

/// Частота кадров на выходе.
pub const RATE: u32 = 48_000;
/// Каналов на выходе.
pub const CHANNELS: u32 = 2;
/// Сэмплов на канал в кадре (10 мс).
pub const FRAME: usize = 480;
/// Сколько звука держим про запас на источник: больше — копится задержка.
const MAX_BUFFER: usize = FRAME * 2 * 20;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Очередь сэмплов одного источника (стерео вперемешку, f32).
#[derive(Debug, Clone, Default)]
struct Queue(Arc<Mutex<VecDeque<f32>>>);

impl Queue {
    fn push(&self, samples: impl IntoIterator<Item = f32>) {
        let mut queue = lock(&self.0);
        queue.extend(samples);
        // Отстали — выбрасываем старое, задержка не должна расти.
        let extra = queue.len().saturating_sub(MAX_BUFFER);
        if extra > 0 {
            queue.drain(..extra + extra % 2);
        }
    }

    /// Добавляет до `out.len()` сэмплов к смеси.
    fn mix_into(&self, out: &mut [f32]) {
        let mut queue = lock(&self.0);
        let take = out.len().min(queue.len());
        for (slot, sample) in out.iter_mut().zip(queue.drain(..take)) {
            *slot += sample;
        }
    }
}

/// Идущий захват системного звука. Останавливается при `Drop`.
#[derive(Debug)]
pub struct SystemAudio {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl SystemAudio {
    /// Начинает захват; кадры приходят в канал.
    ///
    /// # Errors
    ///
    /// Звуковой сервер недоступен или устройство не открылось.
    pub fn start() -> VoiceResult<(Self, mpsc::Receiver<Vec<i16>>)> {
        let (tx, rx) = mpsc::channel(16);
        let running = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&running);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<VoiceResult<()>>();
        let handle = std::thread::Builder::new()
            .name("bs-voice-sysaudio".into())
            .spawn(move || {
                if let Err(err) = run(&flag, &tx, &ready_tx) {
                    let _ = ready_tx.send(Err(err));
                }
            })
            .map_err(|err| VoiceError::Device(err.to_string()))?;
        ready_rx
            .recv()
            .unwrap_or_else(|_| Err(VoiceError::Device("захват звука завершился".into())))?;
        Ok((
            Self {
                running,
                handle: Some(handle),
            },
            rx,
        ))
    }

    /// Останавливает захват.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for SystemAudio {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Раз в 10 мс смешивает очереди и отдаёт кадр; `poll` вызывается между
/// кадрами (обновить список источников и т. п.).
fn pump(
    running: &AtomicBool,
    tx: &mpsc::Sender<Vec<i16>>,
    mut queues: impl FnMut() -> Vec<Queue>,
    mut poll: impl FnMut(),
) {
    let period = Duration::from_millis(10);
    let mut next = Instant::now();
    let mut mix = vec![0.0_f32; FRAME * 2];
    while running.load(Ordering::Relaxed) {
        poll();
        mix.fill(0.0);
        for queue in queues() {
            queue.mix_into(&mut mix);
        }
        let frame: Vec<i16> = mix.iter().copied().map(to_i16).collect();
        if let Err(mpsc::error::TrySendError::Closed(_)) = tx.try_send(frame) {
            break;
        }
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
}

#[cfg(target_os = "linux")]
fn run(
    running: &AtomicBool,
    tx: &mpsc::Sender<Vec<i16>>,
    ready: &std::sync::mpsc::Sender<VoiceResult<()>>,
) -> VoiceResult<()> {
    pulse::run(running, tx, ready)
}

#[cfg(not(target_os = "linux"))]
fn run(
    running: &AtomicBool,
    tx: &mpsc::Sender<Vec<i16>>,
    ready: &std::sync::mpsc::Sender<VoiceResult<()>>,
) -> VoiceResult<()> {
    loopback::run(running, tx, ready)
}

/// Loopback устройства вывода через cpal (Windows, macOS).
#[cfg_attr(
    target_os = "linux",
    allow(dead_code, reason = "на Linux — PulseAudio")
)]
mod loopback {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, Sample as _, SampleFormat, SizedSample};

    use super::{AtomicBool, Queue, RATE, VoiceError, VoiceResult, info, mpsc, pump, warn};
    use crate::mixer::Resampler;

    pub(super) fn run(
        running: &AtomicBool,
        tx: &mpsc::Sender<Vec<i16>>,
        ready: &std::sync::mpsc::Sender<VoiceResult<()>>,
    ) -> VoiceResult<()> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| VoiceError::Device("нет устройства вывода".into()))?;
        let config = device
            .default_output_config()
            .map_err(|err| VoiceError::Device(err.to_string()))?;
        let queue = Queue::default();
        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &config, queue.clone()),
            SampleFormat::I16 => build::<i16>(&device, &config, queue.clone()),
            SampleFormat::I32 => build::<i32>(&device, &config, queue.clone()),
            SampleFormat::U16 => build::<u16>(&device, &config, queue.clone()),
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
            "системный звук: loopback устройства вывода"
        );
        let _ = ready.send(Ok(()));
        pump(running, tx, || vec![queue.clone()], || {});
        drop(stream);
        Ok(())
    }

    fn build<T>(
        device: &cpal::Device,
        config: &cpal::SupportedStreamConfig,
        queue: Queue,
    ) -> VoiceResult<cpal::Stream>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let channels = usize::from(config.channels()).max(1);
        let mut left = Resampler::new(config.sample_rate(), RATE);
        let mut right = Resampler::new(config.sample_rate(), RATE);
        device
            .build_input_stream::<T, _, _>(
                config.config(),
                move |data: &[T], _| {
                    let mut lefts = Vec::with_capacity(data.len() / channels + 8);
                    let mut rights = Vec::with_capacity(data.len() / channels + 8);
                    for frame in data.chunks(channels) {
                        let l = f32::from_sample(frame[0]);
                        let r = frame.get(1).map_or(l, |s| f32::from_sample(*s));
                        left.push(l, |v| lefts.push(v));
                        right.push(r, |v| rights.push(v));
                    }
                    queue.push(lefts.into_iter().zip(rights).flat_map(<[f32; 2]>::from));
                },
                |err| warn!(error = %err, "ошибка захвата системного звука"),
                None,
            )
            .map_err(|err| VoiceError::Device(err.to_string()))
    }
}

/// Запись звука приложений через протокол PulseAudio.
#[cfg(target_os = "linux")]
mod pulse {
    use std::collections::HashMap;
    use std::ffi::CString;
    use std::io::BufReader;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    use futures::executor::block_on;
    use pulseaudio::protocol::{
        self, ChannelMap, Command, Prop, Props, RecordStreamParams, SampleFormat, SampleSpec,
        SinkInputInfo, SinkInputInfoList,
    };
    use pulseaudio::{Client, RecordStream};

    use super::{
        AtomicBool, CHANNELS, FRAME, Queue, RATE, VoiceError, VoiceResult, debug, info, mpsc, pump,
        warn,
    };

    fn device_error(err: impl std::fmt::Display) -> VoiceError {
        VoiceError::Device(format!("PulseAudio/PipeWire: {err}"))
    }

    /// Отдельное блокирующее соединение для списка потоков: у асинхронного
    /// клиента такого запроса нет.
    struct Lister {
        socket: BufReader<UnixStream>,
        version: u16,
        seq: u32,
    }

    impl Lister {
        fn connect() -> VoiceResult<Self> {
            let path = pulseaudio::socket_path_from_env()
                .ok_or_else(|| device_error("звуковой сервер не найден"))?;
            let socket = UnixStream::connect(path).map_err(device_error)?;
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .map_err(device_error)?;
            let mut lister = Self {
                socket: BufReader::new(socket),
                version: protocol::MAX_VERSION,
                seq: 0,
            };
            let cookie = pulseaudio::cookie_path_from_env()
                .and_then(|path| std::fs::read(path).ok())
                .unwrap_or_default();
            let auth: protocol::AuthReply = lister.call(&Command::Auth(protocol::AuthParams {
                version: protocol::MAX_VERSION,
                supports_shm: false,
                supports_memfd: false,
                cookie,
            }))?;
            lister.version = protocol::MAX_VERSION.min(auth.version);
            let mut props = Props::new();
            props.set(Prop::ApplicationName, c"Backspace (system audio)");
            let _: protocol::SetClientNameReply = lister.call(&Command::SetClientName(props))?;
            Ok(lister)
        }

        fn call<R: protocol::CommandReply>(&mut self, command: &Command) -> VoiceResult<R> {
            self.seq = self.seq.wrapping_add(1);
            protocol::write_command_message(self.socket.get_mut(), self.seq, command, self.version)
                .map_err(device_error)?;
            let (_, reply) = protocol::read_reply_message::<R>(&mut self.socket, self.version)
                .map_err(device_error)?;
            Ok(reply)
        }

        fn sink_inputs(&mut self) -> VoiceResult<SinkInputInfoList> {
            self.call(&Command::GetSinkInputInfoList)
        }
    }

    fn prop(props: &Props, prop: Prop) -> Option<&str> {
        let bytes = props.get(prop)?;
        let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
        std::str::from_utf8(bytes).ok()
    }

    /// Поток этого же приложения (голоса звонка, звуки интерфейса).
    fn is_own(input: &SinkInputInfo, pid: &str, binary: Option<&str>) -> bool {
        prop(&input.props, Prop::ApplicationProcessId) == Some(pid)
            || binary.is_some_and(|binary| {
                prop(&input.props, Prop::ApplicationProcessBinary) == Some(binary)
            })
    }

    /// Записываемые сейчас потоки приложений.
    struct Recorder {
        client: Client,
        lister: Lister,
        pid: String,
        binary: Option<String>,
        streams: HashMap<u32, (RecordStream, Queue)>,
        failed: Vec<u32>,
        last_scan: Option<Instant>,
    }

    impl Recorder {
        fn queues(&self) -> Vec<Queue> {
            self.streams
                .values()
                .map(|(_, queue)| queue.clone())
                .collect()
        }

        /// Раз в полсекунды сверяет список потоков: новые начинает писать,
        /// ушедшие закрывает.
        fn scan(&mut self) {
            if self
                .last_scan
                .is_some_and(|last| last.elapsed() < Duration::from_millis(500))
            {
                return;
            }
            self.last_scan = Some(Instant::now());
            let inputs = match self.lister.sink_inputs() {
                Ok(inputs) => inputs,
                Err(err) => {
                    debug!(%err, "список потоков недоступен");
                    return;
                }
            };
            let alive: Vec<u32> = inputs.iter().map(|input| input.index).collect();
            let gone: Vec<u32> = self
                .streams
                .keys()
                .copied()
                .filter(|index| !alive.contains(index))
                .collect();
            for index in gone {
                if let Some((stream, _)) = self.streams.remove(&index) {
                    let _ = block_on(stream.delete());
                }
            }
            self.failed.retain(|index| alive.contains(index));
            for input in inputs {
                if self.streams.contains_key(&input.index)
                    || self.failed.contains(&input.index)
                    || is_own(&input, &self.pid, self.binary.as_deref())
                {
                    continue;
                }
                match self.record(&input) {
                    Ok(entry) => {
                        debug!(input = input.index, "системный звук: новый источник");
                        self.streams.insert(input.index, entry);
                    }
                    Err(err) => {
                        warn!(input = input.index, %err, "поток приложения не записать");
                        self.failed.push(input.index);
                    }
                }
            }
        }

        fn record(&self, input: &SinkInputInfo) -> VoiceResult<(RecordStream, Queue)> {
            let sink = block_on(self.client.sink_info(input.sink_index)).map_err(device_error)?;
            let monitor = sink
                .monitor_source_index
                .ok_or_else(|| device_error("у выхода нет монитора"))?;
            let queue = Queue::default();
            let target = queue.clone();
            let mut props = Props::new();
            props.set(Prop::MediaName, c"Backspace screen share audio");
            #[allow(clippy::cast_possible_truncation, reason = "2 канала")]
            let channels = CHANNELS as u8;
            let params = RecordStreamParams {
                sample_spec: SampleSpec {
                    format: SampleFormat::Float32Le,
                    channels,
                    sample_rate: RATE,
                },
                channel_map: ChannelMap::stereo(),
                source_index: Some(monitor),
                direct_on_input_index: Some(input.index),
                buffer_attr: protocol::stream::BufferAttr {
                    #[allow(clippy::cast_possible_truncation, reason = "10 мс стерео f32")]
                    fragment_size: (FRAME * 2 * 4) as u32,
                    ..protocol::stream::BufferAttr::default()
                },
                flags: protocol::stream::StreamFlags {
                    adjust_latency: true,
                    ..protocol::stream::StreamFlags::default()
                },
                props,
                ..RecordStreamParams::default()
            };
            let stream = block_on(self.client.create_record_stream(
                params,
                move |data: &[u8]| {
                    target.push(
                        data.chunks_exact(4)
                            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    );
                },
            ))
            .map_err(device_error)?;
            Ok((stream, queue))
        }
    }

    pub(super) fn run(
        running: &AtomicBool,
        tx: &mpsc::Sender<Vec<i16>>,
        ready: &std::sync::mpsc::Sender<VoiceResult<()>>,
    ) -> VoiceResult<()> {
        let name = CString::new(format!("backspace-system-audio-{}", std::process::id()))
            .map_err(device_error)?;
        let client = Client::from_env(&name).map_err(device_error)?;
        let lister = Lister::connect()?;
        let binary = std::env::current_exe().ok().and_then(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        });
        let mut recorder = Recorder {
            client,
            lister,
            pid: std::process::id().to_string(),
            binary,
            streams: HashMap::new(),
            failed: Vec::new(),
            last_scan: None,
        };
        recorder.scan();
        info!(
            sources = recorder.streams.len(),
            "системный звук: запись приложений без своего"
        );
        let _ = ready.send(Ok(()));
        let recorder = std::cell::RefCell::new(recorder);
        pump(
            running,
            tx,
            || recorder.borrow().queues(),
            || recorder.borrow_mut().scan(),
        );
        for (_, (stream, _)) in recorder.into_inner().streams.drain() {
            let _ = block_on(stream.delete());
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp, reason = "значения точно представимы")]
mod tests {
    use super::*;

    #[test]
    fn queue_mixes_and_caps_latency() {
        let queue = Queue::default();
        queue.push([0.25_f32; 8]);
        let mut out = [0.5_f32; 4];
        queue.mix_into(&mut out);
        assert_eq!(out, [0.75; 4]);
        queue.push(std::iter::repeat_n(0.1, MAX_BUFFER * 2));
        assert!(lock(&queue.0).len() <= MAX_BUFFER);
        assert_eq!(lock(&queue.0).len() % 2, 0, "стерео-пары не рвутся");
    }

    #[test]
    fn short_queue_leaves_silence() {
        let queue = Queue::default();
        queue.push([1.0_f32; 2]);
        let mut out = [0.0_f32; 4];
        queue.mix_into(&mut out);
        assert_eq!(out, [1.0, 1.0, 0.0, 0.0]);
    }
}
