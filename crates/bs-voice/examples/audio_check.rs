//! Диагностика звука: перечисляет устройства и полсекунды за полсекундой
//! показывает уровень микрофона при громкости 100 % и 200 %.
//!
//! `cargo run -p bs-voice --example audio_check`

use std::time::Duration;

use bs_voice::{AudioEngine, AudioProcessingSettings, DeviceKind, MicMonitor, Volume};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = AudioEngine::new(AudioProcessingSettings::default())?;
    for kind in [DeviceKind::Input, DeviceKind::Output] {
        for device in engine.devices(kind) {
            println!("{kind:?}: {} — {}", device.id, device.name);
        }
    }
    let monitor = MicMonitor::start(&engine, &tokio::runtime::Handle::current())?;
    for volume in [100, 200, 0] {
        engine.set_input_volume(Volume::new(volume));
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            println!("громкость {volume:>3} %: уровень {:.3}", monitor.level());
        }
    }
    Ok(())
}
