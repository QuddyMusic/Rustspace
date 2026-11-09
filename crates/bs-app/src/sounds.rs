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
    /// Пришло сообщение, о котором стоит сообщить.
    Message,
    /// Нам звонят (играет по кругу).
    CallRinging,
    /// Мы звоним и ждём ответа (играет по кругу).
    CallCalling,
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
            Self::Message => include_bytes!("../assets/sounds/message.ogg"),
            Self::CallRinging => include_bytes!("../assets/sounds/call_ringing.ogg"),
            Self::CallCalling => include_bytes!("../assets/sounds/call_calling.ogg"),
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

/// Что известно о новом сообщении для решения «играть ли звук».
#[allow(
    clippy::struct_excessive_bools,
    reason = "независимые признаки сообщения"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MessageAlert {
    /// Написали мы сами.
    pub(crate) own: bool,
    /// Личное сообщение (настройки каналов на него не действуют).
    pub(crate) dm: bool,
    /// Канал или пространство заглушены.
    pub(crate) muted: bool,
    /// Уровень уведомлений канала.
    pub(crate) level: bs_types::NotificationLevel,
    /// Нас упомянули прямо (`<@id>`).
    pub(crate) direct: bool,
    /// Стоит `@everyone` / `@here`.
    pub(crate) everyone: bool,
    /// Упомянута роль, которая у нас есть.
    pub(crate) role: bool,
    /// В пространстве отключены массовые упоминания.
    pub(crate) suppress_everyone: bool,
    /// В пространстве отключены упоминания ролей.
    pub(crate) suppress_roles: bool,
}

/// Правило оригинала (`isMessageAlert`): свои — никогда, личные — всегда,
/// в каналах — по настройке; прямое упоминание проходит всегда, кроме «ничего».
#[must_use]
pub(crate) const fn message_alerts(m: &MessageAlert) -> bool {
    use bs_types::NotificationLevel::{All, Mentions, Nothing};
    if m.own {
        return false;
    }
    if m.dm {
        return true;
    }
    if m.muted {
        return false;
    }
    match m.level {
        Nothing => false,
        All => true,
        Mentions => {
            m.direct || (m.everyone && !m.suppress_everyone) || (m.role && !m.suppress_roles)
        }
    }
}

/// Какой вызов играть сейчас: входящий важнее исходящего, а «не беспокоить»
/// глушит только входящий (как в оригинале; карточка звонка остаётся).
#[must_use]
pub(crate) const fn ringtone(incoming: bool, calling: bool, do_not_disturb: bool) -> Option<Sound> {
    if incoming && !do_not_disturb {
        Some(Sound::CallRinging)
    } else if calling {
        Some(Sound::CallCalling)
    } else {
        None
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
        /// Звук, который сейчас играет по кругу.
        looping: Option<(Sound, Player)>,
        /// Устройство не открылось — больше не пытаемся и не шумим в логе.
        unavailable: bool,
    }

    impl std::fmt::Debug for Sounds {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Sounds")
                .field("open", &self.sink.is_some())
                .field("looping", &self.looping.as_ref().map(|(sound, _)| *sound))
                .field("unavailable", &self.unavailable)
                .finish()
        }
    }

    impl Sounds {
        /// Открывает устройство при первом обращении.
        fn sink(&mut self) -> Option<&MixerDeviceSink> {
            if self.sink.is_none() && !self.unavailable {
                match DeviceSinkBuilder::open_default_sink() {
                    Ok(mut sink) => {
                        sink.log_on_drop(false);
                        self.sink = Some(sink);
                    }
                    Err(err) => {
                        tracing::warn!(%err, "звуковое устройство недоступно, звуков не будет");
                        self.unavailable = true;
                    }
                }
            }
            self.sink.as_ref()
        }

        /// Играет звук и не ждёт его конца.
        pub(crate) fn play(&mut self, sound: Sound) {
            let Some(sink) = self.sink() else {
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

        /// Включает звук по кругу, выключает (`None`) или оставляет как есть,
        /// если уже играет именно он.
        pub(crate) fn set_loop(&mut self, wanted: Option<Sound>) {
            if self.looping.as_ref().map(|(sound, _)| *sound) == wanted {
                return;
            }
            // Остановка: `Player` без `detach` замолкает, когда его бросают.
            self.looping = None;
            let Some(sound) = wanted else {
                return;
            };
            let Some(sink) = self.sink() else {
                return;
            };
            match Decoder::new_looped(Cursor::new(sound.bytes())) {
                Ok(decoder) => {
                    let player = Player::connect_new(sink.mixer());
                    player.set_volume(BASE_VOLUME);
                    player.append(decoder);
                    self.looping = Some((sound, player));
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

        #[allow(
            clippy::unused_self,
            clippy::needless_pass_by_ref_mut,
            reason = "тот же интерфейс, что у настоящего"
        )]
        pub(crate) fn set_loop(&mut self, _wanted: Option<Sound>) {}
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
    fn incoming_call_beats_outgoing_and_dnd_silences_it() {
        assert_eq!(ringtone(true, true, false), Some(Sound::CallRinging));
        assert_eq!(ringtone(true, false, true), None);
        // «Не беспокоить» не мешает слышать собственный исходящий вызов.
        assert_eq!(ringtone(true, true, true), Some(Sound::CallCalling));
        assert_eq!(ringtone(false, true, false), Some(Sound::CallCalling));
        assert_eq!(ringtone(false, false, false), None);
    }

    fn alert() -> MessageAlert {
        MessageAlert {
            own: false,
            dm: false,
            muted: false,
            level: bs_types::NotificationLevel::Mentions,
            direct: false,
            everyone: false,
            role: false,
            suppress_everyone: false,
            suppress_roles: false,
        }
    }

    #[test]
    fn message_rule_follows_the_original() {
        use bs_types::NotificationLevel::{All, Nothing};
        // Свои — никогда; личные — всегда, даже при «ничего» и mute.
        assert!(!message_alerts(&MessageAlert {
            own: true,
            dm: true,
            ..alert()
        }));
        assert!(message_alerts(&MessageAlert {
            dm: true,
            muted: true,
            level: Nothing,
            ..alert()
        }));
        // Канал: mute и «ничего» глушат даже прямое упоминание.
        assert!(!message_alerts(&MessageAlert {
            muted: true,
            level: All,
            ..alert()
        }));
        assert!(!message_alerts(&MessageAlert {
            level: Nothing,
            direct: true,
            ..alert()
        }));
        assert!(message_alerts(&MessageAlert {
            level: All,
            ..alert()
        }));
        // «Только упоминания»: без упоминания тихо.
        assert!(!message_alerts(&alert()));
        assert!(message_alerts(&MessageAlert {
            direct: true,
            ..alert()
        }));
    }

    #[test]
    fn mass_mentions_respect_the_suppress_switches() {
        let everyone = MessageAlert {
            everyone: true,
            ..alert()
        };
        assert!(message_alerts(&everyone));
        assert!(!message_alerts(&MessageAlert {
            suppress_everyone: true,
            ..everyone
        }));
        let role = MessageAlert {
            role: true,
            ..alert()
        };
        assert!(message_alerts(&role));
        assert!(!message_alerts(&MessageAlert {
            suppress_roles: true,
            ..role
        }));
        // Выключатели не гасят прямое упоминание.
        let both = MessageAlert {
            direct: true,
            suppress_everyone: true,
            suppress_roles: true,
            ..everyone
        };
        assert!(message_alerts(&both));
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

    const ALL: [Sound; 10] = [
        Sound::UserJoin,
        Sound::UserLeave,
        Sound::Disconnect,
        Sound::Mute,
        Sound::Unmute,
        Sound::Deafen,
        Sound::Undeafen,
        Sound::Message,
        Sound::CallRinging,
        Sound::CallCalling,
    ];

    #[test]
    fn every_sound_is_a_decodable_vorbis_file() {
        for sound in ALL {
            assert_eq!(&sound.bytes()[..4], b"OggS", "{sound:?}");
        }
    }
}
