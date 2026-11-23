//! Настройки канала и категории: «Основное» и «Права доступа».
//!
//! Тексты — из `spaces.json` (`channel.*`, `category.*`, `permissions.*`)
//! оригинала. Кнопки блокируются по правам как подсказка; решает сервер.

use bs_api::{ChannelUpdate, OverrideScope, PermissionOverride};
use bs_permissions::{Permissions, parse_permission_string, permissions_to_string};
use bs_types::OverrideTargetType;
use egui::{Margin, RichText};

use super::space_settings::{PERMISSION_GROUPS, Rights, rights_for};
use crate::bridge::{BridgeRequest, SpaceEvent, SpaceOp};
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::{theme, widgets};

const NAME_MAX: usize = 100;
const TOPIC_MAX: usize = 1024;

/// Канал или категория.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// Канал.
    Channel(String),
    /// Категория.
    Category(String),
}

impl Target {
    fn scope(&self) -> OverrideScope {
        match self {
            Self::Channel(id) => OverrideScope::Channel(id.clone()),
            Self::Category(id) => OverrideScope::Category(id.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Access,
    Bridges,
    Webhooks,
}

/// Состояние окна.
#[derive(Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "независимые флаги интерфейса"
)]
pub(crate) struct ChannelSettings {
    origin: usize,
    space_id: String,
    target: Target,
    voice: bool,
    tab: Tab,
    name: String,
    topic: String,
    saved_name: String,
    saved_topic: String,
    overrides: Option<Vec<PermissionOverride>>,
    overrides_requested: bool,
    /// Выбранная цель переопределения и её черновик.
    selected: Option<(OverrideTargetType, String)>,
    draft: Option<(Permissions, Permissions)>,
    delete_armed: bool,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    member_search: String,
    /// Мосты канала (после загрузки).
    bridges: Option<bs_api::ChannelBridges>,
    bridges_requested: bool,
    /// Черновик новой связи: сеть и чат.
    bridge_kind: String,
    bridge_chat: String,
    /// Вебхуки канала (после загрузки).
    webhooks: Option<Vec<bs_api::Webhook>>,
    webhooks_requested: bool,
    /// Имя нового вебхука.
    webhook_name: String,
}

fn bits(raw: &str) -> Permissions {
    parse_permission_string(raw).unwrap_or(Permissions::empty())
}

impl ChannelSettings {
    /// Открывает окно для канала или категории пространства.
    pub(crate) fn open(
        bridge: &Bridge,
        origin: usize,
        space_id: &str,
        target: Target,
    ) -> Option<Self> {
        let space = bridge.cache_at(origin)?.space(space_id)?;
        let (name, topic, voice) = match &target {
            Target::Channel(id) => {
                let channel = space.channels.iter().find(|c| c.id == *id)?;
                (
                    channel.name.clone(),
                    channel.topic.clone().unwrap_or_default(),
                    channel.channel_type == bs_types::ChannelType::Voice,
                )
            }
            Target::Category(id) => {
                let category = space.categories.iter().find(|c| c.id == *id)?;
                (category.name.clone(), String::new(), false)
            }
        };
        Some(Self {
            origin,
            space_id: space_id.to_owned(),
            target,
            voice,
            tab: Tab::Overview,
            saved_name: name.clone(),
            saved_topic: topic.clone(),
            name,
            topic,
            overrides: None,
            overrides_requested: false,
            selected: None,
            draft: None,
            delete_armed: false,
            busy: false,
            error: None,
            notice: None,
            member_search: String::new(),
            bridges: None,
            bridges_requested: false,
            bridge_kind: String::new(),
            bridge_chat: String::new(),
            webhooks: None,
            webhooks_requested: false,
            webhook_name: String::new(),
        })
    }

    fn is_channel(&self) -> bool {
        matches!(self.target, Target::Channel(_))
    }

    fn request(&mut self, bridge: &mut Bridge, op: SpaceOp) {
        self.busy = true;
        self.error = None;
        self.notice = None;
        bridge.focus_on(self.origin);
        bridge.request(BridgeRequest::SpaceAdmin {
            space_id: self.space_id.clone(),
            op,
        });
    }

    /// Учитывает итог действия. `false` — окно надо закрыть.
    #[allow(clippy::too_many_lines, reason = "один match по всем ответам")]
    pub(crate) fn apply(&mut self, event: &SpaceEvent) -> bool {
        match event {
            SpaceEvent::Admin {
                space_id,
                op,
                result,
                ..
            } if *space_id == self.space_id => {
                self.busy = false;
                match (op, result) {
                    (SpaceOp::DeleteChannel(id), Ok(())) if matches!(&self.target, Target::Channel(mine) if mine == id) =>
                    {
                        return false;
                    }
                    (SpaceOp::DeleteCategory(id), Ok(())) if matches!(&self.target, Target::Category(mine) if mine == id) =>
                    {
                        return false;
                    }
                    (SpaceOp::UpdateChannel(..) | SpaceOp::RenameCategory(..), Ok(())) => {
                        self.saved_name.clone_from(&self.name);
                        self.saved_topic.clone_from(&self.topic);
                        self.notice = Some(tr("Настройки сохранены").to_owned());
                    }
                    (SpaceOp::CreateWebhook(..) | SpaceOp::DeleteWebhook(..), Ok(())) => {
                        if matches!(op, SpaceOp::CreateWebhook(..)) {
                            self.webhook_name.clear();
                            self.notice = Some(tr("Вебхук создан").to_owned());
                        }
                        self.webhooks_requested = false;
                    }
                    (SpaceOp::AddBridge(..) | SpaceOp::RemoveBridge(..), Ok(())) => {
                        if matches!(op, SpaceOp::AddBridge(..)) {
                            self.bridge_chat.clear();
                            self.notice = Some(tr("Канал связан").to_owned());
                        }
                        self.bridges_requested = false;
                    }
                    (SpaceOp::PutOverride(..) | SpaceOp::DeleteOverride(..), Ok(())) => {
                        self.overrides = None;
                        self.overrides_requested = false;
                        self.draft = None;
                    }
                    (_, Err(err)) => {
                        if err.starts_with(tr("Кто-то другой")) {
                            self.overrides = None;
                            self.overrides_requested = false;
                            self.draft = None;
                        }
                        self.error = Some(err.clone());
                    }
                    _ => {}
                }
            }
            SpaceEvent::Webhooks {
                space_id,
                channel_id,
                result,
            } if *space_id == self.space_id
                && matches!(&self.target, Target::Channel(mine) if mine == channel_id) =>
            {
                match result {
                    Ok(hooks) => self.webhooks = Some(hooks.clone()),
                    Err(err) => {
                        self.webhooks = Some(Vec::new());
                        self.error = Some(err.clone());
                    }
                }
            }
            SpaceEvent::Bridges {
                space_id,
                channel_id,
                result,
            } if *space_id == self.space_id
                && matches!(&self.target, Target::Channel(mine) if mine == channel_id) =>
            {
                match result {
                    Ok(bridges) => {
                        if !bridges.available.contains(&self.bridge_kind) {
                            self.bridge_kind =
                                bridges.available.first().cloned().unwrap_or_default();
                        }
                        self.bridges = Some(bridges.clone());
                    }
                    Err(err) => {
                        self.bridges = Some(bs_api::ChannelBridges::default());
                        self.error = Some(err.clone());
                    }
                }
            }
            SpaceEvent::Overrides {
                space_id,
                scope,
                result,
            } if *space_id == self.space_id && *scope == self.target.scope() => match result {
                Ok(list) => self.overrides = Some(list.clone()),
                Err(err) => {
                    self.overrides = Some(Vec::new());
                    self.error = Some(err.clone());
                }
            },
            SpaceEvent::Left {
                space_id,
                result: Ok(()),
            } if *space_id == self.space_id => return false,
            _ => {}
        }
        true
    }
}

/// Рисует окно. Возвращает `false`, когда его надо закрыть.
pub(crate) fn show(ctx: &egui::Context, state: &mut ChannelSettings, bridge: &mut Bridge) -> bool {
    let Some(rights) = rights_for(bridge, state.origin, &state.space_id) else {
        return false;
    };
    if state.tab == Tab::Access && !state.overrides_requested && rights.manage_roles {
        state.overrides_requested = true;
        state.request(bridge, SpaceOp::ListOverrides(state.target.scope()));
        state.busy = false;
    }
    if state.tab == Tab::Bridges
        && !state.bridges_requested
        && let Target::Channel(id) = &state.target
    {
        state.bridges_requested = true;
        let id = id.clone();
        state.request(bridge, SpaceOp::ListBridges(id));
        state.busy = false;
    }
    if state.tab == Tab::Webhooks
        && !state.webhooks_requested
        && let Target::Channel(id) = &state.target
    {
        state.webhooks_requested = true;
        let id = id.clone();
        state.request(bridge, SpaceOp::ListWebhooks(id));
        state.busy = false;
    }
    let mut keep_open = true;
    let response = egui::Modal::new(egui::Id::unique("channel_settings"))
        .frame(
            egui::Frame::new()
                .fill(theme::bg_bubble())
                .stroke(egui::Stroke::new(1.0, theme::border()))
                .corner_radius(crate::theme::rounded(16))
                .inner_margin(Margin::same(0)),
        )
        .show(ctx, |ui| {
            ui.set_width(720.0);
            ui.set_height(500.0);
            ui.horizontal_top(|ui| {
                nav(ui, state, rights, &mut keep_open);
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() - 18.0);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add_space(18.0);
                            match state.tab {
                                Tab::Overview => overview(ui, state, bridge),
                                Tab::Access => access(ui, state, rights, bridge),
                                Tab::Bridges => bridges_tab(ui, state, bridge),
                                Tab::Webhooks => webhooks_tab(ui, state, bridge),
                            }
                            ui.add_space(16.0);
                        });
                });
            });
        });
    keep_open && !response.should_close()
}

fn title(state: &ChannelSettings) -> &'static str {
    if state.is_channel() {
        tr("Настройки канала")
    } else {
        tr("Настройки категории")
    }
}

fn nav(ui: &mut egui::Ui, state: &mut ChannelSettings, rights: Rights, keep_open: &mut bool) {
    ui.vertical(|ui| {
        ui.set_width(190.0);
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            widgets::section_label(ui, title(state));
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            let prefix = if !state.is_channel() {
                ""
            } else if state.voice {
                "🔊 "
            } else {
                "# "
            };
            ui.label(
                RichText::new(format!("{prefix}{}", state.saved_name)).color(theme::text_muted()),
            );
        });
        ui.add_space(8.0);
        let mut tabs = vec![(Tab::Overview, tr("Основное"), crate::icons::Icon::Gear)];
        if rights.manage_roles {
            tabs.push((Tab::Access, tr("Права доступа"), crate::icons::Icon::Users));
        }
        if state.is_channel()
            && !state.voice
            && (rights.owner
                || rights.perms.contains(Permissions::MANAGE_CHANNELS)
                || rights.perms.contains(Permissions::ADMINISTRATOR))
        {
            tabs.push((Tab::Bridges, tr("Мосты"), crate::icons::Icon::Compass));
            tabs.push((Tab::Webhooks, tr("Вебхуки"), crate::icons::Icon::Send));
        }
        for (tab, label, icon) in tabs {
            if widgets::nav_row(ui, icon, label, state.tab == tab).clicked() {
                if state.tab != tab {
                    state.bridges_requested &= tab != Tab::Bridges;
                    state.webhooks_requested &= tab != Tab::Webhooks;
                }
                state.tab = tab;
                state.error = None;
                state.notice = None;
            }
        }
        ui.add_space(ui.available_height() - 36.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            if ui.button(tr("Закрыть")).clicked() {
                *keep_open = false;
            }
        });
    });
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(19.0)
            .family(theme::bold_family())
            .color(theme::text_bright()),
    );
    ui.add_space(8.0);
}

fn status_lines(ui: &mut egui::Ui, state: &ChannelSettings) {
    if let Some(err) = &state.error {
        ui.add_space(6.0);
        ui.label(RichText::new(err).color(theme::rose()));
    }
    if let Some(note) = &state.notice {
        ui.add_space(6.0);
        ui.label(RichText::new(note).color(theme::text_muted()));
    }
}

fn overview(ui: &mut egui::Ui, state: &mut ChannelSettings, bridge: &mut Bridge) {
    heading(ui, tr("Основное"));
    let channel = state.is_channel();
    widgets::field_label(
        ui,
        if channel {
            tr("Название канала")
        } else {
            tr("Название категории")
        },
    );
    widgets::text_field(ui, &mut state.name, tr("Название"), NAME_MAX);
    if channel {
        widgets::field_label(ui, tr("Тема"));
        widgets::text_area(ui, &mut state.topic, tr("О чём этот канал?"), TOPIC_MAX, 3);
        ui.label(
            RichText::new(tr(
                "Показывается в заголовке канала. Оттуда любой может открыть полный текст.",
            ))
            .size(12.0)
            .color(theme::text_muted()),
        );
    }
    if channel && !state.voice {
        nsfw_row(ui, state, bridge);
    }
    privacy(ui, state, bridge);
    status_lines(ui, state);
    let dirty = state.name != state.saved_name || state.topic != state.saved_topic;
    if dirty {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let label = if state.busy {
                tr("Сохранение…")
            } else if channel {
                tr("Переименовать канал")
            } else {
                tr("Переименовать категорию")
            };
            let valid = !state.name.trim().is_empty();
            if ui
                .add_enabled(valid && !state.busy, theme::primary_button(label))
                .clicked()
            {
                let op = match state.target.clone() {
                    Target::Channel(id) => SpaceOp::UpdateChannel(
                        id,
                        ChannelUpdate {
                            name: (state.name != state.saved_name)
                                .then(|| state.name.trim().to_owned()),
                            topic: (state.topic != state.saved_topic)
                                .then(|| state.topic.trim().to_owned()),
                        },
                    ),
                    Target::Category(id) => {
                        SpaceOp::RenameCategory(id, state.name.trim().to_owned())
                    }
                };
                state.request(bridge, op);
            }
            if ui.button(tr("Отменить изменения")).clicked() {
                state.name.clone_from(&state.saved_name);
                state.topic.clone_from(&state.saved_topic);
            }
        });
    }
    danger_zone(ui, state, bridge);
}

/// Закрытость — запрет `VIEW_CHANNEL` для `@everyone`; роль `@everyone` имеет id пространства.
/// Переключатель «NSFW»: применяется сразу.
fn nsfw_row(ui: &mut egui::Ui, state: &mut ChannelSettings, bridge: &mut Bridge) {
    let Target::Channel(id) = state.target.clone() else {
        return;
    };
    let mut nsfw = bridge
        .cache_at(state.origin)
        .and_then(|c| c.channel(&id))
        .is_some_and(|c| c.nsfw);
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width() - 60.0);
            ui.label(
                RichText::new(tr("NSFW"))
                    .family(theme::bold_family())
                    .color(theme::text_bright()),
            );
            ui.label(
                RichText::new(tr(
                    "Откровенный или шокирующий контент: перед входом каждый подтверждает, что готов его \
                     видеть. Нужен сервер Rustspace.",
                ))
                .size(12.0)
                .color(theme::text_muted()),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::toggle(ui, &mut nsfw) && !state.busy {
                state.request(bridge, SpaceOp::SetNsfw(id.clone(), nsfw));
            }
        });
    });
}

fn privacy(ui: &mut egui::Ui, state: &mut ChannelSettings, bridge: &mut Bridge) {
    let private = bridge
        .cache_at(state.origin)
        .and_then(|c| c.space(&state.space_id))
        .is_some_and(|s| match &state.target {
            Target::Channel(id) => s
                .channels
                .iter()
                .find(|c| c.id == *id)
                .is_some_and(|c| c.is_private == Some(true)),
            Target::Category(id) => s
                .categories
                .iter()
                .find(|c| c.id == *id)
                .is_some_and(|c| c.is_private == Some(true)),
        });
    let (label, description, note) = if state.is_channel() {
        (
            tr("Закрытый канал"),
            tr("Только выбранные участники и роли смогут видеть этот канал."),
            tr(
                "Этот канал скрыт от участников без явного доступа. Пользователи с правом администратора и владельцы пространства всегда видят все каналы.",
            ),
        )
    } else {
        (
            tr("Закрытая категория"),
            tr("Только выбранные участники и роли смогут видеть каналы в этой категории."),
            tr(
                "Эта категория скрыта от участников без явного доступа. Каналы внутри наследуют это ограничение, если для них не задано явное переопределение.",
            ),
        )
    };
    ui.add_space(14.0);
    ui.separator();
    ui.add_space(6.0);
    let mut flipped = private;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(label).color(theme::text_bright()));
            ui.label(
                RichText::new(description)
                    .size(12.0)
                    .color(theme::text_muted()),
            );
        });
        ui.add_enabled_ui(!state.busy, |ui| widgets::toggle(ui, &mut flipped));
    });
    if private {
        ui.add_space(4.0);
        ui.label(RichText::new(note).size(12.0).color(theme::text_muted()));
    }
    if flipped != private {
        toggle_private(state, bridge, flipped);
    }
}

/// Читает переопределение `@everyone` и меняет бит просмотра.
fn toggle_private(state: &mut ChannelSettings, bridge: &mut Bridge, make_private: bool) {
    // Нужны актуальные биты, поэтому читаем их прямо сейчас: через список
    // переопределений, если он загружен, иначе работаем с пустым.
    let everyone = state.space_id.clone();
    let current = state.overrides.as_ref().and_then(|list| {
        list.iter()
            .find(|o| o.target_type == OverrideTargetType::Role && o.target_id == everyone)
            .cloned()
    });
    let (mut allow, mut deny) = current.as_ref().map_or_else(
        || (Permissions::empty(), Permissions::empty()),
        |o| (bits(&o.allow), bits(&o.deny)),
    );
    allow.remove(Permissions::VIEW_CHANNEL);
    if make_private {
        deny.insert(Permissions::VIEW_CHANNEL);
    } else {
        deny.remove(Permissions::VIEW_CHANNEL);
    }
    let scope = state.target.scope();
    let op = if allow.is_empty() && deny.is_empty() {
        SpaceOp::DeleteOverride(scope, OverrideTargetType::Role, everyone)
    } else {
        SpaceOp::PutOverride(
            scope,
            PermissionOverride {
                target_type: OverrideTargetType::Role,
                target_id: everyone,
                allow: permissions_to_string(allow),
                deny: permissions_to_string(deny),
            },
        )
    };
    state.request(bridge, op);
}

fn danger_zone(ui: &mut egui::Ui, state: &mut ChannelSettings, bridge: &mut Bridge) {
    ui.add_space(22.0);
    ui.label(
        RichText::new(tr("ОПАСНАЯ ЗОНА"))
            .size(11.0)
            .family(theme::bold_family())
            .color(theme::rose()),
    );
    ui.add_space(6.0);
    let channel = state.is_channel();
    let description = if channel && state.voice {
        tr(
            "Канал и все его сообщения будут удалены навсегда. Все, кто сейчас находится в этом голосовом канале, будут отключены. Это действие нельзя отменить.",
        )
    } else if channel {
        tr("Канал и все его сообщения будут удалены навсегда. Это действие нельзя отменить.")
    } else {
        tr(
            "Категория будет удалена навсегда. Каналы внутри неё будут перемещены на верхний уровень. Это действие нельзя отменить.",
        )
    };
    ui.label(RichText::new(description).color(theme::text_muted()));
    ui.add_space(6.0);
    let label = if state.delete_armed {
        tr("Нажмите ещё раз, чтобы подтвердить удаление")
    } else if channel {
        tr("Удалить канал")
    } else {
        tr("Удалить категорию")
    };
    let button = ui.add_enabled(
        !state.busy,
        egui::Button::new(RichText::new(label).color(theme::bg_rail()))
            .fill(theme::rose())
            .corner_radius(crate::theme::rounded(8)),
    );
    if button.clicked() {
        if state.delete_armed {
            let op = match state.target.clone() {
                Target::Channel(id) => SpaceOp::DeleteChannel(id),
                Target::Category(id) => SpaceOp::DeleteCategory(id),
            };
            state.request(bridge, op);
        } else {
            state.delete_armed = true;
        }
    }
}

/// Цель переопределения в списке.
struct Entry {
    kind: OverrideTargetType,
    id: String,
    name: String,
    position: i64,
}

fn access(ui: &mut egui::Ui, state: &mut ChannelSettings, rights: Rights, bridge: &mut Bridge) {
    heading(ui, tr("Права доступа"));
    status_lines(ui, state);
    let Some(list) = state.overrides.clone() else {
        ui.label(RichText::new(tr("Загрузка…")).color(theme::text_muted()));
        return;
    };
    let (roles, members) = bridge
        .cache_at(state.origin)
        .and_then(|c| c.space(&state.space_id))
        .map(|s| {
            let roles: Vec<Entry> = s
                .roles
                .iter()
                .map(|r| Entry {
                    kind: OverrideTargetType::Role,
                    id: r.id.clone(),
                    name: if r.is_everyone == Some(true) {
                        "@everyone".to_owned()
                    } else {
                        r.name.clone()
                    },
                    position: r.position,
                })
                .collect();
            let members: Vec<Entry> = s
                .members
                .iter()
                .map(|m| Entry {
                    kind: OverrideTargetType::Member,
                    id: m.user.id.clone(),
                    name: m
                        .user
                        .display_name
                        .clone()
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| m.user.username.clone()),
                    position: m.roles.iter().map(|r| r.position).max().unwrap_or(0),
                })
                .collect();
            (roles, members)
        })
        .unwrap_or_default();
    ui.label(
        RichText::new(tr(
            "Без переопределения здесь действуют права всего пространства.",
        ))
        .color(theme::text_muted()),
    );
    ui.add_space(8.0);
    let has = |kind: OverrideTargetType, id: &str| {
        list.iter()
            .any(|o| o.target_type == kind && o.target_id == id)
    };
    let mut shown: Vec<&Entry> = roles
        .iter()
        .chain(members.iter())
        .filter(|e| has(e.kind, &e.id))
        .collect();
    shown.sort_by_key(|e| {
        (
            e.kind == OverrideTargetType::Member,
            std::cmp::Reverse(e.position),
        )
    });
    if shown.is_empty() {
        ui.label(RichText::new(tr("Переопределений для ролей нет. Здесь для каждой роли действуют права всего пространства.")).color(theme::text_muted()));
    }
    for entry in shown {
        entry_row(ui, state, rights, bridge, &list, entry);
        ui.add_space(4.0);
    }
    ui.add_space(8.0);
    add_menus(ui, state, rights, &roles, &members, &list);
}

fn add_menus(
    ui: &mut egui::Ui,
    state: &mut ChannelSettings,
    rights: Rights,
    roles: &[Entry],
    members: &[Entry],
    list: &[PermissionOverride],
) {
    let free = |e: &&Entry| {
        !list
            .iter()
            .any(|o| o.target_type == e.kind && o.target_id == e.id)
            && bs_permissions::can_manage_role_at(rights.standing, e.position)
    };
    ui.horizontal(|ui| {
        ui.menu_button(tr("Добавить роль"), |ui| {
            crate::widgets::menu_spacing(ui);
            let candidates: Vec<_> = roles.iter().filter(free).collect();
            if candidates.is_empty() {
                ui.label(
                    RichText::new(tr("Больше нет ролей для добавления")).color(theme::text_muted()),
                );
            }
            for entry in candidates {
                if ui.button(&entry.name).clicked() {
                    state.selected = Some((entry.kind, entry.id.clone()));
                    state.draft = Some((Permissions::empty(), Permissions::empty()));
                    ui.close();
                }
            }
        });
        ui.menu_button(tr("Добавить участника"), |ui| {
            crate::widgets::menu_spacing(ui);
            widgets::text_field(ui, &mut state.member_search, tr("Поиск участников…"), 64);
            let needle = state.member_search.trim().to_lowercase();
            let candidates: Vec<_> = members
                .iter()
                .filter(free)
                .filter(|e| needle.is_empty() || e.name.to_lowercase().contains(&needle))
                .take(20)
                .collect();
            for entry in candidates {
                if ui.button(&entry.name).clicked() {
                    state.selected = Some((entry.kind, entry.id.clone()));
                    state.draft = Some((Permissions::empty(), Permissions::empty()));
                    ui.close();
                }
            }
        });
    });
}

fn entry_row(
    ui: &mut egui::Ui,
    state: &mut ChannelSettings,
    rights: Rights,
    bridge: &mut Bridge,
    list: &[PermissionOverride],
    entry: &Entry,
) {
    let stored = list
        .iter()
        .find(|o| o.target_type == entry.kind && o.target_id == entry.id);
    let open = state.selected.as_ref() == Some(&(entry.kind, entry.id.clone()));
    egui::Frame::new()
        .fill(theme::bg_rail())
        .corner_radius(crate::theme::rounded(10))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let prefix = if entry.kind == OverrideTargetType::Member {
                "@ "
            } else {
                ""
            };
            let header = ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{prefix}{}", entry.name)).color(theme::text_bright()),
                );
                if let Some(o) = stored {
                    let n = (bits(&o.allow) | bits(&o.deny)).bits().count_ones();
                    ui.label(
                        RichText::new(format!("· {}", plural_overrides(n)))
                            .size(12.0)
                            .color(theme::text_muted()),
                    );
                }
            });
            if header.response.interact(egui::Sense::click()).clicked() {
                if open {
                    state.selected = None;
                    state.draft = None;
                } else {
                    state.selected = Some((entry.kind, entry.id.clone()));
                    state.draft = stored.map(|o| (bits(&o.allow), bits(&o.deny)));
                }
            }
            if open {
                editor(ui, state, rights, bridge, entry, stored);
            }
        });
}

fn plural_overrides(count: u32) -> String {
    let word = match (count % 10, count % 100) {
        (1, n) if n != 11 => tr("переопределение"),
        (2..=4, n) if !(12..=14).contains(&n) => tr("переопределения"),
        _ => tr("переопределений"),
    };
    format!("{count} {word}")
}

/// Строка права с тремя состояниями: запретить, наследовать, разрешить.
fn tri_state_row(
    ui: &mut egui::Ui,
    bit: Permissions,
    label: &str,
    held: bool,
    locked: bool,
    allow: &mut Permissions,
    deny: &mut Permissions,
) {
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!locked, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let state_now = if allow.contains(bit) {
                1
            } else if deny.contains(bit) {
                -1
            } else {
                0
            };
            for (value, text, tip) in [
                (-1, "✗", tr("Запретить")),
                (0, "–", tr("Нейтрально (наследовать)")),
                (1, "✓", tr("Разрешить")),
            ] {
                let button = egui::Button::new(text)
                    .selected(state_now == value)
                    .min_size(egui::vec2(26.0, 22.0));
                if ui.add(button).on_hover_text(tip).clicked() {
                    allow.remove(bit);
                    deny.remove(bit);
                    match value {
                        1 => allow.insert(bit),
                        -1 => deny.insert(bit),
                        _ => {}
                    }
                }
            }
        });
        let color = if held {
            theme::text_bright()
        } else {
            theme::text_muted()
        };
        ui.label(RichText::new(label).color(color))
            .on_hover_text(if held {
                ""
            } else {
                tr("У вас нет этого права")
            });
    });
}

fn editor(
    ui: &mut egui::Ui,
    state: &mut ChannelSettings,
    rights: Rights,
    bridge: &mut Bridge,
    entry: &Entry,
    stored: Option<&PermissionOverride>,
) {
    let above = !bs_permissions::can_manage_role_at(rights.standing, entry.position);
    let (mut allow, mut deny) = state.draft.unwrap_or_else(|| {
        stored.map_or((Permissions::empty(), Permissions::empty()), |o| {
            (bits(&o.allow), bits(&o.deny))
        })
    });
    let original = stored.map_or((Permissions::empty(), Permissions::empty()), |o| {
        (bits(&o.allow), bits(&o.deny))
    });
    ui.add_space(6.0);
    if above {
        ui.label(
            RichText::new(if entry.kind == OverrideTargetType::Role {
                tr("Эта роль находится на уровне вашей самой высокой роли или выше. Её переопределение можно просматривать, но не изменять и не удалять.")
            } else {
                tr("Этот участник находится на вашем уровне или выше. Его переопределение можно просматривать, но не изменять и не удалять.")
            })
            .size(12.0)
            .color(theme::text_muted()),
        );
    }
    for (group, list) in PERMISSION_GROUPS {
        widgets::section_label(ui, tr(group));
        for (bit, label) in list {
            let held = rights.perms.contains(*bit);
            tri_state_row(
                ui,
                *bit,
                tr(label),
                held,
                above || !held,
                &mut allow,
                &mut deny,
            );
        }
    }
    state.draft = Some((allow, deny));
    if above {
        return;
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let changed = (allow, deny) != original;
        let empty = allow.is_empty() && deny.is_empty();
        if ui
            .add_enabled(
                changed && !state.busy,
                theme::primary_button(if state.busy {
                    tr("Сохранение…")
                } else {
                    tr("Сохранить")
                }),
            )
            .clicked()
        {
            let scope = state.target.scope();
            let op = if empty {
                SpaceOp::DeleteOverride(scope, entry.kind, entry.id.clone())
            } else {
                SpaceOp::PutOverride(
                    scope,
                    PermissionOverride {
                        target_type: entry.kind,
                        target_id: entry.id.clone(),
                        allow: permissions_to_string(allow),
                        deny: permissions_to_string(deny),
                    },
                )
            };
            state.request(bridge, op);
        }
        if stored.is_some()
            && ui
                .add_enabled(
                    !state.busy,
                    egui::Button::new(tr("Удалить переопределение")),
                )
                .clicked()
        {
            let op = SpaceOp::DeleteOverride(state.target.scope(), entry.kind, entry.id.clone());
            state.request(bridge, op);
        }
    });
}

/// Подпись сети.
fn network_title(kind: &str) -> &str {
    match kind {
        "telegram" => "Telegram",
        "discord" => "Discord",
        "matrix" => "Matrix",
        other => other,
    }
}

/// Как узнать идентификатор чата в сети.
fn network_hint(kind: &str) -> &'static str {
    match kind {
        "telegram" => tr(
            "Добавьте бота в группу и отправьте там /rustspace_id — бот ответит ID чата \
             (вида -100…). Чтобы бот видел все сообщения, выключите ему privacy mode: \
             @BotFather → /setprivacy → Disable.",
        ),
        "discord" => tr(
            "Пригласите бота на сервер и в нужном канале напишите !rustspace_id — бот ответит \
             ID канала. Боту нужны права читать и писать сообщения и «Управлять вебхуками» \
             (тогда в Discord видно имя автора).",
        ),
        "matrix" => tr(
            "Пригласите бота в комнату — он войдёт сам, — затем напишите !rustspace_id: бот \
             ответит ID комнаты (вида !abc:matrix.org). Зашифрованные комнаты не поддерживаются.",
        ),
        _ => "",
    }
}

/// Вкладка «Мосты»: связи канала с Telegram, Discord и Matrix.
fn bridges_tab(ui: &mut egui::Ui, state: &mut ChannelSettings, bridge: &mut Bridge) {
    heading(ui, tr("Мосты"));
    ui.label(
        RichText::new(tr(
            "Сообщения канала уходят в связанные чаты Telegram, Discord и Matrix, а оттуда \
             приходят сюда. Авторы из других сетей подписаны названием сети.",
        ))
        .color(theme::text_muted()),
    );
    ui.add_space(12.0);
    let Some(bridges) = state.bridges.clone() else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(theme::muted(tr("Загрузка…")));
        });
        return;
    };
    let Target::Channel(channel) = state.target.clone() else {
        return;
    };
    if bridges.available.is_empty() {
        egui::Frame::new()
            .fill(theme::bg_input())
            .corner_radius(crate::theme::rounded(10))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(tr("На сервере мосты не настроены")).family(theme::bold_family()),
                );
                ui.add_space(4.0);
                ui.label(theme::muted(tr(
                    "Администратор сервера задаёт токены ботов в настройках сервера: \
                     TELEGRAM_BOT_TOKEN, DISCORD_BOT_TOKEN, MATRIX_HOMESERVER и \
                     MATRIX_ACCESS_TOKEN. Подробности — в README, раздел «Мосты».",
                )));
            });
    }

    links(ui, state, &bridges.links, bridge);
    if bridges.available.is_empty() {
        return;
    }
    ui.add_space(10.0);
    link_form(ui, state, &bridges.available, &channel, bridge);
    if let Some(err) = &state.error {
        ui.add_space(8.0);
        ui.label(RichText::new(err).color(theme::rose()));
    }
    if let Some(notice) = &state.notice {
        ui.add_space(8.0);
        ui.label(RichText::new(notice).color(theme::mint()));
    }
}

/// Список связей канала.
fn links(
    ui: &mut egui::Ui,
    state: &mut ChannelSettings,
    links: &[bs_api::BridgeLink],
    bridge: &mut Bridge,
) {
    for link in links {
        egui::Frame::new()
            .fill(theme::bg_input())
            .corner_radius(crate::theme::rounded(10))
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(network_title(&link.kind)).family(theme::bold_family()));
                    ui.label(
                        RichText::new(&link.external_id)
                            .monospace()
                            .color(theme::text_muted()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!state.busy, egui::Button::new(tr("Отвязать")))
                            .clicked()
                        {
                            state.request(bridge, SpaceOp::RemoveBridge(link.id.clone()));
                        }
                    });
                });
            });
        ui.add_space(6.0);
    }
}

/// Форма новой связи: сеть, чат и подсказка, где взять ID.
fn link_form(
    ui: &mut egui::Ui,
    state: &mut ChannelSettings,
    available: &[String],
    channel: &str,
    bridge: &mut Bridge,
) {
    widgets::section_label(ui, tr("Связать с чатом"));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("bridge_kind")
            .selected_text(network_title(&state.bridge_kind).to_owned())
            .width(130.0)
            .show_ui(ui, |ui| {
                for kind in available {
                    ui.selectable_value(&mut state.bridge_kind, kind.clone(), network_title(kind));
                }
            });
        let hint = match state.bridge_kind.as_str() {
            "telegram" => "-1001234567890",
            "discord" => "123456789012345678",
            "matrix" => "!room:matrix.org",
            _ => "",
        };
        ui.add(
            egui::TextEdit::singleline(&mut state.bridge_chat)
                .hint_text(hint)
                .desired_width(240.0)
                .margin(egui::vec2(10.0, 7.0)),
        );
        let ready = !state.bridge_chat.trim().is_empty() && !state.busy;
        if ui
            .add_enabled(ready, theme::primary_button(tr("Связать")))
            .clicked()
        {
            state.request(
                bridge,
                SpaceOp::AddBridge(
                    channel.to_owned(),
                    state.bridge_kind.clone(),
                    state.bridge_chat.trim().to_owned(),
                ),
            );
        }
    });
    ui.add_space(6.0);
    ui.label(theme::muted(network_hint(&state.bridge_kind)).size(12.5));
}

/// Вкладка «Вебхуки»: адреса, по которым внешние программы пишут в канал.
fn webhooks_tab(ui: &mut egui::Ui, state: &mut ChannelSettings, bridge: &mut Bridge) {
    heading(ui, tr("Вебхуки"));
    ui.label(
        RichText::new(tr(
            "Вебхук — адрес, по которому программа пишет в канал без входа: CI, мониторинг, \
             RSS, свой бот. Формат как у вебхуков Discord (content, username, embeds), так что \
             готовые интеграции работают, если подставить этот адрес.",
        ))
        .color(theme::text_muted()),
    );
    ui.add_space(12.0);
    let Some(hooks) = state.webhooks.clone() else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(theme::muted(tr("Загрузка…")));
        });
        return;
    };
    let Target::Channel(channel) = state.target.clone() else {
        return;
    };
    for hook in &hooks {
        egui::Frame::new()
            .fill(theme::bg_input())
            .corner_radius(crate::theme::rounded(10))
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&hook.name).family(theme::bold_family()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!state.busy, egui::Button::new(tr("Удалить")))
                            .clicked()
                        {
                            state.request(bridge, SpaceOp::DeleteWebhook(hook.id.clone()));
                        }
                        if ui.button(tr("Копировать адрес")).clicked() {
                            ui.ctx().copy_text(hook.url.clone());
                            state.notice = Some(tr("Адрес скопирован").to_owned());
                        }
                    });
                });
                theme::label_fit(
                    ui,
                    RichText::new(&hook.url)
                        .monospace()
                        .size(11.5)
                        .color(theme::text_faint()),
                    ui.available_width(),
                );
            });
        ui.add_space(6.0);
    }
    ui.add_space(10.0);
    widgets::section_label(ui, tr("Новый вебхук"));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.webhook_name)
                .hint_text(tr("Имя, например GitHub"))
                .desired_width(240.0)
                .margin(egui::vec2(10.0, 7.0)),
        );
        let ready = !state.webhook_name.trim().is_empty() && !state.busy;
        if ui
            .add_enabled(ready, theme::primary_button(tr("Создать")))
            .clicked()
        {
            let name = state.webhook_name.trim().to_owned();
            state.request(bridge, SpaceOp::CreateWebhook(channel.clone(), name));
        }
    });
    ui.add_space(6.0);
    ui.label(
        theme::muted(tr(
            "Адрес — секрет: кто его знает, тот пишет в канал. Утёк — удалите вебхук и создайте новый.",
        ))
        .size(12.5),
    );
    ui.add_space(6.0);
    ui.label(
        RichText::new(
            "curl -H 'Content-Type: application/json' -d '{\"content\":\"Привет\"}' АДРЕС",
        )
        .monospace()
        .size(11.5)
        .color(theme::text_faint()),
    );
    if let Some(err) = &state.error {
        ui.add_space(8.0);
        ui.label(RichText::new(err).color(theme::rose()));
    }
    if let Some(notice) = &state.notice {
        ui.add_space(8.0);
        ui.label(RichText::new(notice).color(theme::mint()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_plural_forms() {
        assert_eq!(plural_overrides(1), "1 переопределение");
        assert_eq!(plural_overrides(2), "2 переопределения");
        assert_eq!(plural_overrides(5), "5 переопределений");
        assert_eq!(plural_overrides(11), "11 переопределений");
        assert_eq!(plural_overrides(21), "21 переопределение");
        assert_eq!(plural_overrides(22), "22 переопределения");
    }

    #[test]
    fn scope_follows_target() {
        assert_eq!(
            Target::Channel("c1".to_owned()).scope(),
            OverrideScope::Channel("c1".to_owned())
        );
        assert_eq!(
            Target::Category("g1".to_owned()).scope(),
            OverrideScope::Category("g1".to_owned())
        );
    }
}
