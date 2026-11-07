//! Политика уведомлений канала: наследование уровня и mute.
//!
//! Порт `resolveChannelNotificationPolicy` оригинала: свой уровень канала
//! главнее уровня пространства, тот главнее умолчания; канал заглушён, пока
//! действует его собственный mute или mute пространства.

use bs_types::{
    ChannelNotificationPolicy, NotificationLevel, NotificationLevelSource, NotificationSetting,
};

/// Уровень пространства без сохранённого выбора и каналов, которые его наследуют.
pub const DEFAULT_LEVEL: NotificationLevel = NotificationLevel::Mentions;

/// Действует ли mute настройки в момент `now` (epoch-мс).
#[must_use]
pub fn mute_active(setting: Option<&NotificationSetting>, now: i64) -> bool {
    setting.is_some_and(|s| s.muted && s.muted_until.is_none_or(|until| until > now))
}

/// Что действует для канала: уровень после наследования и заглушение.
#[must_use]
pub fn resolve_policy(
    space: Option<&NotificationSetting>,
    channel: Option<&NotificationSetting>,
    now: i64,
) -> ChannelNotificationPolicy {
    let (level, level_source) = channel
        .and_then(|s| s.level)
        .map(|l| (l, NotificationLevelSource::Channel))
        .or_else(|| {
            space
                .and_then(|s| s.level)
                .map(|l| (l, NotificationLevelSource::Space))
        })
        .unwrap_or((DEFAULT_LEVEL, NotificationLevelSource::Default));
    let channel_muted = mute_active(channel, now);
    let space_muted = mute_active(space, now);
    let ends: Vec<Option<i64>> = [(channel_muted, channel), (space_muted, space)]
        .into_iter()
        .filter(|(active, _)| *active)
        .filter_map(|(_, s)| s.map(|s| s.muted_until))
        .collect();
    // Бессрочный mute (хотя бы один конец `None`) конца не имеет.
    let muted_until = if ends.is_empty() || ends.contains(&None) {
        None
    } else {
        ends.into_iter().flatten().max()
    };
    ChannelNotificationPolicy {
        suppress_everyone: space.and_then(|s| s.suppress_everyone) == Some(true),
        suppress_roles: space.and_then(|s| s.suppress_roles) == Some(true),
        level,
        level_source,
        muted: channel_muted || space_muted,
        muted_until,
        channel_muted,
        space_muted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(level: Option<NotificationLevel>, muted: bool, until: Option<i64>) -> NotificationSetting {
        NotificationSetting {
            suppress_everyone: None,
            suppress_roles: None,
            space_id: "s".to_owned(),
            channel_id: None,
            level,
            muted,
            muted_until: until,
            updated_at: 1,
        }
    }

    #[test]
    fn defaults_without_settings() {
        let p = resolve_policy(None, None, 10);
        assert_eq!(p.level, NotificationLevel::Mentions);
        assert_eq!(p.level_source, NotificationLevelSource::Default);
        assert!(!p.muted);
    }

    #[test]
    fn channel_level_beats_space_level() {
        let space = setting(Some(NotificationLevel::All), false, None);
        let channel = setting(Some(NotificationLevel::Nothing), false, None);
        let p = resolve_policy(Some(&space), Some(&channel), 10);
        assert_eq!(p.level, NotificationLevel::Nothing);
        assert_eq!(p.level_source, NotificationLevelSource::Channel);
        let inherit = setting(None, false, None);
        let p = resolve_policy(Some(&space), Some(&inherit), 10);
        assert_eq!(p.level, NotificationLevel::All);
        assert_eq!(p.level_source, NotificationLevelSource::Space);
    }

    #[test]
    fn timed_mute_ends_and_space_mute_covers_channels() {
        let timed = setting(None, true, Some(100));
        assert!(resolve_policy(None, Some(&timed), 99).muted);
        assert!(!resolve_policy(None, Some(&timed), 100).muted);
        let space = setting(None, true, None);
        let p = resolve_policy(Some(&space), None, 5);
        assert!(p.muted && p.space_muted && !p.channel_muted);
        assert_eq!(p.muted_until, None);
    }

    #[test]
    fn later_of_two_ends_wins() {
        let a = setting(None, true, Some(50));
        let b = setting(None, true, Some(80));
        assert_eq!(resolve_policy(Some(&a), Some(&b), 1).muted_until, Some(80));
    }

    #[test]
    fn unmuted_flag_ignores_stale_time() {
        assert!(!mute_active(Some(&setting(None, false, None)), 1));
    }
}
