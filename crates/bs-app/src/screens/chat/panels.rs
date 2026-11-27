//! Колонки и панели: пространства, каналы, пользователь, участники, шапка.

use super::{
    Align, Bridge, ChannelKind, ChatScreen, Icon, Layout, Margin, MemberRow, MemberSection, Nav,
    RichText, SelectedChannel, Sense, SidebarChannel, SidebarDm, SidebarSpace, UserStatus,
    author_name, connection_label, dm_display_name, face, face_of, group_member_rows, member_rows,
    now_ms, theme, tr, widgets,
};

/// Что показано в центральной области.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum View {
    /// Друзья и «Сейчас онлайн».
    #[default]
    Friends,
    /// Обзор пространств.
    Explore,
    /// Лента выбранного канала.
    Channel,
    /// Страница «Backspace» (проектный хаб).
    Backspace,
}

/// ПКМ по каналу: «Настройки уведомлений» и «Настройки канала».
pub(super) fn channel_menu(
    row: &egui::Response,
    space: &SidebarSpace,
    channel: &SidebarChannel,
    nav: &mut Option<Nav>,
) {
    row.context_menu(|ui| {
        crate::widgets::menu_spacing(ui);
        if ui.button(tr("Настройки уведомлений")).clicked() {
            *nav = Some(Nav::Notifications {
                origin: space.origin,
                space_id: space.id.clone(),
                channel_id: Some(channel.id.clone()),
            });
            ui.close();
        }
        if space.rights.manage_channels && ui.button(tr("Настройки канала")).clicked()
        {
            *nav = Some(Nav::ChannelSettings {
                origin: space.origin,
                space_id: space.id.clone(),
                target: crate::screens::channel_settings::Target::Channel(channel.id.clone()),
            });
            ui.close();
        }
    });
}

/// Шестерёнка настроек на строке канала при наведении, как в оригинале
/// (`ChannelSidebar`: только с правом управлять каналами). Возвращает `true`,
/// если щелчок пришёлся на неё — тогда строка канал не открывает.
pub(super) fn channel_gear(
    ui: &egui::Ui,
    row: &egui::Response,
    space: &SidebarSpace,
    channel: &SidebarChannel,
    nav: &mut Option<Nav>,
) -> bool {
    if !space.rights.manage_channels {
        return false;
    }
    // Значок «без звука» стоит у правого края — шестерёнка левее него.
    let shift = if channel.muted { 40.0 } else { 18.0 };
    let center = egui::pos2(row.rect.right() - shift, row.rect.center().y);
    let gear = egui::Rect::from_center_size(center, egui::Vec2::splat(20.0));
    // Только когда указатель над самой строкой (не под окном поверх неё).
    if !row.contains_pointer() {
        return false;
    }
    let over_gear = ui
        .ctx()
        .pointer_hover_pos()
        .is_some_and(|p| gear.contains(p));
    let color = if over_gear {
        theme::text_bright()
    } else {
        theme::text_muted()
    };
    crate::icons::paint(ui.painter(), center, 15.0, Icon::Gear, color);
    if over_gear {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        egui::Tooltip::always_open(
            ui.ctx().clone(),
            ui.layer_id(),
            row.id.with("gear_tip"),
            egui::PopupAnchor::Pointer,
        )
        .show(|ui| ui.label(tr("Настройки канала")));
    }
    let clicked = row.clicked() && row.interact_pointer_pos().is_some_and(|p| gear.contains(p));
    if clicked {
        *nav = Some(Nav::ChannelSettings {
            origin: space.origin,
            space_id: space.id.clone(),
            target: crate::screens::channel_settings::Target::Channel(channel.id.clone()),
        });
    }
    clicked
}

/// ПКМ по беседе: позвать собеседника в своё пространство (один на один,
/// PR #485 оригинала) и удалить чат у себя.
fn dm_menu(response: &egui::Response, dm: &SidebarDm, nav: &mut Option<Nav>) {
    response.context_menu(|ui| {
        crate::widgets::menu_spacing(ui);
        if dm.status.is_some() && ui.button(tr("Пригласить в пространство…")).clicked()
        {
            *nav = Some(Nav::InvitePerson {
                origin: dm.origin,
                dm_id: dm.id.clone(),
            });
            ui.close();
        }
        super::folders::dm_menu(ui, &dm.id);
        if dm.bridged && ui.button(tr("Перечитать историю")).clicked() {
            *nav = Some(Nav::ResyncDm {
                origin: dm.origin,
                dm_id: dm.id.clone(),
            });
            ui.close();
        }
        if ui
            .button(RichText::new(tr("Удалить чат")).color(theme::rose()))
            .clicked()
        {
            *nav = Some(Nav::DeleteDm(crate::screens::delete_dm::DeleteDm::new(
                dm.origin,
                dm.id.clone(),
                dm.label.clone(),
            )));
            ui.close();
        }
    });
}

/// Шапка пространства: имя (ПКМ — меню) и кнопка приглашения.
pub(super) fn space_header(ui: &mut egui::Ui, space: &SidebarSpace, nav: &mut Option<Nav>) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        let width = ui.available_width() - if space.rights.invite { 40.0 } else { 10.0 };
        ui.scope(|ui| {
            ui.set_max_width(width.max(1.0));
            ui.add(
                egui::Label::new(
                    RichText::new(&space.name)
                        .size(15.5)
                        .family(theme::bold_family())
                        .color(theme::text_bright()),
                )
                .truncate()
                .sense(Sense::click()),
            )
        })
        .inner
        .context_menu(|ui| {
            crate::widgets::menu_spacing(ui);
            space_menu(ui, space, nav);
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(10.0);
            if space.rights.invite
                && ui
                    .add(widgets::flat_icon(Icon::Users, 20.0, theme::text_muted()))
                    .on_hover_text(tr("Пригласить людей"))
                    .clicked()
            {
                *nav = Some(Nav::Invite {
                    origin: space.origin,
                    space_id: space.id.clone(),
                });
            }
            if space.rights.can_open_settings()
                && ui
                    .add(widgets::flat_icon(Icon::Gear, 20.0, theme::text_muted()))
                    .on_hover_text(tr("Настройки пространства"))
                    .clicked()
            {
                *nav = Some(Nav::SpaceSettings {
                    origin: space.origin,
                    space_id: space.id.clone(),
                });
            }
        });
    });
}

/// Кнопки «Создать канал» и «Создать категорию» под списком каналов.
pub(super) fn create_buttons(
    ui: &mut egui::Ui,
    space: &SidebarSpace,
    categories: &[(String, String)],
    nav: &mut Option<Nav>,
) {
    ui.add_space(12.0);
    for (label, category) in [
        (tr("Создать канал"), false),
        (tr("Создать категорию"), true),
    ] {
        if widgets::nav_row(ui, Icon::Plus, label, false).clicked() {
            *nav = Some(Nav::SpaceDialog(if category {
                crate::screens::space_dialogs::SpaceDialog::category(space.origin, &space.id)
            } else {
                crate::screens::space_dialogs::SpaceDialog::channel(
                    space.origin,
                    &space.id,
                    categories.to_vec(),
                    None,
                )
            }));
        }
    }
}

/// Меню пространства (ПКМ по значку, шапке или пустому месту сайдбара).
pub(super) fn space_menu(ui: &mut egui::Ui, space: &SidebarSpace, nav: &mut Option<Nav>) {
    use crate::screens::space_dialogs::SpaceDialog;
    let categories: Vec<(String, String)> = space
        .groups
        .iter()
        .filter_map(|g| Some((g.id.clone()?, g.name.clone()?)))
        .collect();
    if space.rights.invite && ui.button(tr("Пригласить людей")).clicked() {
        *nav = Some(Nav::Invite {
            origin: space.origin,
            space_id: space.id.clone(),
        });
        ui.close();
    }
    if ui
        .button(tr("Мой профиль здесь…"))
        .on_hover_text(tr(
            "Свои аватар, баннер и статус только для этого пространства",
        ))
        .clicked()
    {
        *nav = Some(Nav::SpaceProfile(
            crate::screens::space_profile::SpaceProfileDialog::open(
                space.origin,
                &space.id,
                &space.name,
            ),
        ));
        ui.close();
    }
    if ui.button(tr("Настройки уведомлений")).clicked() {
        *nav = Some(Nav::Notifications {
            origin: space.origin,
            space_id: space.id.clone(),
            channel_id: None,
        });
        ui.close();
    }
    if space.rights.can_open_settings() && ui.button(tr("Настройки пространства")).clicked()
    {
        *nav = Some(Nav::SpaceSettings {
            origin: space.origin,
            space_id: space.id.clone(),
        });
        ui.close();
    }
    if space.rights.manage_channels {
        if ui.button(tr("Создать канал")).clicked() {
            *nav = Some(Nav::SpaceDialog(SpaceDialog::channel(
                space.origin,
                &space.id,
                categories,
                None,
            )));
            ui.close();
        }
        if ui.button(tr("Создать категорию")).clicked() {
            *nav = Some(Nav::SpaceDialog(SpaceDialog::category(
                space.origin,
                &space.id,
            )));
            ui.close();
        }
    }
    if !space.rights.owner {
        if space.rights.invite || space.rights.manage_channels {
            ui.separator();
        }
        if ui
            .button(RichText::new(tr("Покинуть пространство")).color(theme::rose()))
            .clicked()
        {
            *nav = Some(Nav::SpaceDialog(SpaceDialog::leave(
                space.origin,
                &space.id,
                &space.name,
            )));
            ui.close();
        }
    }
}

impl ChatScreen {
    /// Всплывающее сообщение внизу по центру.
    pub(super) fn show_toast(&mut self, ctx: &egui::Context, now: f64) {
        let Some((text, until)) = &self.toast else {
            return;
        };
        if now >= *until {
            self.toast = None;
            return;
        }
        egui::Area::new(egui::Id::unique("toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -28.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::bg_bubble())
                    .stroke(egui::Stroke::new(1.0, theme::border()))
                    .corner_radius(crate::theme::rounded(10))
                    .inner_margin(Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).color(theme::text_bright()));
                    });
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    /// Колонка пространств: «домой», пространства, «создать», «обзор».
    pub(super) fn show_rail(
        &self,
        ui: &mut egui::Ui,
        items: &[SidebarSpace],
        dms: &[SidebarDm],
        bridge_hosts: &[String],
        nav: &mut Option<Nav>,
    ) {
        let dm_unread = dms.iter().any(|d| d.unread);
        egui::Panel::left("rail")
            .exact_size(72.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::rail_frame())
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("rail_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        let home = self.active_space.is_none() && self.view == View::Friends;
                        if theme::rail_home(ui, home, dm_unread)
                            .on_hover_text(tr("Главная"))
                            .clicked()
                        {
                            *nav = Some(Nav::Friends);
                        }
                        theme::rail_separator(ui);
                        for space in items {
                            let selected = self
                                .active_space
                                .as_ref()
                                .is_some_and(|(o, id)| *o == space.origin && *id == space.id);
                            let gradient = theme::space_gradient(&space.id, space.color.as_deref());
                            let response = theme::rail_button(
                                ui,
                                &theme::first_letter(&space.name),
                                gradient,
                                selected,
                                space.has_unread(),
                                space.icon.as_deref(),
                            );
                            let owner = bridge_hosts.get(space.origin).cloned().unwrap_or_default();
                            if space.origin != 0 {
                                widgets::origin_badge(ui, response.rect, &owner);
                            }
                            let hover = if space.origin == 0 {
                                space.name.clone()
                            } else {
                                format!("{} · {owner}", space.name)
                            };
                            let response = response.on_hover_text(hover);
                            response.context_menu(|ui| {
                                crate::widgets::menu_spacing(ui);
                                space_menu(ui, space, nav);
                            });
                            if response.clicked() && !selected {
                                *nav = Some(Nav::Space((space.origin, space.id.clone())));
                            }
                        }
                        if widgets::rail_action(ui, Icon::Plus, theme::mint(), false)
                            .on_hover_text(tr("Создать пространство"))
                            .clicked()
                        {
                            *nav = Some(Nav::CreateSpace);
                        }
                        if widgets::rail_action(ui, Icon::Join, theme::sky(), false)
                            .on_hover_text(tr("Присоединиться к пространству"))
                            .clicked()
                        {
                            *nav = Some(Nav::SpaceDialog(
                                crate::screens::space_dialogs::SpaceDialog::join(),
                            ));
                        }
                        if widgets::rail_action(
                            ui,
                            Icon::Compass,
                            theme::sky(),
                            self.view == View::Explore,
                        )
                        .on_hover_text(tr("Обзор пространств"))
                        .clicked()
                        {
                            *nav = Some(Nav::Explore);
                        }
                    });
            });
    }

    /// Вторая колонка: поиск, навигация, беседы или каналы, панель пользователя.
    pub(super) fn show_left_column(
        &mut self,
        ui: &mut egui::Ui,
        bridge: &Bridge,
        items: &[SidebarSpace],
        dms: &[SidebarDm],
        nav: &mut Option<Nav>,
    ) {
        let width = crate::columns::width(ui.ctx(), crate::columns::Side::Left);
        let panel = egui::Panel::left("channels")
            .resizable(false)
            .exact_size(width)
            .show_separator_line(false)
            .frame(theme::module(theme::bg_sidebar()))
            .show(ui, |ui| {
                egui::Panel::top("sidebar_search")
                    .exact_size(52.0)
                    .resizable(false)
                    .show_separator_line(false)
                    .frame(egui::Frame::new().inner_margin(Margin::symmetric(10, 10)))
                    .show(ui, |ui| {
                        let hint = if self.active_space.is_some() {
                            tr("Найти канал")
                        } else {
                            tr("Найти или начать беседу")
                        };
                        let response = widgets::search_box(ui, &mut self.sidebar_filter, hint);
                        // Ctrl+K (⌘K) — к поиску.
                        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K)) {
                            response.request_focus();
                        }
                        let submitted =
                            response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if submitted
                            && self.active_space.is_none()
                            && !self.sidebar_filter.trim().is_empty()
                        {
                            *nav = Some(Nav::SearchPeople(self.sidebar_filter.trim().to_owned()));
                        }
                    });

                Self::show_user_bar(ui, bridge, &self.voice, nav);
                let dock = crate::voice::show_dock(ui, &self.voice);
                if dock != crate::voice::DockAction::None {
                    *nav = Some(Nav::Voice(dock));
                }

                let filter = self.sidebar_filter.trim().to_lowercase();
                let hub_dot = crate::screens::hub::has_update(bridge);
                egui::ScrollArea::vertical()
                    .id_salt("channel_list")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if let Some(space) = self.active_space.as_ref().and_then(|(origin, id)| {
                            items.iter().find(|s| s.origin == *origin && &s.id == id)
                        }) {
                            self.channel_list(ui, space, &filter, nav);
                        } else {
                            let saved = Self::saved_chat(bridge);
                            self.home_list(ui, dms, saved.as_deref(), &filter, hub_dot, nav);
                        }
                    });
            });
        crate::columns::handle(ui, panel.response.rect, crate::columns::Side::Left);
    }

    /// Голосовой канал: строка входа и участники под ней.
    pub(super) fn voice_channel(
        &self,
        ui: &mut egui::Ui,
        space: &SidebarSpace,
        channel: &SidebarChannel,
        nav: &mut Option<Nav>,
    ) {
        let active = self.voice.call.as_ref().is_some_and(|c| {
            (c.origin, c.channel_id.as_str()) == (space.origin, channel.id.as_str())
        });
        let row = theme::voice_row(ui, &channel.name, active).on_hover_text(if active {
            tr("Нажмите, чтобы отключиться")
        } else {
            tr("Нажмите, чтобы подключиться")
        });
        if channel.muted {
            theme::muted_mark(ui, &row.rect);
        }
        channel_menu(&row, space, channel, nav);
        let streamer = channel
            .occupants
            .iter()
            .find(|o| o.flags.streaming && !o.me);
        if let Some(streamer) = streamer.filter(|_| row.hovered()) {
            crate::stream_preview::hovered(
                ui.ctx(),
                crate::stream_preview::Hover {
                    origin: space.origin,
                    channel_id: channel.id.clone(),
                    user_id: streamer.id.clone(),
                    name: streamer.name.clone(),
                    rect: row.rect,
                },
            );
        }
        if !channel_gear(ui, &row, space, channel, nav) && row.clicked() {
            *nav = Some(Nav::VoiceJoin {
                origin: space.origin,
                channel_id: channel.id.clone(),
                channel: channel.name.clone(),
                space: space.name.clone(),
                space_id: space.id.clone(),
            });
        }
        for occupant in &channel.occupants {
            let row = theme::occupant_row(
                ui,
                &occupant.name,
                theme::OccupantFlags {
                    speaking: self.voice.speaking.contains(&occupant.id),
                    ..occupant.flags
                },
                occupant.avatar.as_deref(),
            );
            if occupant.flags.streaming && !occupant.me && row.hovered() {
                crate::stream_preview::hovered(
                    ui.ctx(),
                    crate::stream_preview::Hover {
                        origin: space.origin,
                        channel_id: channel.id.clone(),
                        user_id: occupant.id.clone(),
                        name: occupant.name.clone(),
                        rect: row.rect,
                    },
                );
            }
            if !occupant.me {
                let others: Vec<(String, String)> = space
                    .groups
                    .iter()
                    .flat_map(|g| g.channels.iter())
                    .filter(|c| c.voice && c.id != channel.id)
                    .map(|c| (c.id.clone(), c.name.clone()))
                    .collect();
                row.context_menu(|ui| {
                    crate::widgets::menu_spacing(ui);
                    if let Some(audio) = crate::voice::local_audio_menu(
                        ui,
                        &self.voice.prefs,
                        &occupant.id,
                        crate::voice_media::AudioSource::Voice,
                    ) {
                        *nav = Some(Nav::LocalAudio(audio));
                    }
                    ui.separator();
                    if let Some(op) = crate::voice::moderation_menu(ui, &occupant.id, &others) {
                        *nav = Some(Nav::VoiceMod {
                            origin: space.origin,
                            op,
                        });
                        ui.close();
                    }
                });
            }
        }
    }

    /// Каналы пространства по категориям.
    pub(super) fn channel_list(
        &self,
        ui: &mut egui::Ui,
        space: &SidebarSpace,
        filter: &str,
        nav: &mut Option<Nav>,
    ) {
        space_header(ui, space, nav);
        ui.add_space(6.0);
        let categories: Vec<(String, String)> = space
            .groups
            .iter()
            .filter_map(|g| Some((g.id.clone()?, g.name.clone()?)))
            .collect();
        for group in &space.groups {
            let visible: Vec<_> = group
                .channels
                .iter()
                .filter(|c| filter.is_empty() || c.name.to_lowercase().contains(filter))
                .collect();
            if visible.is_empty() && (group.name.is_none() || !filter.is_empty()) {
                continue;
            }
            if let Some(name) = &group.name {
                let header = theme::category_header(ui, name, space.rights.manage_channels);
                if header.plus.is_some_and(|plus| plus.clicked()) {
                    *nav = Some(Nav::SpaceDialog(
                        crate::screens::space_dialogs::SpaceDialog::channel(
                            space.origin,
                            &space.id,
                            categories.clone(),
                            group.id.clone(),
                        ),
                    ));
                }
                if let Some(id) = group.id.as_ref().filter(|_| space.rights.manage_channels) {
                    header.label.context_menu(|ui| {
                        crate::widgets::menu_spacing(ui);
                        if ui.button(tr("Настройки категории")).clicked() {
                            *nav = Some(Nav::ChannelSettings {
                                origin: space.origin,
                                space_id: space.id.clone(),
                                target: crate::screens::channel_settings::Target::Category(
                                    id.clone(),
                                ),
                            });
                            ui.close();
                        }
                    });
                }
            }
            for channel in visible {
                if channel.voice {
                    self.voice_channel(ui, space, channel, nav);
                    continue;
                }
                let selected = self.view == View::Channel
                    && self.selected_channel.as_ref().is_some_and(|current| {
                        current.kind == ChannelKind::Space
                            && current.origin == space.origin
                            && current.channel.as_str() == channel.id
                    });
                let row = theme::list_row(
                    ui,
                    &format!("#   {}", channel.name),
                    selected,
                    channel.unread,
                );
                if channel.muted {
                    theme::muted_mark(ui, &row.rect);
                }
                channel_menu(&row, space, channel, nav);
                if !channel_gear(ui, &row, space, channel, nav) && row.clicked() {
                    *nav = Some(Nav::Channel(SelectedChannel {
                        origin: space.origin,
                        channel: channel.id.clone(),
                        kind: ChannelKind::Space,
                    }));
                }
            }
        }
        if space.rights.manage_channels && filter.is_empty() {
            create_buttons(ui, space, &categories, nav);
        }
        let rest = ui.available_rect_before_wrap();
        if rest.height() > 4.0 {
            ui.interact(rest, ui.scope_id().with("sidebar_rest"), Sense::click())
                .context_menu(|ui| {
                    crate::widgets::menu_spacing(ui);
                    space_menu(ui, space, nav);
                });
        }
    }

    /// «Избранное» и «Закладки» в списке главной.
    fn saved_rows(&self, ui: &mut egui::Ui, saved: Option<&str>, nav: &mut Option<Nav>) {
        let saved_open = self.view == View::Channel
            && saved.is_some()
            && self
                .selected_channel
                .as_ref()
                .is_some_and(|c| c.origin == 0 && Some(c.channel.as_str()) == saved);
        if widgets::nav_row(ui, Icon::Bookmark, tr("Избранное"), saved_open)
            .on_hover_text(tr("Чат с самим собой: заметки, ссылки, файлы"))
            .clicked()
        {
            *nav = Some(Nav::Saved);
        }
        if widgets::nav_row(ui, Icon::Pin, tr("Закладки"), false)
            .on_hover_text(tr("Сохранённые сообщения из всех чатов"))
            .clicked()
        {
            *nav = Some(Nav::Bookmarks);
        }
    }

    /// Главная: «Друзья», «Обзор» и личные беседы.
    pub(super) fn home_list(
        &self,
        ui: &mut egui::Ui,
        dms: &[SidebarDm],
        saved: Option<&str>,
        filter: &str,
        hub_dot: bool,
        nav: &mut Option<Nav>,
    ) {
        if widgets::nav_row(ui, Icon::User, tr("Друзья"), self.view == View::Friends).clicked()
        {
            *nav = Some(Nav::Friends);
        }
        if widgets::nav_row(ui, Icon::Compass, tr("Обзор"), self.view == View::Explore).clicked()
        {
            *nav = Some(Nav::Explore);
        }
        if widgets::brand_row(
            ui,
            crate::brand::NAME,
            self.view == View::Backspace,
            hub_dot,
        )
        .on_hover_text(if hub_dot {
            tr("Сервер обновлён")
        } else {
            tr("О проекте Rustspace")
        })
        .clicked()
        {
            *nav = Some(Nav::Backspace);
        }
        self.saved_rows(ui, saved, nav);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            widgets::section_label(ui, tr("Личные сообщения"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(10.0);
                if ui
                    .add(widgets::flat_icon(Icon::Plus, 18.0, theme::text_muted()))
                    .on_hover_text(tr("Найти человека"))
                    .clicked()
                {
                    *nav = Some(Nav::SearchPeople(String::new()));
                }
                if ui
                    .add(widgets::flat_icon(Icon::Users, 18.0, theme::text_muted()))
                    .on_hover_text(tr("Создать группу"))
                    .clicked()
                {
                    *nav = Some(Nav::GroupCreate);
                }
            });
        });
        ui.add_space(4.0);
        let tab = super::folders::tabs_bar(ui, dms);
        let folders = crate::prefs::get().chat_folders;
        ui.add_space(4.0);
        let now = now_ms();
        let mut shown = 0;
        for dm in dms
            .iter()
            .filter(|d| !(d.origin == 0 && Some(d.id.as_str()) == saved))
            .filter(|d| tab.matches(d, &folders))
            .filter(|d| filter.is_empty() || d.label.to_lowercase().contains(filter))
        {
            shown += 1;
            let selected = self.view == View::Channel
                && self.selected_channel.as_ref().is_some_and(|current| {
                    current.kind == ChannelKind::Dm
                        && current.origin == dm.origin
                        && current.channel.as_str() == dm.id
                });
            let stamp = dm.at.map(|at| theme::short_stamp(at, now));
            let response = widgets::dm_entry(
                ui,
                &widgets::DmEntry {
                    name: &dm.label,
                    face: (Some(dm.face.0.as_str()), dm.face.1.as_deref()),
                    status: dm.status,
                    preview: dm.preview.as_deref(),
                    stamp: stamp.as_deref(),
                    selected,
                    unread: dm.unread,
                    avatar: dm.avatar.as_deref(),
                },
            );
            dm_menu(&response, dm, nav);
            if response.clicked() {
                *nav = Some(Nav::Channel(SelectedChannel {
                    origin: dm.origin,
                    channel: dm.id.clone(),
                    kind: ChannelKind::Dm,
                }));
            }
        }
        if shown == 0 {
            empty_hint(ui, &tab, filter);
        }
    }

    /// Нижняя панель колонки: «я», статус соединения и кнопка настроек.
    pub(super) fn show_user_bar(
        ui: &mut egui::Ui,
        bridge: &Bridge,
        voice: &crate::voice::VoiceState,
        nav: &mut Option<Nav>,
    ) {
        let (muted, deafened) = voice.flags();
        let my_face = bridge
            .cache_at(0)
            .map_or_else(|| (String::new(), None), |cache| face_of(cache.me()));
        let (name, handle, status, my_avatar) = bridge.cache_at(0).map_or_else(
            || (String::new(), String::new(), UserStatus::Online, None),
            |cache| {
                let me = cache.me();
                (
                    author_name(me.display_name.as_deref(), &me.username).to_owned(),
                    format!("@{}", me.username),
                    cache.user_status(&me.id).unwrap_or(me.status),
                    me.avatar.clone().map(|v| {
                        bridge
                            .base_at(0)
                            .map_or_else(|| v.clone(), |base| crate::images::absolute(base, &v))
                    }),
                )
            },
        );
        let connected = *bridge.connection() == bs_instances::ConnectionState::Connected;
        egui::Panel::bottom("user_bar")
            .exact_size(66.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().inner_margin(Margin::symmetric(10, 8)))
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(theme::inset())
                    .stroke(if theme::glass() {
                        theme::glass_stroke()
                    } else {
                        egui::Stroke::new(1.0, theme::border())
                    })
                    .corner_radius(crate::theme::rounded(12))
                    .inner_margin(Margin::symmetric(10, 0))
                    .show(ui, |ui| {
                        ui.set_height(48.0);
                        ui.set_width(ui.available_width());
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            let left = ui.cursor().min;
                            theme::avatar(
                                ui,
                                face(&name, &my_face),
                                32.0,
                                Some(status),
                                my_avatar.as_deref(),
                            );
                            // Справа три кнопки: имя не должно на них залезать.
                            let text_width = ui.available_width() - 106.0;
                            ui.vertical(|ui| {
                                ui.set_max_width(text_width.max(1.0));
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.add_space(8.0);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&name)
                                            .size(14.0)
                                            .family(theme::bold_family())
                                            .color(theme::text_bright()),
                                    )
                                    .truncate(),
                                );
                                ui.add(
                                    egui::Label::new(
                                        theme::muted(if connected {
                                            handle
                                        } else {
                                            connection_label(bridge.connection())
                                        })
                                        .size(11.0),
                                    )
                                    .truncate(),
                                );
                            });
                            super::me_menu::button(ui, left, bridge, status, nav);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                user_bar_buttons(ui, muted, deafened, nav);
                            });
                        });
                    });
            });
    }

    /// Правая панель: участники пространства по секциям.
    /// Строки правой панели: участники пространства или групповой беседы;
    /// `None` — панели здесь нет (личная беседа один на один).
    pub(super) fn member_panel_rows(&self, bridge: &Bridge) -> Option<Vec<MemberRow>> {
        if let Some((origin, space_id)) = &self.active_space {
            let mut rows = bridge
                .cache()
                .map(|cache| member_rows(cache, space_id))
                .unwrap_or_default();
            // Свой аватар участника в этом пространстве.
            if let Some(session) = bridge.session_at(*origin) {
                for row in &mut rows {
                    if let Some(avatar) = session
                        .space_profile(space_id, &row.user_id)
                        .and_then(|p| p.avatar.clone())
                    {
                        row.avatar = Some(avatar);
                    }
                }
            }
            return Some(rows);
        }
        let selected = self
            .selected_channel
            .as_ref()
            .filter(|s| s.kind == ChannelKind::Dm)?;
        if self.is_saved_open(bridge) {
            return None;
        }
        let cache = bridge.cache_at(selected.origin)?;
        let dm = cache
            .dm_channel(&selected.channel)
            .filter(|dm| dm.owner_id.is_some())?;
        Some(group_member_rows(cache, dm))
    }

    pub(super) fn show_member_panel(
        ui: &mut egui::Ui,
        rows: &[MemberRow],
    ) -> Option<(String, String)> {
        let mut clicked = None;
        let width = crate::columns::width(ui.ctx(), crate::columns::Side::Right);
        let panel = egui::Panel::right("members")
            .show_separator_line(false)
            .exact_size(width)
            .resizable(false)
            .frame(theme::module(theme::bg_sidebar()).inner_margin(Margin::symmetric(10, 16)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("members_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut current: Option<MemberSection> = None;
                        for row in rows {
                            let section = row.section();
                            if current.as_ref() != Some(&section) {
                                if current.is_some() {
                                    ui.add_space(12.0);
                                }
                                let count = rows.iter().filter(|r| r.section() == section).count();
                                let title = section.title();
                                ui.horizontal(|ui| {
                                    ui.add_space(8.0);
                                    ui.label(theme::section_title(&format!("{title} — {count}")));
                                });
                                ui.add_space(4.0);
                                current = Some(section);
                            }
                            if theme::member_row(
                                ui,
                                face(&row.name, &row.face),
                                row.status,
                                row.name_color(),
                                row.avatar.as_deref(),
                                row.activity
                                    .as_ref()
                                    .map(|(icon, text, color)| (*icon, text.as_str(), *color)),
                                row.bot,
                            )
                            .clicked()
                            {
                                clicked = Some((row.user_id.clone(), row.name.clone()));
                            }
                        }
                    });
            });
        crate::columns::handle(ui, panel.response.rect, crate::columns::Side::Right);
        clicked
    }

    /// Кнопки справа в шапке, справа налево: участники, настройки группы,
    /// звонок, поиск. Один ряд — кнопки не наезжают друг на друга.
    pub(super) fn header_buttons(
        &mut self,
        ui: &mut egui::Ui,
        bridge: &Bridge,
        has_members: bool,
        title: &str,
    ) {
        let Some(selected) = self.selected_channel.clone() else {
            return;
        };
        if has_members {
            let color = if self.show_members {
                theme::text_bright()
            } else {
                theme::text_faint()
            };
            if ui
                .add(widgets::flat_icon(Icon::Users, 22.0, color))
                .on_hover_text(if self.show_members {
                    tr("Скрыть участников")
                } else {
                    tr("Показать участников")
                })
                .clicked()
            {
                self.show_members = !self.show_members;
            }
        }
        if selected.kind == ChannelKind::Dm
            && !self.is_saved_open(bridge)
            && let Some(dm) = bridge
                .cache_at(selected.origin)
                .and_then(|c| c.dm_channel(&selected.channel))
        {
            if dm.owner_id.is_some()
                && ui
                    .add(widgets::flat_icon(Icon::Gear, 22.0, theme::text_faint()))
                    .on_hover_text(tr("Настройки группы"))
                    .clicked()
            {
                self.group_dialog = Some(crate::screens::group_dm::GroupDialog::manage(
                    selected.origin,
                    dm,
                ));
            }
            if crate::bridge::e2ee::is_direct(dm) {
                let status = bridge.e2ee_status(selected.origin, &selected.channel);
                let (label, color) = if status.key_changed {
                    (tr("Ключ изменился"), theme::amber())
                } else if status.enabled {
                    (tr("Зашифровано"), theme::green())
                } else {
                    (tr("Шифрование"), theme::text_faint())
                };
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new(label).size(12.0).color(color))
                            .frame(false),
                    )
                    .on_hover_text(tr("Сквозное шифрование этой переписки"))
                    .clicked()
                {
                    self.e2ee_dialog = Some(crate::screens::e2ee_dialog::E2eeDialog::new(
                        selected.origin,
                        selected.channel.clone(),
                    ));
                }
            }
            if self.voice.dm.is_none()
                && ui
                    .add(widgets::flat_icon(Icon::Phone, 22.0, theme::text_faint()))
                    .on_hover_text(tr("Позвонить"))
                    .clicked()
            {
                self.pending_call = Some((
                    selected.channel.clone(),
                    dm.federated_id.clone(),
                    title.to_owned(),
                ));
            }
        }
        self.notification_button(ui, bridge, &selected);
        self.gallery_button(ui, bridge, &selected);
        let color = if self.search.open {
            theme::text_bright()
        } else {
            theme::text_faint()
        };
        if ui
            .add(widgets::flat_icon(Icon::Search, 22.0, color))
            .on_hover_text(tr("Поиск по сообщениям"))
            .clicked()
        {
            self.search.toggle();
        }
        super::downloads::button(ui);
    }

    /// Кнопка галереи вложений.
    fn gallery_button(&mut self, ui: &mut egui::Ui, bridge: &Bridge, selected: &SelectedChannel) {
        let color = if self.gallery.is_some() {
            theme::text_bright()
        } else {
            theme::text_faint()
        };
        if ui
            .add(widgets::flat_icon(Icon::Image, 22.0, color))
            .on_hover_text(tr("Вложения: картинки, видео, аудио, GIF, ссылки, файлы"))
            .clicked()
        {
            self.gallery = if self.gallery.is_some() {
                None
            } else {
                Some(super::gallery::Gallery::open(
                    bridge,
                    selected.origin,
                    &selected.channel,
                    selected.kind,
                ))
            };
        }
    }

    /// Колокольчик: уведомления канала (как в меню канала оригинала).
    fn notification_button(
        &mut self,
        ui: &mut egui::Ui,
        bridge: &Bridge,
        selected: &SelectedChannel,
    ) {
        if selected.kind != ChannelKind::Space {
            return;
        }
        let Some(space_id) = bridge
            .cache_at(selected.origin)
            .and_then(|c| c.channel(&selected.channel))
            .map(|c| c.space_id.clone())
        else {
            return;
        };
        if ui
            .add(widgets::flat_icon(Icon::Bell, 22.0, theme::text_faint()))
            .on_hover_text(tr("Настройки уведомлений"))
            .clicked()
        {
            self.notification_settings =
                crate::screens::notification_settings::NotificationSettings::open(
                    bridge,
                    selected.origin,
                    &space_id,
                    Some(&selected.channel),
                );
        }
    }

    /// Канал NSFW, в который ещё не соглашались войти: вместо ленты —
    /// предупреждение. `true` — показано предупреждение.
    pub(super) fn show_nsfw_gate(
        &self,
        ui: &mut egui::Ui,
        bridge: &Bridge,
        nav: &mut Option<Nav>,
    ) -> bool {
        let Some(selected) = self.selected_channel.as_ref() else {
            return false;
        };
        let nsfw = selected.kind == ChannelKind::Space
            && bridge
                .cache_at(selected.origin)
                .and_then(|c| c.channel(&selected.channel))
                .is_some_and(|c| c.nsfw);
        if !nsfw || crate::prefs::get().nsfw_allowed.contains(&selected.channel) {
            return false;
        }
        let channel = selected.channel.clone();
        ui.add_space((ui.available_height() * 0.25).max(24.0));
        ui.vertical_centered(|ui| {
            ui.set_max_width(440.0);
            ui.add(crate::widgets::flat_icon(
                crate::icons::Icon::Eye,
                40.0,
                theme::rose(),
            ));
            ui.add_space(6.0);
            ui.label(
                RichText::new(tr("NSFW"))
                    .size(20.0)
                    .family(theme::bold_family())
                    .color(theme::text_bright()),
            );
            ui.add_space(6.0);
            ui.label(theme::muted(tr(
                "В этом канале может быть откровенный или шокирующий контент. Продолжайте, \
                 только если готовы его видеть.",
            )));
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                let width = 150.0_f32.mul_add(2.0, ui.spacing().item_spacing.x);
                ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                if ui
                    .add_sized([150.0, 34.0], egui::Button::new(tr("Назад")))
                    .clicked()
                {
                    *nav = Some(Nav::Friends);
                }
                if ui
                    .add_sized([150.0, 34.0], theme::primary_button(tr("Показать")))
                    .clicked()
                {
                    crate::prefs::update(|prefs| {
                        prefs.nsfw_allowed.insert(channel.clone());
                    });
                }
            });
        });
        true
    }

    /// Шапка ленты: название канала, тема, переключатель участников.
    /// Полоса над личной перепиской, пока она не зашифрована: сообщения
    /// хранятся на серверах открытым текстом. Закрывается крестиком —
    /// для этой переписки навсегда.
    pub(super) fn show_plaintext_notice(&mut self, ui: &mut egui::Ui, bridge: &Bridge) {
        let Some(selected) = self.selected_channel.as_ref() else {
            return;
        };
        if selected.kind != ChannelKind::Dm
            || crate::prefs::get()
                .plaintext_dismissed
                .contains(&selected.channel)
        {
            return;
        }
        let Some(dm) = bridge
            .cache_at(selected.origin)
            .and_then(|c| c.dm_channel(&selected.channel))
        else {
            return;
        };
        if !crate::bridge::e2ee::is_direct(dm) {
            return;
        }
        // Переписка с чатом личного моста: шифровать некому, а сервер и так
        // пересылает её в Telegram или Matrix.
        if dm.members.iter().any(|m| is_bridge_puppet(&m.username)) {
            return;
        }
        let me = bridge.cache_at(selected.origin).map(|c| c.me().id.clone());
        let remote = dm
            .members
            .iter()
            .any(|m| Some(&m.id) != me.as_ref() && m.home_instance.is_some());
        bridge.e2ee_probe(selected.origin, &selected.channel);
        let status = bridge.e2ee_status(selected.origin, &selected.channel);
        let Some(text) = plaintext_notice(&status, remote) else {
            return;
        };
        let (origin, channel) = (selected.origin, selected.channel.clone());
        let amber = theme::amber();
        egui::Panel::top("plaintext_notice")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(amber.gamma_multiply(0.14))
                    .inner_margin(Margin::symmetric(20, 6)),
            )
            .show(ui, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let (mark, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
                    crate::icons::paint(ui.painter(), mark.center(), 15.0, Icon::Lock, amber);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(widgets::flat_icon(Icon::Close, 14.0, theme::text_faint()))
                            .on_hover_text(tr("Больше не показывать в этой переписке"))
                            .clicked()
                        {
                            crate::prefs::update(|prefs| {
                                prefs.plaintext_dismissed.insert(channel.clone());
                            });
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(tr("Подробнее")).size(12.5).color(amber),
                                )
                                .frame(false),
                            )
                            .clicked()
                        {
                            self.e2ee_dialog = Some(crate::screens::e2ee_dialog::E2eeDialog::new(
                                origin,
                                channel.clone(),
                            ));
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(text).size(12.5).color(theme::text_bright()),
                                )
                                .wrap(),
                            );
                        });
                    });
                });
            });
    }

    pub(super) fn show_header(&mut self, ui: &mut egui::Ui, bridge: &Bridge) {
        let (prefix, title, topic) = self.selected_channel.as_ref().map_or_else(
            || ("", String::new(), None),
            |selected| {
                let Some(cache) = bridge.cache() else {
                    return ("", String::new(), None);
                };
                match selected.kind {
                    ChannelKind::Space => cache.channel(&selected.channel).map_or_else(
                        || ("", String::new(), None),
                        |c| ("#", c.name.clone(), c.topic.clone()),
                    ),
                    ChannelKind::Dm => cache.dm_channel(&selected.channel).map_or_else(
                        || ("", String::new(), None),
                        |dm| ("@", dm_display_name(dm, &cache.me().id), None),
                    ),
                }
            },
        );
        let has_members = self.member_panel_rows(bridge).is_some();
        egui::Panel::top("chat_header")
            .exact_size(52.0)
            .resizable(false)
            .frame(egui::Frame::new().inner_margin(Margin::symmetric(20, 0)))
            .show(ui, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.label(RichText::new(prefix).size(20.0).color(theme::text_faint()));
                    // Справа кнопки шапки (поиск, участники, звонок…): текст до них.
                    let buttons = 150.0;
                    let title_width = (ui.available_width() - buttons).max(60.0);
                    theme::label_fit(
                        ui,
                        RichText::new(&title)
                            .family(theme::bold_family())
                            .size(16.5)
                            .color(theme::text_bright()),
                        title_width,
                    );
                    if let Some(topic) = topic.filter(|t| !t.trim().is_empty()) {
                        ui.add_space(6.0);
                        let rest = ui.available_width() - buttons;
                        if rest > 40.0 {
                            theme::label_fit(
                                ui,
                                theme::muted(topic.replace('\n', " ")).size(13.0),
                                rest,
                            );
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        self.header_buttons(ui, bridge, has_members, &title);
                    });
                });
            });
    }
}

/// Кнопки панели «я»: микрофон, звук, настройки (справа налево).
fn user_bar_buttons(ui: &mut egui::Ui, muted: bool, deafened: bool, nav: &mut Option<Nav>) {
    if widgets::icon_button(ui, Icon::Gear, 32.0)
        .on_hover_text(tr("Настройки"))
        .clicked()
    {
        *nav = Some(Nav::Settings);
    }
    let phones = if deafened {
        Icon::HeadphonesOff
    } else {
        Icon::Headphones
    };
    if widgets::icon_button(ui, phones, 30.0)
        .on_hover_text(if deafened {
            tr("Включить звук")
        } else {
            tr("Выключить звук")
        })
        .clicked()
    {
        *nav = Some(Nav::Voice(crate::voice::DockAction::ToggleDeafen));
    }
    let mic = if muted { Icon::MicOff } else { Icon::Mic };
    if widgets::icon_button(ui, mic, 30.0)
        .on_hover_text(if muted {
            tr("Включить микрофон")
        } else {
            tr("Выключить микрофон")
        })
        .clicked()
    {
        *nav = Some(Nav::Voice(crate::voice::DockAction::ToggleMute));
    }
}

/// Название моста, если это его служебный пользователь.
pub(crate) fn bridge_network(username: &str) -> Option<&'static str> {
    if !is_bridge_puppet(username) {
        return None;
    }
    Some(match username.get(..2) {
        Some("tg") => "Telegram",
        Some("mx") => "Matrix",
        _ => "Discord",
    })
}

/// Подсказка вместо пустого списка бесед.
fn empty_hint(ui: &mut egui::Ui, tab: &super::folders::Tab, filter: &str) {
    ui.add_space(8.0);
    ui.vertical_centered(|ui| {
        ui.label(theme::muted(
            if matches!(tab, super::folders::Tab::Folder(_)) && filter.is_empty() {
                tr("Папка пуста: ПКМ по беседе → «В папку»")
            } else if filter.is_empty() {
                tr("Бесед пока нет")
            } else {
                tr("Нет совпадений — Enter, чтобы искать людей")
            },
        ));
    });
}

/// Служебный пользователь моста («Анна · Telegram»): имя вида `tg_p…`,
/// `mx_p…`, `dc_p…` и цифры.
pub(crate) fn is_bridge_puppet(username: &str) -> bool {
    ["tg_p", "mx_p", "dc_p"].iter().any(|prefix| {
        username
            .strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// Текст предупреждения о незашифрованной переписке; `None` — шифрование
/// включено и предупреждать не о чем.
fn plaintext_notice(
    status: &crate::bridge::e2ee::ChatStatus,
    federated: bool,
) -> Option<&'static str> {
    if status.enabled {
        return None;
    }
    Some(if status.has_peer_key {
        tr(
            "Переписка не зашифрована: её может прочитать администратор сервера. \
            Шифрование можно включить.",
        )
    } else if federated {
        tr(
            "Переписка не зашифрована: у собеседника с другого инстанса нет ключа — его сервер \
            или клиент, похоже, не поддерживает шифрование.",
        )
    } else {
        tr("Переписка не зашифрована: собеседник ещё не опубликовал ключ шифрования.")
    })
}

#[cfg(test)]
mod plaintext_tests {
    use super::plaintext_notice;
    use crate::bridge::e2ee::ChatStatus;

    #[test]
    fn bridge_puppets_are_recognized() {
        assert!(super::is_bridge_puppet("tg_p384264417637748736"));
        assert!(super::is_bridge_puppet("mx_p1"));
        assert!(!super::is_bridge_puppet("tg_p"));
        assert!(!super::is_bridge_puppet("tg_pavel"));
        assert!(!super::is_bridge_puppet("quddy"));
    }

    #[test]
    fn notice_only_while_unencrypted() {
        let on = ChatStatus {
            enabled: true,
            ..ChatStatus::default()
        };
        assert!(plaintext_notice(&on, true).is_none());
        let none = ChatStatus::default();
        assert!(plaintext_notice(&none, true).is_some_and(|t| t.contains("другого инстанса")));
        assert!(plaintext_notice(&none, false).is_some_and(|t| !t.contains("другого инстанса")));
        let keyed = ChatStatus {
            has_peer_key: true,
            ..ChatStatus::default()
        };
        assert!(plaintext_notice(&keyed, false).is_some_and(|t| t.contains("можно включить")));
    }
}
