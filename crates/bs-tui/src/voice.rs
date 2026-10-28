//! Фоновый исполнитель голоса для TUI: держит VoiceSession в отдельном
//! потоке и обменивается с UI-потоком командами и событиями.
//! Построен по образцу net::NetWorker (spawn + drain).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, mpsc};

use bs_voice::{
    AudioEngine, AudioProcessingSettings, VoiceConfig, VoiceEvent, VoiceResult, VoiceSession,
};

/// Команда UI-потока исполнителю голоса.
#[derive(Debug)]
pub enum VoiceCommand {
    Connect {
        url: String,
        token: String,
        identity: String,
        display_name: String,
    },
    Disconnect,
    SetMuted(bool),
    SetDeafened(bool),
}

/// Событие исполнителя для UI-потока.
#[derive(Debug)]
pub enum VoiceCtlEvent {
    Connected,
    ConnectFailed { message: String },
    Disconnected,
    Voice(VoiceEvent),
}

type EventQueue = Arc<Mutex<VecDeque<VoiceCtlEvent>>>;

fn push_event(queue: &EventQueue, event: VoiceCtlEvent) {
    if let Ok(mut q) = queue.lock() {
        q.push_back(event);
        while q.len() > 256 {
            q.pop_front();
        }
    }
}

/// Дескриптор фонового исполнителя голоса.
#[derive(Debug)]
pub struct VoiceCtl {
    cmd_tx: mpsc::UnboundedSender<VoiceCommand>,
    events: EventQueue,
    join: Option<std::thread::JoinHandle<()>>,
}

impl VoiceCtl {
    /// Запускает фоновый поток с собственным tokio-рантаймом.
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let events: EventQueue = Arc::new(Mutex::new(VecDeque::new()));
        let worker_events = Arc::clone(&events);
        let join = std::thread::spawn(move || run_worker(cmd_rx, worker_events));
        Self {
            cmd_tx,
            events,
            join: Some(join),
        }
    }

    pub fn connect(&self, url: String, token: String, identity: String, display_name: String) {
        let _ = self.cmd_tx.send(VoiceCommand::Connect {
            url,
            token,
            identity,
            display_name,
        });
    }

    pub fn disconnect(&self) {
        let _ = self.cmd_tx.send(VoiceCommand::Disconnect);
    }

    pub fn set_muted(&self, muted: bool) {
        let _ = self.cmd_tx.send(VoiceCommand::SetMuted(muted));
    }

    pub fn set_deafened(&self, deafened: bool) {
        let _ = self.cmd_tx.send(VoiceCommand::SetDeafened(deafened));
    }

    /// Забирает накопившиеся события; вызывается из UI-потока.
    pub fn drain(&self) -> Vec<VoiceCtlEvent> {
        self.events
            .lock()
            .map_or_else(|_| Vec::new(), |mut queue| queue.drain(..).collect())
    }
}

impl Drop for VoiceCtl {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(VoiceCommand::Disconnect);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
enum VoicePoll {
    Event(VoiceEvent),
    Skip,
    Dead,
}

async fn recv_voice(rx: &mut Option<broadcast::Receiver<VoiceEvent>>) -> VoicePoll {
    let Some(rx) = rx.as_mut() else {
        std::future::pending::<()>().await;
        return VoicePoll::Dead;
    };
    match rx.recv().await {
        Ok(v) => VoicePoll::Event(v),
        Err(broadcast::error::RecvError::Lagged(_)) => VoicePoll::Skip,
        Err(broadcast::error::RecvError::Closed) => VoicePoll::Dead,
    }
}

fn run_worker(cmd_rx: mpsc::UnboundedReceiver<VoiceCommand>, events: EventQueue) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    runtime.block_on(session_loop(cmd_rx, events));
}

async fn session_loop(mut cmd_rx: mpsc::UnboundedReceiver<VoiceCommand>, events: EventQueue) {
    let mut session: Option<VoiceSession> = None;
    let mut event_rx: Option<broadcast::Receiver<VoiceEvent>> = None;
    let mut next_rx: Option<broadcast::Receiver<VoiceEvent>> = None;
    let mut clear_rx = false;

    loop {
        if clear_rx {
            clear_rx = false;
            event_rx = None;
        }
        if let Some(rx) = next_rx.take() {
            event_rx = Some(rx);
        }
        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    VoiceCommand::Connect { url, token, identity, display_name } => {
                        clear_rx = true;
                        if let Some(old) = session.take() {
                            let _ = old.disconnect().await;
                        }
                        match open_session(&url, &token, identity, display_name).await {
                            Ok(sess) => {
                                next_rx = Some(sess.subscribe());
                                push_event(&events, VoiceCtlEvent::Connected);
                                session = Some(sess);
                            }
                            Err(e) => {
                                push_event(&events, VoiceCtlEvent::ConnectFailed {
                                    message: e.to_string(),
                                });
                            }
                        }
                    }
                    VoiceCommand::Disconnect => {
                        clear_rx = true;
                        if let Some(old) = session.take() {
                            let _ = old.disconnect().await;
                            push_event(&events, VoiceCtlEvent::Disconnected);
                        }
                    }
                    VoiceCommand::SetMuted(v) => {
                        if let Some(sess) = session.as_ref() {
                            let _ = sess.set_muted(v).await;
                        }
                    }
                    VoiceCommand::SetDeafened(v) => {
                        if let Some(sess) = session.as_ref() {
                            let _ = sess.set_deafened(v).await;
                        }
                    }
                }
            }
            poll = recv_voice(&mut event_rx) => {
                match poll {
                    VoicePoll::Event(v) => push_event(&events, VoiceCtlEvent::Voice(v)),
                    VoicePoll::Skip => {}
                    VoicePoll::Dead => clear_rx = true,
                }
            }
        }
    }

    if let Some(old) = session.take() {
        let _ = old.disconnect().await;
    }
}

async fn open_session(
    url: &str,
    token: &str,
    identity: String,
    display_name: String,
) -> VoiceResult<VoiceSession> {
    let audio = AudioEngine::new(AudioProcessingSettings::default())?;
    let config = VoiceConfig {
        auto_publish_mic: true,
        start_muted: false,
        start_deafened: false,
        ..VoiceConfig::default()
    };
    VoiceSession::connect(url, token, config, audio, identity, display_name).await
}
