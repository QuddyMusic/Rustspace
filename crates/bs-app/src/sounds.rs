//! Звуки интерфейса: вход и выход из голосового канала, мьют и наушники.
//!
//! Набор и правила — как в оригинале (`SoundController`): звук играет на
//! *переходе* состояния, а при подключении к каналу участники, уже сидевшие
//! там, молчат. Решение «какой звук» — чистая функция [`cues`], её можно
//! проверить без звуковой карты. Сам вывод — через `rodio`, он включается
//! фичей `sounds` (её тянет `voice`); без фичи и без звукового устройства
//! приложение просто молчит.
//!
//! Файлы `assets/sounds/*.ogg` — звуки проекта Backspace, лицензия та же
//! (AGPL-3.0-only).

use std::collections::BTreeSet;

/// Звуковая подсказка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sound {
    /// Кто-то вошёл в канал (в том числе мы сами).
    UserJoin,
    /// Кто-то вышел из канала.
    UserLeave,
    /// Мы вышли из канала.
    Disconnect,
    /// Микрофон выключен.
    Mute,
    /// Микрофон включён.
    Unmute,
    /// Звук выключен.
    Deafen,
    /// Звук включён.
    Undeafen,
}

impl Sound {
    /// Содержимое файла (ogg/vorbis), зашитое в программу.
    #[cfg_attr(
        not(feature = "sounds"),
        allow(dead_code, reason = "нужно только настоящему проигрывателю")
    )]
    const fn bytes(self) -> &'static [u8] {
        match self {
            Self::UserJoin => include_bytes!("../assets/sounds/user_join.ogg"),
            Self::UserLeave => include_bytes!("../assets/sounds/user_leave.ogg"),
            Self::Disconnect => include_bytes!("../assets/sounds/disconnect.ogg"),
            Self::Mute => include_bytes!("../assets/sounds/mute.ogg"),
            Self::Unmute => include_bytes!("../assets/sounds/unmute.ogg"),
            Self::Deafen => include_bytes!("../assets/sounds/deafen.ogg"),
            Self::Undeafen => include_bytes!("../assets/sounds/undeafen.ogg"),
        }
    }
}

/// Что видно о голосовой комнате в один момент.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RoomView {
    /// Сервер подтвердил, что мы в канале.
    pub(crate) in_room: bool,
    /// Микрофон выключен.
    pub(crate) muted: bool,
    /// Звук выключен.
    pub(crate) deafened: bool,
    /// Остальные участники канала.
    pub(crate) others: BTreeSet<String>,
}

/// Какие звуки сыграть при переходе `prev` → `next`.
#[must_use]
pub(crate) fn cues(prev: &RoomView, next: &RoomView) -> Vec<Sound> {
    match (prev.in_room, next.in_room) {
        // Вошли: остальные в комнате — это исходное состояние, а не события.
        (false, true) => vec![Sound::UserJoin],
        (true, false) => vec![Sound::Disconnect],
        (false, false) => Vec::new(),
        (true, true) => {
            let mut out = Vec::new();
            // Наушники тянут за собой микрофон: играет один звук, не два.
            if prev.deafened != next.deafened {
                out.push(if next.deafened {
                    Sound::Deafen
                } else {
                    Sound::Undeafen
                });
            } else if prev.muted != next.muted {
                out.push(if next.muted {
                    Sound::Mute
                } else {
                    Sound::Unmute
                });
            }
            if next.others.difference(&prev.others).next().is_some() {
                out.push(Sound::UserJoin);
            }
            if prev.others.difference(&next.others).next().is_some() {
                out.push(Sound::UserLeave);
            }
            out
        }
    }
}

/// Базовая громкость звуков интерфейса (как `SFX_BASE_VOLUME` оригинала).
#[cfg(feature = "sounds")]
const BASE_VOLUME: f32 = 0.8;

#[cfg(feature = "sounds")]
mod real {
    use std::io::Cursor;

    use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};

    use super::{BASE_VOLUME, Sound};

    /// Проигрыватель: устройство открывается при первом звуке.
    #[derive(Default)]
    pub(crate) struct Sounds {
        sink: Option<MixerDeviceSink>,
        /// Устройство не открылось — больше не пытаемся и не шумим в логе.
        unavailable: bool,
    }

    impl std::fmt::Debug for Sounds {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Sounds")
                .field("open", &self.sink.is_some())
                .field("unavailable", &self.unavailable)
                .finish()
        }
    }

    impl Sounds {
        /// Играет звук и не ждёт его конца.
        pub(crate) fn play(&mut self, sound: Sound) {
            if self.unavailable {
                return;
            }
            if self.sink.is_none() {
                match DeviceSinkBuilder::open_default_sink() {
                    Ok(mut sink) => {
                        sink.log_on_drop(false);
                        self.sink = Some(sink);
                    }
                    Err(err) => {
                        tracing::warn!(%err, "звуковое устройство недоступно, звуков не будет");
                        self.unavailable = true;
                        return;
                    }
                }
            }
            let Some(sink) = &self.sink else {
                return;
            };
            match Decoder::new_vorbis(Cursor::new(sound.bytes())) {
                Ok(decoder) => {
                    let player = Player::connect_new(sink.mixer());
                    player.set_volume(BASE_VOLUME);
                    player.append(decoder);
                    player.detach();
                }
                Err(err) => tracing::warn!(%err, ?sound, "не удалось разобрать звук"),
            }
        }
    }
}

#[cfg(not(feature = "sounds"))]
mod real {
    use super::Sound;

    /// Заглушка: сборка без звуков.
    #[derive(Debug, Default)]
    pub(crate) struct Sounds;

    impl Sounds {
        #[allow(
            clippy::unused_self,
            clippy::needless_pass_by_ref_mut,
            reason = "тот же интерфейс, что у настоящего"
        )]
        pub(crate) fn play(&mut self, _sound: Sound) {}
    }
}

pub(crate) use real::Sounds;

#[cfg(test)]
mod tests {
    use super::*;

    fn view(in_room: bool, muted: bool, deafened: bool, others: &[&str]) -> RoomView {
        RoomView {
            in_room,
            muted,
            deafened,
            others: others.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn joining_is_one_cue_and_existing_members_stay_quiet() {
        let out = cues(
            &view(false, false, false, &[]),
            &view(true, false, false, &["a", "b"]),
        );
        assert_eq!(out, [Sound::UserJoin]);
    }

    #[test]
    fn leaving_plays_disconnect_only() {
        let out = cues(
            &view(true, true, false, &["a"]),
            &view(false, false, false, &[]),
        );
        assert_eq!(out, [Sound::Disconnect]);
        assert!(
            cues(
                &view(false, false, false, &[]),
                &view(false, true, false, &["a"])
            )
            .is_empty()
        );
    }

    #[test]
    fn deafen_drags_mute_but_plays_once() {
        let from = view(true, false, false, &[]);
        assert_eq!(cues(&from, &view(true, true, true, &[])), [Sound::Deafen]);
        assert_eq!(
            cues(&view(true, true, true, &[]), &view(true, false, false, &[])),
            [Sound::Undeafen]
        );
        assert_eq!(cues(&from, &view(true, true, false, &[])), [Sound::Mute]);
        assert_eq!(cues(&view(true, true, false, &[]), &from), [Sound::Unmute]);
    }

    #[test]
    fn others_coming_and_going() {
        let base = view(true, false, false, &["a"]);
        assert_eq!(
            cues(&base, &view(true, false, false, &["a", "b"])),
            [Sound::UserJoin]
        );
        assert_eq!(
            cues(&base, &view(true, false, false, &[])),
            [Sound::UserLeave]
        );
        assert!(cues(&base, &base.clone()).is_empty());
    }

    #[cfg(feature = "sounds")]
    #[test]
    fn every_sound_decodes() {
        for sound in ALL {
            let decoder = rodio::Decoder::new_vorbis(std::io::Cursor::new(sound.bytes()))
                .unwrap_or_else(|err| panic!("{sound:?}: {err}"));
            assert!(decoder.count() > 1000, "{sound:?} слишком короткий");
        }
    }

    const ALL: [Sound; 7] = [
        Sound::UserJoin,
        Sound::UserLeave,
        Sound::Disconnect,
        Sound::Mute,
        Sound::Unmute,
        Sound::Deafen,
        Sound::Undeafen,
    ];

    #[test]
    fn every_sound_is_a_decodable_vorbis_file() {
        for sound in ALL {
            assert_eq!(&sound.bytes()[..4], b"OggS", "{sound:?}");
        }
    }
}
