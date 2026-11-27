//! Модель экрана: сайдбар, участники, строки ленты.

#![allow(clippy::wildcard_imports)]
use super::*;

/// Что пользователь попросил сделать за пределами чата.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ChatAction {
    /// Ничего.
    #[default]
    None,
    /// Выйти из инстанса.
    Logout,
    /// Перейти на сохранённый аккаунт (номер в списке) или добавить новый.
    SwitchAccount(Option<usize>),
}

/// Выбранный канал.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedChannel {
    /// Инстанс, которому принадлежит канал (номер в мосте).
    pub(crate) origin: usize,
    /// Идентификатор канала внутри инстанса.
    pub(crate) channel: ChannelId,
    /// Канал пространства или личный канал: у них разные события и маршруты.
    pub(crate) kind: ChannelKind,
}

/// Канал в сайдбаре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarChannel {
    /// Идентификатор канала.
    pub(crate) id: String,
    /// Имя канала без `#`.
    pub(crate) name: String,
    /// Есть непрочитанные.
    pub(crate) unread: bool,
    /// Канал заглушён (свой mute или mute пространства).
    pub(crate) muted: bool,
    /// Голосовой канал.
    pub(crate) voice: bool,
    /// Кто сейчас в голосовом канале.
    pub(crate) occupants: Vec<Occupant>,
}

/// Участник голосового канала в сайдбаре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Occupant {
    /// Это мы сами.
    pub(crate) me: bool,
    /// Идентификатор пользователя.
    pub(crate) id: String,
    /// Имя.
    pub(crate) name: String,
    /// Микрофон, звук, камера, экран (`speaking` здесь не заполняется).
    pub(crate) flags: theme::OccupantFlags,
    /// Аватар (имя файла или адрес); `None` — рисуем инициалы.
    pub(crate) avatar: Option<String>,
}

/// Группа каналов: категория или каналы без категории (`name == None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarGroup {
    /// Идентификатор категории (у «без категории» его нет).
    pub(crate) id: Option<String>,
    /// Название категории.
    pub(crate) name: Option<String>,
    /// Каналы группы в порядке сервера.
    pub(crate) channels: Vec<SidebarChannel>,
}

/// Пространство в сайдбаре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarSpace {
    /// Инстанс, на котором живёт пространство.
    pub(crate) origin: usize,
    /// Идентификатор пространства (уникален только внутри инстанса).
    pub(crate) id: String,
    /// Имя пространства.
    pub(crate) name: String,
    /// Цвет значка из палитры сервера.
    pub(crate) color: Option<String>,
    /// Значение поля `icon` (картинка значка).
    pub(crate) icon: Option<String>,
    /// Каналы по категориям.
    pub(crate) groups: Vec<SidebarGroup>,
    /// Что нам здесь разрешено.
    pub(crate) rights: SpaceRights,
}

/// Права текущего пользователя в пространстве (сервер проверяет их сам; тут
/// они лишь решают, какие кнопки показать).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "независимые права")]
pub(crate) struct SpaceRights {
    /// Мы владелец.
    pub(crate) owner: bool,
    /// Создавать и менять каналы и категории.
    pub(crate) manage_channels: bool,
    /// Создавать приглашения.
    pub(crate) invite: bool,
    /// Менять настройки пространства.
    pub(crate) manage_space: bool,
    /// Исключать или блокировать участников.
    pub(crate) moderate: bool,
}

impl SpaceRights {
    /// Есть ли смысл открывать настройки пространства.
    pub(super) fn can_open_settings(self) -> bool {
        self.owner || self.manage_space || self.moderate
    }

    /// Права по полям пространства: владелец может всё, остальным — маска
    /// `myPermissions`.
    pub(super) fn of(space: &bs_types::SpaceWithChannelsAndMembers, me: &str) -> Self {
        use bs_permissions::{Permissions, has_permission_bit, parse_permission_string};
        let owner = space.space.owner_id == me;
        let bits = space
            .my_permissions
            .as_deref()
            .and_then(|raw| parse_permission_string(raw).ok())
            .unwrap_or(Permissions::empty());
        let has = |flag| owner || has_permission_bit(bits, flag);
        Self {
            owner,
            manage_channels: has(Permissions::MANAGE_CHANNELS),
            invite: has(Permissions::CREATE_INVITE),
            manage_space: has(Permissions::MANAGE_SPACE),
            moderate: has(Permissions::KICK_MEMBERS) || has(Permissions::BAN_MEMBERS),
        }
    }
}

impl SidebarSpace {
    /// Все каналы пространства подряд.
    pub(super) fn channels(&self) -> impl Iterator<Item = &SidebarChannel> {
        self.groups.iter().flat_map(|g| g.channels.iter())
    }

    /// Первый текстовый канал: его открываем по умолчанию.
    pub(super) fn first_text(&self) -> Option<&SidebarChannel> {
        self.channels().find(|c| !c.voice)
    }

    /// Есть ли непрочитанное хотя бы в одном канале.
    pub(super) fn has_unread(&self) -> bool {
        self.channels().any(|c| c.unread)
    }
}

/// Личный канал в сайдбаре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarDm {
    /// Инстанс, который доставляет эту беседу.
    pub(crate) origin: usize,
    /// Ключ беседы одинаков у всех её копий на разных инстансах.
    pub(crate) key: Option<String>,
    /// Идентификатор личного канала.
    pub(crate) id: String,
    /// Имя собеседника или название группы.
    pub(crate) label: String,
    /// Есть непрочитанные.
    pub(crate) unread: bool,
    /// Присутствие собеседника (только для беседы один на один).
    pub(crate) status: Option<UserStatus>,
    /// Начало последнего сообщения.
    pub(crate) preview: Option<String>,
    /// Время последнего сообщения (мс).
    pub(crate) at: Option<i64>,
    /// Аватар собеседника (беседа один на один) или иконка группы.
    pub(crate) avatar: Option<String>,
    /// Ключ цвета заглушки и `avatarColor` (собеседник или сама группа).
    pub(crate) face: (String, Option<String>),
    /// Чат из личного моста (Telegram и т. п.).
    pub(crate) bridged: bool,
    /// Название моста («Telegram»), если чат из него.
    pub(crate) network: Option<&'static str>,
    /// Групповая беседа (больше одного собеседника).
    pub(crate) group: bool,
}

/// Кто сидит в голосовом канале, с флагами микрофона и звука.
pub(super) fn occupants(
    cache: &Cache,
    names: &HashMap<String, String>,
    channel_id: &str,
) -> Vec<Occupant> {
    let avatars = avatar_map(cache);
    cache
        .users_in_voice(channel_id)
        .iter()
        .map(|id| {
            let state = cache.voice_user_state(id);
            Occupant {
                id: id.clone(),
                name: names.get(id).cloned().unwrap_or_else(|| id.clone()),
                flags: theme::OccupantFlags {
                    muted: state.is_some_and(|s| s.is_muted),
                    deafened: state.is_some_and(|s| s.is_deafened),
                    speaking: false,
                    camera: state.is_some_and(|s| s.is_camera_on),
                    streaming: state.is_some_and(|s| s.is_screen_sharing),
                },
                me: *id == cache.me().id,
                avatar: avatars.get(id).cloned(),
            }
        })
        .collect()
}

/// Собирает сайдбар: пространства, в них категории и каналы в порядке сервера.
#[must_use]
pub(crate) fn sidebar_items(cache: &Cache) -> Vec<SidebarSpace> {
    let names = name_map(cache);
    let mut spaces: Vec<_> = cache.spaces().collect();
    spaces.sort_by(|a, b| a.space.id.cmp(&b.space.id));
    spaces
        .into_iter()
        .map(|space| {
            let to_row = |channel: &bs_types::Channel| {
                let voice = channel.channel_type == bs_types::ChannelType::Voice;
                let muted = cache
                    .channel_policy(&space.space.id, &channel.id, now_ms())
                    .muted;
                SidebarChannel {
                    id: channel.id.clone(),
                    name: channel.name.clone(),
                    unread: !muted && cache.unread_count(&channel.id) > 0,
                    muted,
                    voice,
                    occupants: if voice {
                        occupants(cache, &names, &channel.id)
                    } else {
                        Vec::new()
                    },
                }
            };
            let mut all: Vec<&bs_types::Channel> = cache.channels(&space.space.id).collect();
            all.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.id.cmp(&b.id)));

            let mut categories: Vec<_> = space.categories.iter().collect();
            categories.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.id.cmp(&b.id)));

            let mut groups = Vec::new();
            let loose: Vec<SidebarChannel> = all
                .iter()
                .filter(|c| {
                    c.category_id
                        .as_ref()
                        .is_none_or(|id| !categories.iter().any(|cat| &cat.id == id))
                })
                .map(|c| to_row(c))
                .collect();
            if !loose.is_empty() {
                groups.push(SidebarGroup {
                    id: None,
                    name: None,
                    channels: loose,
                });
            }
            for category in categories {
                let channels: Vec<SidebarChannel> = all
                    .iter()
                    .filter(|c| c.category_id.as_ref() == Some(&category.id))
                    .map(|c| to_row(c))
                    .collect();
                groups.push(SidebarGroup {
                    id: Some(category.id.clone()),
                    name: Some(category.name.clone()),
                    channels,
                });
            }
            SidebarSpace {
                origin: 0,
                id: space.space.id.clone(),
                name: space.space.name.clone(),
                color: space.space.avatar_color.clone(),
                icon: space.space.icon.clone(),
                groups,
                rights: SpaceRights::of(space, &cache.me().id),
            }
        })
        .collect()
}

/// Личные каналы: свежие (больший snowflake) выше.
#[must_use]
pub(crate) fn dm_sidebar_items(cache: &Cache) -> Vec<SidebarDm> {
    let me = cache.me().id.clone();
    let mut dms: Vec<_> = cache.dm_channels().collect();
    dms.sort_by(|a, b| b.id.len().cmp(&a.id.len()).then_with(|| b.id.cmp(&a.id)));
    dms.into_iter()
        .map(|dm| SidebarDm {
            origin: 0,
            key: dm.federated_id.clone(),
            id: dm.id.clone(),
            label: dm_display_name(dm, &me),
            unread: cache.unread_count(&dm.id) > 0,
            status: {
                let mut others = dm.members.iter().filter(|m| m.id != me);
                match (others.next(), others.next()) {
                    (Some(other), None) => {
                        Some(cache.user_status(&other.id).unwrap_or(other.status))
                    }
                    _ => None,
                }
            },
            preview: dm_last_text(dm),
            at: dm_last_at(dm),
            bridged: dm
                .members
                .iter()
                .any(|m| super::panels::is_bridge_puppet(&m.username)),
            network: dm
                .members
                .iter()
                .find_map(|m| super::panels::bridge_network(&m.username)),
            group: dm.members.iter().filter(|m| m.id != me).count() > 1,
            avatar: {
                let mut others = dm.members.iter().filter(|m| m.id != me);
                match (others.next(), others.next()) {
                    (Some(other), None) => other.avatar.clone(),
                    _ => dm.icon.clone(),
                }
            },
            face: {
                let mut others = dm.members.iter().filter(|m| m.id != me);
                match (others.next(), others.next()) {
                    (Some(other), None) => face_of(other),
                    _ => (dm.id.clone(), None),
                }
            },
        })
        .collect()
}

/// Пространства и личные беседы всех работающих инстансов.
///
/// Пространства идут по инстансам (домашний первым). Копии одной беседы на
/// разных инстансах (одинаковый ключ) сливаются в одну; показывается копия
/// домашнего инстанса, как в оригинале: туда приходят ответы собеседника.
/// Инстанс без связи уступает копию тому, что на связи: если домашний
/// недоступен, беседа идёт через пиринговый (failover оригинала).
/// Файлы получают полный адрес владельца, чтобы их можно было грузить, не
/// зная, чьё это значение.
#[must_use]
pub(crate) fn merged_sidebar(bridge: &Bridge) -> (Vec<SidebarSpace>, Vec<SidebarDm>) {
    let parts = bridge
        .live_origins()
        .into_iter()
        .filter_map(|origin| {
            let cache = bridge.cache_at(origin)?;
            let base = bridge.base_at(origin)?;
            Some((origin, base, sidebar_items(cache), dm_sidebar_items(cache)))
        })
        .collect();
    merge_preferring(parts, |origin| bridge.is_connected(origin))
}

/// Склейка, в которой копии бесед инстансов без связи уступают копиям
/// тех, что на связи. Пространства остаются в порядке инстансов.
#[must_use]
pub(super) fn merge_preferring(
    mut parts: Vec<(usize, &str, Vec<SidebarSpace>, Vec<SidebarDm>)>,
    connected: impl Fn(usize) -> bool,
) -> (Vec<SidebarSpace>, Vec<SidebarDm>) {
    // Стабильно: на связи — вперёд, внутри групп порядок прежний.
    parts.sort_by_key(|(origin, ..)| !connected(*origin));
    let (mut spaces, dms) = merge_origins(parts);
    spaces.sort_by_key(|space| space.origin);
    (spaces, dms)
}

/// Склейка списков нескольких инстансов (см. [`merged_sidebar`]).
#[must_use]
pub(super) fn merge_origins(
    parts: Vec<(usize, &str, Vec<SidebarSpace>, Vec<SidebarDm>)>,
) -> (Vec<SidebarSpace>, Vec<SidebarDm>) {
    let mut spaces = Vec::new();
    let mut dms: Vec<SidebarDm> = Vec::new();
    for (origin, base, origin_spaces, origin_dms) in parts {
        for mut space in origin_spaces {
            space.origin = origin;
            space.icon = space.icon.map(|v| crate::images::absolute(base, &v));
            for occupant in space
                .groups
                .iter_mut()
                .flat_map(|g| g.channels.iter_mut())
                .flat_map(|c| c.occupants.iter_mut())
            {
                occupant.avatar = occupant
                    .avatar
                    .take()
                    .map(|v| crate::images::absolute(base, &v));
            }
            spaces.push(space);
        }
        for mut dm in origin_dms {
            if let Some(key) = &dm.key
                && dms.iter().any(|known| known.key.as_ref() == Some(key))
            {
                continue;
            }
            dm.origin = origin;
            dm.avatar = dm.avatar.map(|v| crate::images::absolute(base, &v));
            dms.push(dm);
        }
    }
    dms.sort_by(|a, b| {
        b.at.cmp(&a.at)
            .then_with(|| b.id.len().cmp(&a.id.len()))
            .then_with(|| b.id.cmp(&a.id))
    });
    (spaces, dms)
}

/// Участник в правой панели.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemberRow {
    /// Идентификатор пользователя.
    pub(crate) user_id: String,
    /// Отображаемое имя (ник на пространстве, иначе имя профиля).
    pub(crate) name: String,
    /// Присутствие.
    pub(crate) status: UserStatus,
    /// Владелец пространства.
    pub(crate) owner: bool,
    /// Аватар.
    pub(crate) avatar: Option<String>,
    /// Ключ цвета заглушки и `avatarColor`.
    pub(crate) face: (String, Option<String>),
    /// Активность: значок, строка и цвет.
    pub(crate) activity: Option<(crate::icons::Icon, String, egui::Color32)>,
    /// Старшая роль в пространстве (по ней группа и цвет имени).
    pub(crate) role: Option<TopRole>,
    /// Бот.
    pub(crate) bot: bool,
}

/// Старшая роль участника.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TopRole {
    /// Идентификатор.
    pub(crate) id: String,
    /// Название.
    pub(crate) name: String,
    /// Место в иерархии: больше — старше.
    pub(crate) position: i64,
    /// Цвет имени.
    pub(crate) color: egui::Color32,
}

/// Старшая роль из ролей участника (`@everyone` не в счёт), как в оригинале
/// (`memberTopRole`).
pub(crate) fn top_role(roles: &[bs_types::Role]) -> Option<TopRole> {
    roles
        .iter()
        .filter(|r| r.is_everyone != Some(true))
        .max_by_key(|r| r.position)
        .map(|r| TopRole {
            id: r.id.clone(),
            name: r.name.clone(),
            position: r.position,
            color: role_color(&r.color).unwrap_or_else(theme::text),
        })
}

/// Цвет роли `#rrggbb`.
pub(crate) fn role_color(hex: &str) -> Option<egui::Color32> {
    let digits = hex.trim().trim_start_matches('#');
    if digits.len() != 6 {
        return None;
    }
    let [_, red, green, blue] = u32::from_str_radix(digits, 16).ok()?.to_be_bytes();
    Some(egui::Color32::from_rgb(red, green, blue))
}

/// Цвет имени участника пространства: старшая роль; у владельца без ролей —
/// розовый (`memberNameColor` оригинала); иначе обычный текст.
pub(crate) fn name_color(role: Option<&TopRole>, owner: bool) -> Option<egui::Color32> {
    role.map(|r| r.color).or_else(|| owner.then(theme::rose))
}

/// Ключ цвета заглушки аватара и сохранённый цвет пользователя.
pub(crate) fn face_of(user: &bs_types::User) -> (String, Option<String>) {
    (
        user.home_user_id.clone().unwrap_or_else(|| user.id.clone()),
        user.avatar_color.clone(),
    )
}

/// Лицо для отрисовки по паре из [`face_of`].
pub(crate) fn face<'a>(name: &'a str, pair: &'a (String, Option<String>)) -> theme::Face<'a> {
    theme::Face {
        name,
        key: Some(&pair.0),
        color: pair.1.as_deref(),
    }
}

/// Секция правой панели.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MemberSection {
    /// Владелец пространства.
    Owner,
    /// В сети, по старшей роли (старшие выше).
    Role {
        /// Роль.
        id: String,
        /// Название.
        name: String,
        /// Место в иерархии.
        position: i64,
    },
    /// В сети без ролей (включая «неактивен» и «не беспокоить»).
    Online,
    /// Не в сети.
    Offline,
}

impl MemberRow {
    /// Секция, в которой участник показывается.
    pub(crate) fn section(&self) -> MemberSection {
        if self.owner {
            MemberSection::Owner
        } else if matches!(self.status, UserStatus::Offline) {
            MemberSection::Offline
        } else if let Some(role) = &self.role {
            MemberSection::Role {
                id: role.id.clone(),
                name: role.name.clone(),
                position: role.position,
            }
        } else {
            MemberSection::Online
        }
    }

    /// Цвет имени.
    pub(crate) fn name_color(&self) -> Option<egui::Color32> {
        name_color(self.role.as_ref(), self.owner)
    }
}

impl MemberSection {
    /// Заголовок секции без числа.
    pub(crate) fn title(&self) -> String {
        match self {
            Self::Owner => tr("Владелец").to_owned(),
            Self::Role { name, .. } => name.clone(),
            Self::Online => tr("В сети").to_owned(),
            Self::Offline => tr("Не в сети").to_owned(),
        }
    }

    /// Порядок секций: владелец, роли от старшей, в сети, не в сети.
    fn rank(&self) -> (u8, i64) {
        match self {
            Self::Owner => (0, 0),
            Self::Role { position, .. } => (1, -position),
            Self::Online => (2, 0),
            Self::Offline => (3, 0),
        }
    }
}

/// Порядок статусов в секции: онлайн, неактивен, не беспокоить, офлайн.
pub(super) const fn status_rank(status: UserStatus) -> u8 {
    match status {
        UserStatus::Online => 0,
        UserStatus::Idle => 1,
        UserStatus::DoNotDisturb => 2,
        UserStatus::Offline => 3,
    }
}

/// Участники пространства: владелец, затем те, кто в сети, затем остальные;
/// внутри секции — по статусу и алфавиту.
#[must_use]
pub(crate) fn member_rows(cache: &Cache, space_id: &str) -> Vec<MemberRow> {
    let Some(space) = cache.space(space_id) else {
        return Vec::new();
    };
    let owner_id = &space.space.owner_id;
    let mut rows: Vec<MemberRow> = space
        .members
        .iter()
        .map(|m| MemberRow {
            user_id: m.user.id.clone(),
            name: m.member.nickname.clone().unwrap_or_else(|| {
                author_name(m.user.display_name.as_deref(), &m.user.username).to_owned()
            }),
            status: cache.user_status(&m.user.id).unwrap_or(m.user.status),
            owner: &m.user.id == owner_id,
            avatar: m.user.avatar.clone(),
            face: face_of(&m.user),
            activity: crate::screens::activity_view::summary(cache.user_activities(&m.user.id)),
            role: top_role(&m.roles),
            bot: m.user.bot,
        })
        .collect();
    rows.sort_by(|a, b| {
        a.section()
            .rank()
            .cmp(&b.section().rank())
            .then_with(|| status_rank(a.status).cmp(&status_rank(b.status)))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    rows
}

/// Участники групповой беседы: владелец, затем в сети, затем остальные.
#[must_use]
pub(crate) fn group_member_rows(cache: &Cache, dm: &bs_types::DmChannel) -> Vec<MemberRow> {
    let mut rows: Vec<MemberRow> = dm
        .members
        .iter()
        .map(|user| MemberRow {
            user_id: user.id.clone(),
            name: author_name(user.display_name.as_deref(), &user.username).to_owned(),
            status: cache.user_status(&user.id).unwrap_or(user.status),
            owner: dm.owner_id.as_deref() == Some(user.id.as_str()),
            avatar: user.avatar.clone(),
            face: face_of(user),
            activity: crate::screens::activity_view::summary(cache.user_activities(&user.id)),
            role: None,
            bot: user.bot,
        })
        .collect();
    rows.sort_by(|a, b| {
        a.section()
            .rank()
            .cmp(&b.section().rank())
            .then_with(|| status_rank(a.status).cmp(&status_rank(b.status)))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    rows
}

/// Одна строка ленты: одинаковая для каналов и личных сообщений.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MessageRow {
    pub(super) id: String,
    pub(super) author_id: String,
    pub(super) author: String,
    pub(super) body: String,
    pub(super) edited: bool,
    /// Время создания, мс с эпохи.
    pub(super) created_at: i64,
    /// Аватар автора.
    pub(super) avatar: Option<String>,
    /// Ключ цвета заглушки (`homeUserId` или id) и сохранённый `avatarColor`.
    pub(super) face: (String, Option<String>),
    /// Вложения.
    pub(super) attachments: Vec<AttachmentView>,
    /// Карточки: превью ссылок и карточки вебхуков.
    pub(super) embeds: Vec<bs_types::Embed>,
    /// Реакции, сгруппированные по эмодзи.
    pub(super) reactions: Vec<crate::screens::body::ReactionGroup>,
    /// Сообщение, на которое это — ответ: id, автор и текст.
    pub(super) reply: Option<(String, String, String)>,
    /// Системное сообщение беседы (`type: "system"`); `None` — обычное.
    pub(super) system: Option<SystemLine>,
    /// Цвет имени автора (старшая роль в пространстве).
    pub(super) name_color: Option<egui::Color32>,
    /// Автор — бот.
    pub(super) bot: bool,
}

/// Содержимое системного сообщения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SystemLine {
    /// Известное событие.
    Event(Box<bs_types::DmSystemEvent>),
    /// Событие, которое клиент не знает: показывается нейтрально.
    Unknown,
}

/// Окно, в котором подряд идущие сообщения одного автора склеиваются.
pub(super) const GROUP_WINDOW_MS: i64 = 5 * 60 * 1000;

/// Нужно ли показывать заголовок (аватар и имя) у `cur` после `prev`.
///
/// Заголовок нужен у первого сообщения, при смене автора, паузе дольше
/// пяти минут или смене календарного дня.
#[must_use]
pub(super) fn needs_header(prev: Option<&MessageRow>, cur: &MessageRow) -> bool {
    let Some(prev) = prev else {
        return true;
    };
    prev.system.is_some()
        || cur.system.is_some()
        || prev.author_id != cur.author_id
        || cur.created_at.saturating_sub(prev.created_at) > GROUP_WINDOW_MS
        || theme::local_day(prev.created_at) != theme::local_day(cur.created_at)
}

/// Имена пользователей, известных кэшу: для упоминаний и подсказок реакций.
/// Аватары известных пользователей (участники пространств, личные чаты, мы).
pub(super) fn avatar_map(cache: &Cache) -> HashMap<String, String> {
    let mut avatars = HashMap::new();
    let users = cache
        .spaces()
        .flat_map(|space| space.members.iter().map(|m| &m.user))
        .chain(cache.dm_channels().flat_map(|dm| dm.members.iter()))
        .chain(std::iter::once(cache.me()));
    for user in users {
        if let Some(avatar) = user.avatar.as_ref().filter(|a| !a.is_empty()) {
            avatars.insert(user.id.clone(), avatar.clone());
        }
    }
    avatars
}

pub(super) fn name_map(cache: &Cache) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for space in cache.spaces() {
        for member in &space.members {
            names.insert(
                member.user.id.clone(),
                author_name(member.user.display_name.as_deref(), &member.user.username).to_owned(),
            );
        }
    }
    for dm in cache.dm_channels() {
        for user in &dm.members {
            names.insert(
                user.id.clone(),
                author_name(user.display_name.as_deref(), &user.username).to_owned(),
            );
        }
    }
    let me = cache.me();
    names.insert(
        me.id.clone(),
        author_name(me.display_name.as_deref(), &me.username).to_owned(),
    );
    names
}

pub(super) fn reaction_groups(
    reactions: &[bs_types::Reaction],
    names: &HashMap<String, String>,
    me: &str,
) -> Vec<crate::screens::body::ReactionGroup> {
    crate::screens::body::group_reactions(
        reactions.iter().map(|r| {
            (
                r.emoji.as_str(),
                r.user_id.as_str(),
                names.get(&r.user_id).map_or("?", String::as_str),
            )
        }),
        me,
    )
}

/// Строки ленты канала `channel_id` из кэша (старые → новые).
pub(super) fn message_rows(
    cache: &Cache,
    kind: ChannelKind,
    channel_id: &str,
    names: &HashMap<String, String>,
) -> Vec<MessageRow> {
    let me = cache.me().id.as_str();
    // Цвета имён по ролям пространства этого канала.
    let space_colors: HashMap<String, egui::Color32> = if kind == ChannelKind::Space {
        cache
            .channel(channel_id)
            .and_then(|channel| cache.space(&channel.space_id))
            .map(|space| {
                space
                    .members
                    .iter()
                    .filter_map(|m| {
                        let owner = m.user.id == space.space.owner_id;
                        name_color(top_role(&m.roles).as_ref(), owner)
                            .map(|color| (m.user.id.clone(), color))
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        HashMap::new()
    };
    let quote = |user: &bs_types::User, content: Option<&String>| {
        (
            author_name(user.display_name.as_deref(), &user.username).to_owned(),
            content.cloned().unwrap_or_default(),
        )
    };
    match kind {
        ChannelKind::Space => cache
            .messages(channel_id)
            .iter()
            .map(|msg| MessageRow {
                id: msg.message.id.clone(),
                author_id: msg.user.id.clone(),
                author: author_name(msg.user.display_name.as_deref(), &msg.user.username)
                    .to_owned(),
                body: msg.message.content.clone().unwrap_or_default(),
                edited: msg.message.edited_at.is_some(),
                created_at: msg.message.created_at,
                avatar: msg.user.avatar.clone(),
                face: face_of(&msg.user),
                bot: msg.user.bot,
                attachments: msg
                    .attachments
                    .iter()
                    .map(AttachmentView::from_api)
                    .collect(),
                embeds: msg.embeds.clone(),
                reactions: reaction_groups(&msg.reactions, names, me),
                reply: msg.reply_to.as_ref().map(|r| {
                    let (author, text) = quote(&r.user, r.message.content.as_ref());
                    (r.message.id.clone(), author, text)
                }),
                // Системные строки канала (тычок) — как у бесед.
                system: system_line(msg.message.message_type, msg.message.content.as_deref()),
                name_color: space_colors.get(&msg.user.id).copied(),
            })
            .collect(),
        ChannelKind::Dm => cache
            .dm_messages(channel_id)
            .iter()
            .map(|msg| MessageRow {
                id: msg.message.id.clone(),
                author_id: msg.user.id.clone(),
                author: author_name(msg.user.display_name.as_deref(), &msg.user.username)
                    .to_owned(),
                body: msg.message.content.clone().unwrap_or_default(),
                edited: msg.message.edited_at.is_some(),
                created_at: msg.message.created_at,
                avatar: msg.user.avatar.clone(),
                face: face_of(&msg.user),
                bot: msg.user.bot,
                attachments: msg
                    .attachments
                    .iter()
                    .map(AttachmentView::from_api)
                    .collect(),
                embeds: msg.embeds.clone(),
                reactions: reaction_groups(&msg.reactions, names, me),
                reply: msg.reply_to.as_ref().map(|r| {
                    let (author, text) = quote(&r.user, r.message.content.as_ref());
                    (r.message.id.clone(), author, text)
                }),
                system: system_line(msg.message.message_type, msg.message.content.as_deref()),
                name_color: None,
            })
            .collect(),
    }
}

/// Системная строка ленты, если сообщение системное.
fn system_line(kind: Option<bs_types::MessageType>, content: Option<&str>) -> Option<SystemLine> {
    (kind == Some(bs_types::MessageType::System)).then(|| {
        content
            .and_then(bs_types::DmSystemEvent::parse)
            .map_or(SystemLine::Unknown, |event| {
                SystemLine::Event(Box::new(event))
            })
    })
}

/// Подпись «кто печатает» по списку имён.
#[must_use]
pub(super) fn typing_text(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [one] => Some(crate::tf!("{one} печатает…", one = one)),
        [a, b] => Some(crate::tf!("{a} и {b} печатают…", a = a, b = b)),
        _ => Some(tr("Несколько человек печатают…").to_owned()),
    }
}

/// Текущее время, мс с эпохи.
pub(super) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
