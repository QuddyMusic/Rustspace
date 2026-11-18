//! Музыка и видео: что сейчас играет в плеере. Порт `mediaDetector` из PR
//! #482 оригинального Backspace (cirzear, QuddyMusic). На Linux — MPRIS
//! через D-Bus (любой плеер: Spotify, браузеры, mpv, VLC, Rhythmbox…).

use bs_types::{Activity, ActivityAssets, ActivityPlayback, ActivityTimestamps, ActivityType};

/// Трек, как его сообщил плеер.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Track {
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) album: Option<String>,
    pub(crate) art_url: Option<String>,
    pub(crate) duration_ms: Option<i64>,
    pub(crate) position_ms: Option<i64>,
    pub(crate) player: String,
    pub(crate) url: Option<String>,
    pub(crate) video: bool,
}

/// Начало трека на наших часах; меняется при смене трека или перемотке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Timeline {
    key: String,
    pub(crate) start: i64,
}

/// Отклонение позиции, которое ещё не считается перемоткой.
const SYNC_TOLERANCE_MS: i64 = 1500;

/// Одна отметка начала на трек: плееры повторяют одну и ту же нулевую
/// позицию, и пересчёт на каждом опросе обнулял бы прогресс у других.
pub(crate) fn next_timeline(previous: Option<&Timeline>, track: &Track, now: i64) -> Timeline {
    let key = format!("{}\0{}\0{}", track.player, track.title, track.artist);
    let position = track.position_ms.filter(|p| *p >= 0);
    match previous {
        Some(previous) if previous.key == key => {
            let moved = position
                .is_some_and(|p| p > 0 && (p - (now - previous.start)).abs() > SYNC_TOLERANCE_MS);
            Timeline {
                key,
                start: if moved {
                    now - position.unwrap_or(0)
                } else {
                    previous.start
                },
            }
        }
        _ => Timeline {
            key,
            start: now - position.unwrap_or(0),
        },
    }
}

fn cut(text: &str) -> String {
    text.chars().take(128).collect()
}

/// Трек → активность «слушает»/«смотрит» с прогрессом.
pub(crate) fn to_activity(track: &Track, start: i64, now: i64) -> Activity {
    let duration = track.duration_ms.filter(|d| *d > 0);
    let elapsed = (now - start).max(0);
    let position = duration.map_or(elapsed, |d| d.min(elapsed));
    Activity {
        activity_type: if track.video {
            ActivityType::Watching
        } else {
            ActivityType::Listening
        },
        name: cut(&track.title),
        details: Some(cut(&track.artist)).filter(|a| !a.is_empty()),
        state: track.album.as_deref().map(cut).filter(|a| !a.is_empty()),
        timestamps: Some(ActivityTimestamps {
            start: Some(start),
            end: duration.map(|d| start + d),
        }),
        playback: Some(ActivityPlayback {
            position_ms: position,
            duration_ms: duration,
        }),
        assets: Some(ActivityAssets {
            large_image: track.art_url.clone(),
            large_text: track.album.as_deref().map(cut),
            small_image: None,
            small_text: Some(cut(&track.player)).filter(|p| !p.is_empty()),
        }),
        url: track.url.clone(),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "таблица сервисов, по ветке на каждый"
)]
#[cfg(any(all(target_os = "linux", feature = "presence"), test))]
/// Ссылка на трек — только у известных сервисов и в каноническом виде, без
/// лишних параметров (в них бывают идентификаторы сессии).
pub(crate) fn sanitize_track_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Some(id) = raw.strip_prefix("spotify:track:") {
        return id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric())
            .then(|| format!("https://open.spotify.com/track/{id}"));
    }
    let url = url::Url::parse(raw).ok()?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return None;
    }
    let host = url.host_str()?.to_lowercase();
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let query = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let id_like = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    let slug = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._~%-".contains(&b))
    };
    let on = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    match host.as_str() {
        "open.spotify.com" => match segments.as_slice() {
            ["track", id, ..] if id.bytes().all(|b| b.is_ascii_alphanumeric()) => {
                Some(format!("https://open.spotify.com/track/{id}"))
            }
            _ => None,
        },
        "music.apple.com" if !segments.is_empty() && segments.iter().all(|s| slug(s)) => {
            let base = format!("https://music.apple.com/{}", segments.join("/"));
            Some(match query("i").filter(|i| digits(i)) {
                Some(track) => format!("{base}?i={track}"),
                None => base,
            })
        }
        "music.youtube.com" => query("v")
            .filter(|v| id_like(v))
            .map(|v| format!("https://music.youtube.com/watch?v={v}")),
        "music.163.com" => {
            let id = query("id").or_else(|| {
                let fragment = url.fragment()?;
                let at = fragment.find("id=")?;
                Some(
                    fragment[at + 3..]
                        .chars()
                        .take_while(char::is_ascii_digit)
                        .collect(),
                )
            });
            id.filter(|id| digits(id))
                .map(|id| format!("https://music.163.com/#/song?id={id}"))
        }
        "y.qq.com" if !segments.is_empty() && segments.iter().all(|s| slug(s)) => {
            Some(format!("https://y.qq.com/{}", segments.join("/")))
        }
        _ if host.starts_with("music.yandex.")
            && ["ru", "com", "by", "kz", "uz"].contains(&&host[13..]) =>
        {
            match segments.as_slice() {
                ["album", album, "track", track, ..] if digits(album) && digits(track) => {
                    Some(format!("https://{host}/album/{album}/track/{track}"))
                }
                _ => None,
            }
        }
        _ if on("tidal.com") => {
            let at = segments.iter().position(|s| *s == "track")?;
            segments
                .get(at + 1)
                .filter(|id| digits(id))
                .map(|id| format!("https://tidal.com/track/{id}"))
        }
        _ if on("deezer.com") => {
            let at = segments.iter().position(|s| *s == "track")?;
            segments
                .get(at + 1)
                .filter(|id| digits(id))
                .map(|id| format!("https://www.deezer.com/track/{id}"))
        }
        _ if host.ends_with(".bandcamp.com") => {
            let sub = &host[..host.len() - ".bandcamp.com".len()];
            let ok = !sub.is_empty()
                && sub
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            match segments.as_slice() {
                ["track", name, ..] if ok && slug(name) => {
                    Some(format!("https://{sub}.bandcamp.com/track/{name}"))
                }
                _ => None,
            }
        }
        _ if on("soundcloud.com") => match segments.as_slice() {
            [user, track, ..] if slug(user) && slug(track) => {
                Some(format!("https://soundcloud.com/{user}/{track}"))
            }
            _ => None,
        },
        _ => None,
    }
}

#[cfg_attr(not(all(target_os = "linux", feature = "presence")), allow(dead_code))]
/// Название плеера по имени на шине (`org.mpris.MediaPlayer2.spotify`).
pub(crate) fn player_name(bus: &str) -> String {
    let sub = bus.trim_start_matches("org.mpris.MediaPlayer2.");
    // Экземпляры браузеров: `firefox.instance_1_23`, `chromium.instance123`.
    let sub = sub.split('.').next().unwrap_or(sub).to_lowercase();
    match sub.as_str() {
        "spotify" => "Spotify".into(),
        "netease-cloud-music" => "网易云音乐".into(),
        "qqmusic" => "QQ音乐".into(),
        "rhythmbox" => "Rhythmbox".into(),
        "vlc" => "VLC".into(),
        "firefox" => "Firefox".into(),
        "chromium" => "Chromium".into(),
        "chrome" => "Chrome".into(),
        "yandex-music" => "Яндекс Музыка".into(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
        }
    }
}

/// Опрос MPRIS: первый играющий плеер.
#[cfg(all(target_os = "linux", feature = "presence"))]
pub(crate) mod mpris {
    use std::collections::HashMap;

    use zbus::blocking::Connection;
    use zbus::blocking::fdo::{DBusProxy, PropertiesProxy};
    use zbus::names::InterfaceName;
    use zbus::zvariant::{OwnedValue, Value};

    use super::{Track, player_name, sanitize_track_url};

    const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

    fn text(value: &Value<'_>) -> Option<String> {
        match value {
            Value::Str(s) => Some(s.as_str().to_owned()),
            Value::ObjectPath(p) => Some(p.as_str().to_owned()),
            Value::Value(inner) => text(inner),
            _ => None,
        }
    }

    fn texts(value: &Value<'_>) -> Vec<String> {
        match value {
            Value::Array(items) => items.iter().filter_map(text).collect(),
            Value::Value(inner) => texts(inner),
            other => text(other).into_iter().collect(),
        }
    }

    fn number(value: &Value<'_>) -> Option<i64> {
        match value {
            Value::I64(n) => Some(*n),
            Value::U64(n) => i64::try_from(*n).ok(),
            Value::I32(n) => Some(i64::from(*n)),
            Value::U32(n) => Some(i64::from(*n)),
            #[allow(
                clippy::cast_possible_truncation,
                reason = "длительность в микросекундах"
            )]
            Value::F64(n) => Some(*n as i64),
            Value::Value(inner) => number(inner),
            _ => None,
        }
    }

    /// Метаданные MPRIS → трек.
    pub(crate) fn parse_metadata(
        metadata: &HashMap<String, OwnedValue>,
        player: &str,
    ) -> Option<super::Track> {
        let get = |key: &str| metadata.get(key).map(|v| &**v);
        let title = get("xesam:title").and_then(text)?.trim().to_owned();
        if title.is_empty() {
            return None;
        }
        let artist = get("xesam:artist")
            .map(texts)
            .unwrap_or_default()
            .join(", ")
            .trim()
            .to_owned();
        let album = get("xesam:album")
            .and_then(text)
            .map(|a| a.trim().to_owned())
            .filter(|a| !a.is_empty());
        let duration = get("mpris:length")
            .and_then(number)
            .map(|us| us / 1000)
            .filter(|d| *d > 0);
        let art = get("mpris:artUrl")
            .and_then(text)
            .filter(|u| u.starts_with("https://"));
        let url = get("xesam:url")
            .and_then(text)
            .and_then(|u| sanitize_track_url(&u));
        Some(Track {
            title,
            artist,
            album,
            art_url: art,
            duration_ms: duration,
            position_ms: None,
            player: player.to_owned(),
            url,
            video: false,
        })
    }

    /// Что играет сейчас.
    pub(crate) fn poll(connection: &Connection) -> Option<Track> {
        let names = DBusProxy::new(connection).ok()?.list_names().ok()?;
        let interface = InterfaceName::try_from(PLAYER).ok()?;
        for name in names
            .iter()
            .filter(|n| n.as_str().starts_with("org.mpris.MediaPlayer2."))
        {
            let Ok(builder) = PropertiesProxy::builder(connection).destination(name.as_str())
            else {
                continue;
            };
            let Ok(builder) = builder.path("/org/mpris/MediaPlayer2") else {
                continue;
            };
            let Ok(proxy) = builder.build() else { continue };
            let playing = proxy
                .get(interface.clone(), "PlaybackStatus")
                .ok()
                .and_then(|v| text(&v))
                .is_some_and(|s| s == "Playing");
            if !playing {
                continue;
            }
            let Ok(metadata) = proxy.get(interface.clone(), "Metadata") else {
                continue;
            };
            let Ok(metadata) = HashMap::<String, OwnedValue>::try_from(metadata) else {
                continue;
            };
            if let Some(mut track) = parse_metadata(&metadata, &player_name(name.as_str())) {
                track.position_ms = proxy
                    .get(interface.clone(), "Position")
                    .ok()
                    .and_then(|v| number(&v))
                    .map(|us| us / 1000)
                    .filter(|p| *p >= 0);
                return Some(track);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(position: Option<i64>) -> Track {
        Track {
            title: "Song".into(),
            artist: "Band".into(),
            player: "Spotify".into(),
            duration_ms: Some(200_000),
            position_ms: position,
            ..Track::default()
        }
    }

    #[test]
    fn timeline_survives_stale_zero_positions_and_follows_seeks() {
        let first = next_timeline(None, &track(Some(10_000)), 100_000);
        assert_eq!(first.start, 90_000);
        let stale = next_timeline(Some(&first), &track(Some(0)), 105_000);
        assert_eq!(stale.start, 90_000);
        let close = next_timeline(Some(&first), &track(Some(15_500)), 105_000);
        assert_eq!(close.start, 90_000);
        let seek = next_timeline(Some(&first), &track(Some(60_000)), 105_000);
        assert_eq!(seek.start, 45_000);
        let activity = to_activity(&track(None), 45_000, 105_000);
        assert_eq!(
            activity.playback,
            Some(ActivityPlayback {
                position_ms: 60_000,
                duration_ms: Some(200_000)
            })
        );
        assert_eq!(activity.timestamps.and_then(|t| t.end), Some(245_000));
    }

    #[test]
    fn track_links_are_canonical() {
        assert_eq!(
            sanitize_track_url("spotify:track:abc123").as_deref(),
            Some("https://open.spotify.com/track/abc123")
        );
        assert_eq!(
            sanitize_track_url("https://open.spotify.com/track/abc?si=secret").as_deref(),
            Some("https://open.spotify.com/track/abc")
        );
        assert_eq!(
            sanitize_track_url("https://music.yandex.ru/album/1/track/2?utm=x").as_deref(),
            Some("https://music.yandex.ru/album/1/track/2")
        );
        assert_eq!(
            sanitize_track_url("https://music.163.com/#/song?id=42").as_deref(),
            Some("https://music.163.com/#/song?id=42")
        );
        assert_eq!(sanitize_track_url("file:///home/me/a.mp3"), None);
        assert_eq!(sanitize_track_url("https://example.com/track/1"), None);
        assert_eq!(
            player_name("org.mpris.MediaPlayer2.firefox.instance_1_84"),
            "Firefox"
        );
    }
}
