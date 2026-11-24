//! Сводка «Сведений о соединении»: пинг, потери, джиттер, сервер и протокол
//! из статистики WebRTC звонка (как у оригинала).

use livekit::webrtc::stats::RtcStats;

/// Сведения о соединении звонка.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConnectionStats {
    /// Пинг до сервера, мс.
    pub ping_ms: Option<f64>,
    /// Потеря входящих пакетов, %.
    pub packet_loss: Option<f64>,
    /// Джиттер входящего звука, мс.
    pub jitter_ms: Option<f64>,
    /// Адрес сервера (кандидат на той стороне).
    pub server: Option<String>,
    /// Протокол (`udp`, `tcp`).
    pub protocol: Option<String>,
}

/// Сводит статистику публикации и подписки в [`ConnectionStats`].
#[must_use]
pub fn summarize(publisher: &[RtcStats], subscriber: &[RtcStats]) -> ConnectionStats {
    let all = || publisher.iter().chain(subscriber.iter());
    let mut out = ConnectionStats::default();
    // Действующая пара кандидатов: пинг, сервер и протокол.
    let pair = all().find_map(|stat| match stat {
        RtcStats::CandidatePair(pair) if pair.candidate_pair.nominated => Some(pair),
        _ => None,
    });
    if let Some(pair) = pair {
        let rtt = pair.candidate_pair.current_round_trip_time;
        if rtt > 0.0 {
            out.ping_ms = Some(rtt * 1000.0);
        }
        let remote = all().find_map(|stat| match stat {
            RtcStats::RemoteCandidate(c) if c.rtc.id == pair.candidate_pair.remote_candidate_id => {
                Some(&c.remote_candidate)
            }
            _ => None,
        });
        if let Some(remote) = remote {
            out.server = Some(format!("{}:{}", remote.address, remote.port));
            out.protocol = Some(remote.protocol.clone());
        }
    }
    // Входящий звук: потери и джиттер.
    let (mut lost, mut received, mut jitter, mut streams) = (0_i64, 0_u64, 0.0_f64, 0_u32);
    for stat in subscriber {
        if let RtcStats::InboundRtp(rtp) = stat
            && rtp.stream.kind == "audio"
        {
            lost += rtp.received.packets_lost.max(0);
            received += rtp.received.packets_received;
            jitter += rtp.received.jitter;
            streams += 1;
        }
    }
    #[allow(clippy::cast_precision_loss, reason = "доля для показа")]
    if streams > 0 {
        let total = received as f64 + lost as f64;
        if total > 0.0 {
            out.packet_loss = Some(lost as f64 / total * 100.0);
        }
        out.jitter_ms = Some(jitter / f64::from(streams) * 1000.0);
    }
    out
}
