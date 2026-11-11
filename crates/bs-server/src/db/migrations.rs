//! Миграции SQLite оригинала (drizzle), вшитые в сервер.
//!
//! Файл сгенерирован из `drizzle/meta/_journal.json`. Миграции применяются
//! так же, как мигратором drizzle, поэтому базу можно открывать и оригинальным
//! сервером, и этим.

/// Миграция: имя, время из журнала и текст SQL.
#[derive(Debug)]
pub struct Migration {
    /// Имя файла без расширения.
    pub tag: &'static str,
    /// Поле `when` журнала: по нему drizzle решает, что уже применено.
    pub when: i64,
    /// Текст файла.
    pub sql: &'static str,
}

/// Все миграции по порядку.
pub static MIGRATIONS: &[Migration] = &[
    Migration {
        tag: "0000_lethal_wildside",
        when: 1_777_065_939_127,
        sql: include_str!("../../migrations/sqlite/0000_lethal_wildside.sql"),
    },
    Migration {
        tag: "0001_complex_screwball",
        when: 1_777_144_927_946,
        sql: include_str!("../../migrations/sqlite/0001_complex_screwball.sql"),
    },
    Migration {
        tag: "0002_peer_approval_token",
        when: 1_777_196_627_239,
        sql: include_str!("../../migrations/sqlite/0002_peer_approval_token.sql"),
    },
    Migration {
        tag: "0003_brave_inhumans",
        when: 1_777_229_997_210,
        sql: include_str!("../../migrations/sqlite/0003_brave_inhumans.sql"),
    },
    Migration {
        tag: "0004_slim_mentallo",
        when: 1_777_395_066_272,
        sql: include_str!("../../migrations/sqlite/0004_slim_mentallo.sql"),
    },
    Migration {
        tag: "0005_group_dm_metadata",
        when: 1_778_428_627_458,
        sql: include_str!("../../migrations/sqlite/0005_group_dm_metadata.sql"),
    },
    Migration {
        tag: "0006_spicy_scourge",
        when: 1_782_474_063_349,
        sql: include_str!("../../migrations/sqlite/0006_spicy_scourge.sql"),
    },
    Migration {
        tag: "0007_nervous_orphan",
        when: 1_782_832_912_087,
        sql: include_str!("../../migrations/sqlite/0007_nervous_orphan.sql"),
    },
    Migration {
        tag: "0008_cute_sebastian_shaw",
        when: 1_782_932_926_154,
        sql: include_str!("../../migrations/sqlite/0008_cute_sebastian_shaw.sql"),
    },
    Migration {
        tag: "0009_uneven_red_ghost",
        when: 1_783_011_477_517,
        sql: include_str!("../../migrations/sqlite/0009_uneven_red_ghost.sql"),
    },
    Migration {
        tag: "0010_broken_blazing_skull",
        when: 1_783_035_334_526,
        sql: include_str!("../../migrations/sqlite/0010_broken_blazing_skull.sql"),
    },
    Migration {
        tag: "0011_loose_boomer",
        when: 1_788_370_324_540,
        sql: include_str!("../../migrations/sqlite/0011_loose_boomer.sql"),
    },
    Migration {
        tag: "0012_absurd_shiver_man",
        when: 1_788_371_689_229,
        sql: include_str!("../../migrations/sqlite/0012_absurd_shiver_man.sql"),
    },
    Migration {
        tag: "0013_lucky_mysterio",
        when: 1_788_650_162_712,
        sql: include_str!("../../migrations/sqlite/0013_lucky_mysterio.sql"),
    },
    Migration {
        tag: "0014_chemical_siren",
        when: 1_789_425_505_956,
        sql: include_str!("../../migrations/sqlite/0014_chemical_siren.sql"),
    },
    Migration {
        tag: "0015_real_tomas",
        when: 1_790_028_172_955,
        sql: include_str!("../../migrations/sqlite/0015_real_tomas.sql"),
    },
    Migration {
        tag: "0016_whole_loki",
        when: 1_790_083_597_831,
        sql: include_str!("../../migrations/sqlite/0016_whole_loki.sql"),
    },
    Migration {
        tag: "0017_fat_rafael_vega",
        when: 1_790_283_423_706,
        sql: include_str!("../../migrations/sqlite/0017_fat_rafael_vega.sql"),
    },
    Migration {
        tag: "0018_peering_reason_instance_connect",
        when: 1_790_514_324_455,
        sql: include_str!("../../migrations/sqlite/0018_peering_reason_instance_connect.sql"),
    },
    Migration {
        tag: "0019_chosen_status",
        when: 1_790_529_786_715,
        sql: include_str!("../../migrations/sqlite/0019_chosen_status.sql"),
    },
    Migration {
        tag: "0020_users_home_user_id_index",
        when: 1_790_621_580_065,
        sql: include_str!("../../migrations/sqlite/0020_users_home_user_id_index.sql"),
    },
    Migration {
        tag: "0021_outbox_queue_keys",
        when: 1_790_696_637_343,
        sql: include_str!("../../migrations/sqlite/0021_outbox_queue_keys.sql"),
    },
    Migration {
        tag: "0022_peer_resync",
        when: 1_791_335_942_928,
        sql: include_str!("../../migrations/sqlite/0022_peer_resync.sql"),
    },
    Migration {
        tag: "0023_peer_status_reason",
        when: 1_791_372_547_519,
        sql: include_str!("../../migrations/sqlite/0023_peer_status_reason.sql"),
    },
    Migration {
        tag: "0024_reaction_unique",
        when: 1_791_393_454_331,
        sql: include_str!("../../migrations/sqlite/0024_reaction_unique.sql"),
    },
    Migration {
        tag: "0025_detached_identity",
        when: 1_791_404_634_631,
        sql: include_str!("../../migrations/sqlite/0025_detached_identity.sql"),
    },
    Migration {
        tag: "0026_notification_settings",
        when: 1_791_414_124_908,
        sql: include_str!("../../migrations/sqlite/0026_notification_settings.sql"),
    },
    Migration {
        tag: "0027_unread_count_indexes",
        when: 1_791_751_492_187,
        sql: include_str!("../../migrations/sqlite/0027_unread_count_indexes.sql"),
    },
    Migration {
        tag: "0028_mass_mention_preferences",
        when: 1_791_753_591_416,
        sql: include_str!("../../migrations/sqlite/0028_mass_mention_preferences.sql"),
    },
    Migration {
        tag: "0029_persistent_channel_pokes",
        when: 1_791_769_904_748,
        sql: include_str!("../../migrations/sqlite/0029_persistent_channel_pokes.sql"),
    },
    Migration {
        tag: "0030_space_owner_title",
        when: 1_791_771_387_548,
        sql: include_str!("../../migrations/sqlite/0030_space_owner_title.sql"),
    },
    Migration {
        tag: "0031_personal_stickers",
        when: 1_791_785_539_220,
        sql: include_str!("../../migrations/sqlite/0031_personal_stickers.sql"),
    },
];
