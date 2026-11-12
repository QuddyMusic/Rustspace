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

/// Шапка пространства: имя (ПКМ — меню) и кнопка приглашения.
pub(super) fn space_header(ui: &mut egui::Ui, space: &SidebarSpace, nav: &mut Option<Nav>) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.add(
            egui::Label::new(
                RichText::new(theme::truncate_chars(&space.name, 24))
                    .size(15.5)
                    .family(theme::bold_family())
                    .color(theme::text_bright()),
            )
            .sense(Sense::click()),
        )
        .context_menu(|ui| space_menu(ui, space, nav));
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
            .frame(
                egui::Frame::new()
                    .fill(theme::bg_rail())
                    .inner_margin(Margin::symmetric(0, 10)),
            )
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
                            response.context_menu(|ui| space_menu(ui, space, nav));
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
        egui::Panel::left("channels")
            .resizable(true)
            .default_size(250.0)
            .size_range(200.0..=360.0)
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
                            self.home_list(ui, dms, &filter, hub_dot, nav);
                        }
                    });
            });
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
        if row.clicked() {
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
            if !occupant.me {
                let others: Vec<(String, String)> = space
                    .groups
                    .iter()
                    .flat_map(|g| g.channels.iter())
                    .filter(|c| c.voice && c.id != channel.id)
                    .map(|c| (c.id.clone(), c.name.clone()))
                    .collect();
                row.context_menu(|ui| {
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
                if row.clicked() {
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
                .context_menu(|ui| space_menu(ui, space, nav));
        }
    }

    /// Главная: «Друзья», «Обзор» и личные беседы.
    pub(super) fn home_list(
        &self,
        ui: &mut egui::Ui,
        dms: &[SidebarDm],
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
        if widgets::brand_row(ui, "Backspace", self.view == View::Backspace, hub_dot)
            .on_hover_text(if hub_dot {
                tr("Backspace обновлён")
            } else {
                tr("О проекте Backspace")
            })
            .clicked()
        {
            *nav = Some(Nav::Backspace);
        }
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
        let now = now_ms();
        let mut shown = 0;
        for dm in dms
            .iter()
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
            if response.clicked() {
                *nav = Some(Nav::Channel(SelectedChannel {
                    origin: dm.origin,
                    channel: dm.id.clone(),
                    kind: ChannelKind::Dm,
                }));
            }
        }
        if shown == 0 {
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                ui.label(theme::muted(if filter.is_empty() {
                    tr("Бесед пока нет")
                } else {
                    tr("Нет совпадений — Enter, чтобы искать людей")
                }));
            });
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
                    .fill(theme::bg_rail())
                    .stroke(egui::Stroke::new(1.0, theme::border()))
                    .corner_radius(crate::theme::rounded(12))
                    .inner_margin(Margin::symmetric(10, 0))
                    .show(ui, |ui| {
                        ui.set_height(48.0);
                        ui.set_width(ui.available_width());
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            theme::avatar(
                                ui,
                                face(&name, &my_face),
                                32.0,
                                Some(status),
                                my_avatar.as_deref(),
                            );
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new(theme::truncate_chars(&name, 13))
                                        .size(14.0)
                                        .family(theme::bold_family())
                                        .color(theme::text_bright()),
                                );
                                let sub = if connected {
                                    handle
                                } else {
                                    connection_label(bridge.connection())
                                };
                                ui.label(theme::muted(theme::truncate_chars(&sub, 18)).size(11.0));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
                            });
                        });
                    });
            });
    }

    /// Правая панель: участники пространства по секциям.
    /// Строки правой панели: участники пространства или групповой беседы;
    /// `None` — панели здесь нет (личная беседа один на один).
    pub(super) fn member_panel_rows(&self, bridge: &Bridge) -> Option<Vec<MemberRow>> {
        if let Some((_, space_id)) = &self.active_space {
            return Some(
                bridge
                    .cache()
                    .map(|cache| member_rows(cache, space_id))
                    .unwrap_or_default(),
            );
        }
        let selected = self
            .selected_channel
            .as_ref()
            .filter(|s| s.kind == ChannelKind::Dm)?;
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
        egui::Panel::right("members")
            .show_separator_line(false)
            .exact_size(236.0)
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
                            if current != Some(section) {
                                if current.is_some() {
                                    ui.add_space(12.0);
                                }
                                let count = rows.iter().filter(|r| r.section() == section).count();
                                ui.horizontal(|ui| {
                                    ui.add_space(8.0);
                                    ui.label(theme::section_title(&format!(
                                        "{} — {count}",
                                        section.title()
                                    )));
                                });
                                ui.add_space(4.0);
                                current = Some(section);
                            }
                            if theme::member_row(
                                ui,
                                face(&row.name, &row.face),
                                row.status,
                                row.avatar.as_deref(),
                            )
                            .clicked()
                            {
                                clicked = Some((row.user_id.clone(), row.name.clone()));
                            }
                        }
                    });
            });
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
                    self.e2ee_dialog = Some(crate::screens::e2ee_dialog::E2eeDialog {
                        origin: selected.origin,
                        dm_id: selected.channel.clone(),
                    });
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
    }

    /// Шапка ленты: название канала, тема, переключатель участников.
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
                    ui.label(
                        RichText::new(&title)
                            .family(theme::bold_family())
                            .size(16.5)
                            .color(theme::text_bright()),
                    );
                    if let Some(topic) = topic.filter(|t| !t.trim().is_empty()) {
                        ui.add_space(6.0);
                        ui.label(theme::muted(theme::truncate_chars(&topic, 80)).size(13.0));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        self.header_buttons(ui, bridge, has_members, &title);
                    });
                });
            });
    }
}
