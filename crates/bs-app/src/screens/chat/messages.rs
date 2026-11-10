//! Лента сообщений, поле ввода, поиск, реакции.

use super::{
    Bridge, ChannelKind, ChatScreen, FLASH_SECONDS, HistoryState, Margin, MessageRow, ReplyTarget,
    RichText, SelectedChannel, Sense, SystemLine, dm_display_name, face, media, message_actions,
    message_rows, name_map, needs_header, now_ms, outgoing_text, should_ack, should_load_older,
    theme, tr, typing_text,
};

/// Строка состояния истории над лентой. Возвращает `true`, если нажали «Повторить».
pub(super) fn show_history_header(ui: &mut egui::Ui, history: &HistoryState) -> bool {
    if history.loading {
        ui.vertical_centered(|ui| {
            ui.label(theme::muted(tr("Загрузка истории…")));
        });
    } else if let Some(error) = &history.error {
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(crate::tf!(
                    "Не удалось загрузить историю: {error}",
                    error = error
                ))
                .color(theme::rose()),
            );
        });
        return ui.button(tr("Повторить")).clicked();
    } else if history.exhausted {
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            ui.label(
                RichText::new(tr("Это начало беседы"))
                    .size(18.0)
                    .family(theme::bold_family())
                    .color(theme::text_bright()),
            );
            ui.label(theme::muted(tr(
                "Здесь пока всё, что было написано раньше.",
            )));
            ui.add_space(16.0);
        });
    }
    false
}

impl ChatScreen {
    pub(super) fn show_composer(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge) {
        let hint = match (&self.selected_channel, bridge.cache()) {
            (Some(selected), Some(cache)) => match selected.kind {
                ChannelKind::Space => cache.channel(&selected.channel).map_or_else(
                    || tr("Сообщение").to_owned(),
                    |c| crate::tf!("Написать в #{a0}", a0 = c.name),
                ),
                ChannelKind::Dm => cache.dm_channel(&selected.channel).map_or_else(
                    || tr("Сообщение").to_owned(),
                    |dm| crate::tf!("Написать {a0}", a0 = dm_display_name(dm, &cache.me().id)),
                ),
            },
            _ => tr("Сообщение").to_owned(),
        };
        let typing = self.selected_channel.as_ref().and_then(|selected| {
            let cache = bridge.cache()?;
            let me = &cache.me().id;
            let names: Vec<String> = cache
                .typing_in(&selected.channel, now_ms())
                .filter(|t| &t.user_id != me)
                .map(|t| t.username.clone())
                .collect();
            typing_text(&names)
        });

        egui::Panel::bottom("composer")
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::bg_chat())
                    .inner_margin(Margin {
                        left: 20,
                        right: 20,
                        top: 0,
                        bottom: 18,
                    }),
            )
            .show(ui, |ui| {
                // Строка над полем: «печатает…» или режим правки; высота постоянна.
                ui.horizontal(|ui| {
                    ui.set_min_height(20.0);
                    if let Some(reply) = self.reply.clone().filter(|_| !self.is_editing()) {
                        ui.label(
                            theme::muted(crate::tf!(
                                "Ответ для {a0}: {a1}",
                                a0 = reply.author,
                                a1 = theme::truncate_chars(&reply.preview.replace('\n', " "), 60)
                            ))
                            .size(12.5),
                        );
                        if ui
                            .small_button("✕")
                            .on_hover_text(tr("Отменить ответ"))
                            .clicked()
                        {
                            self.reply = None;
                        }
                    } else if self.is_editing() {
                        ui.label(
                            RichText::new(tr("Правка сообщения"))
                                .color(theme::amber())
                                .size(12.5),
                        );
                        if ui.small_button(tr("Отмена (Esc)")).clicked() {
                            self.cancel_edit();
                        }
                    } else if let Some(text) = &typing {
                        ui.label(theme::muted(text).size(12.5));
                    }
                });
                let enabled = self.selected_channel.is_some();
                let editing = self.is_editing();
                let outgoing =
                    self.composer
                        .show(ui, bridge, &mut self.draft, &hint, enabled, editing);
                if editing && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.cancel_edit();
                }
                if let Some(outgoing) = outgoing {
                    self.submit(bridge, outgoing);
                    ui.memory_mut(|m| m.request_focus(crate::screens::composer::edit_id()));
                }
            });
    }

    /// Отправляет черновик: новое сообщение или правку.
    pub(super) fn submit(&mut self, bridge: &Bridge, outgoing: crate::screens::composer::Outgoing) {
        let Some(selected) = self.selected_channel.clone() else {
            return;
        };
        let content = outgoing.content;
        if !outgoing.keep_draft
            && let Some(edit) = self.edit.take()
        {
            if let Some(text) = outgoing_text(&content) {
                bridge.edit_message(selected.kind, edit.message_id, text);
            }
            self.draft.clear();
            return;
        }
        let text = outgoing_text(&content);
        let reply_to_id = self.reply.as_ref().map(|r| r.message_id.clone());
        match selected.kind {
            ChannelKind::Space if outgoing.attachments.is_empty() => {
                if let Some(text) = text {
                    bridge.send_message(selected.kind, selected.channel, text, reply_to_id);
                }
            }
            ChannelKind::Space => bridge.send_rest(
                selected.channel,
                bs_api::SendMessageBody {
                    content: text.unwrap_or_default(),
                    attachments: outgoing.attachments,
                    reply_to_id,
                },
            ),
            ChannelKind::Dm => {
                if text.is_some() || !outgoing.attachments.is_empty() {
                    bridge.send_dm_with(selected.channel, text, outgoing.attachments, reply_to_id);
                }
            }
        }
        if !outgoing.keep_draft {
            self.draft.clear();
            self.reply = None;
        }
    }

    /// Сбрасывает состояние прокрутки, если открыли другой канал.
    pub(super) fn sync_scroll_key(&mut self, selected: &SelectedChannel) {
        let key = format!("{}:{}", selected.origin, selected.channel);
        if self.scroll_key.as_ref() != Some(&key) {
            self.scroll_key = Some(key);
            self.scroll_anchor = None;
            self.was_scrolled_down = false;
        }
    }

    /// Прочитано: `ack` уходит только когда последнее сообщение изменилось.
    pub(super) fn ack_latest(
        &mut self,
        bridge: &Bridge,
        ack_key: &str,
        channel_id: &str,
        last_id: Option<&str>,
    ) {
        let Some(last_id) = last_id else {
            return;
        };
        let last = self.last_ack.as_ref();
        if should_ack(
            last.map(|(c, m)| (c.as_str(), m.as_str())),
            ack_key,
            last_id,
        ) {
            bridge.ack_channel(channel_id.to_owned(), last_id.to_owned());
            self.last_ack = Some((ack_key.to_owned(), last_id.to_owned()));
        }
    }

    pub(super) fn show_messages(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::bg_chat())
                    .inner_margin(Margin::symmetric(0, 0)),
            )
            .show(ui, |ui| {
                let Some(selected) = self.selected_channel.clone() else {
                    ui.centered_and_justified(|ui| {
                        ui.label(theme::muted(tr(
                            "Выберите канал или беседу слева — или найдите друзей",
                        )));
                    });
                    return;
                };
                self.sync_scroll_key(&selected);
                let channel_id = selected.channel.clone();
                if let Some(id) = bridge.take_jump(&channel_id) {
                    self.jump_to = Some(id);
                }
                bridge.ensure_history(selected.kind, &channel_id);
                let history = bridge
                    .history_state(&channel_id)
                    .cloned()
                    .unwrap_or_default();

                let (me, rows) = {
                    let Some(cache) = bridge.cache() else {
                        return;
                    };
                    self.names = name_map(cache);
                    (
                        cache.me().id.clone(),
                        message_rows(cache, selected.kind, &channel_id, &self.names),
                    )
                };
                let ack_key = format!("{}:{channel_id}", selected.origin);
                let first_id = rows.first().map(|row| row.id.clone());
                let mut retry = false;
                let mut edit_request = None;

                let output = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        retry = show_history_header(ui, &history);
                        let mut prev: Option<&MessageRow> = None;
                        for row in &rows {
                            let show_header = needs_header(prev, row);
                            if show_header
                                && prev.is_none_or(|p| {
                                    theme::local_day(p.created_at)
                                        != theme::local_day(row.created_at)
                                })
                            {
                                theme::day_divider(ui, &theme::day_label(row.created_at));
                            }
                            let manage = message_actions(&row.author_id, &me).manage;
                            let view = MessageView {
                                selected: &selected,
                                show_header,
                                manage,
                                first_id: first_id.as_deref(),
                            };
                            self.show_message(ui, bridge, row, &view, &mut edit_request);
                            prev = Some(row);
                        }
                        ui.add_space(8.0);
                    });
                if let Some((id, content)) = edit_request {
                    self.begin_edit(id, &content);
                }

                // Подгрузка старых сообщений при прокрутке к верху ленты.
                let offset = output.state.clamped_offset().y;
                if offset >= crate::logic::LOAD_OLDER_THRESHOLD_PX {
                    self.was_scrolled_down = true;
                }
                let wants_older = should_load_older(
                    offset,
                    output.content_size.y,
                    output.inner_rect.height(),
                    self.was_scrolled_down,
                );
                if retry || (wants_older && history.can_auto_load()) {
                    if self.scroll_anchor.is_none() {
                        self.scroll_anchor.clone_from(&first_id);
                    }
                    bridge.load_older(selected.kind, &channel_id);
                }

                self.ack_latest(
                    bridge,
                    &ack_key,
                    &channel_id,
                    rows.last().map(|row| row.id.as_str()),
                );
                self.show_react_picker(ui, bridge);
            });
    }

    /// Принимает выбор места для скачиваемых файлов и запускает скачивание.
    pub(super) fn poll_save(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.save_pending else {
            return;
        };
        match pending.try_recv() {
            Ok(picked) => {
                self.save_pending = None;
                if let Some(hub) = crate::images::ImageHub::of(ctx) {
                    for (target, path) in picked {
                        hub.request_download(&target.value, path, &target.name);
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.save_pending = None,
        }
    }

    /// Прокручивает к сообщению из результатов поиска и ненадолго подсвечивает его.
    pub(super) fn jump_and_flash(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        id: &str,
    ) {
        let now = ui.input(|i| i.time);
        if self.jump_to.as_deref() == Some(id) {
            response.scroll_to_me(Some(egui::Align::Center));
            self.flash = Some((id.to_owned(), now + FLASH_SECONDS));
            self.jump_to = None;
        }
        if let Some((flash_id, until)) = &self.flash
            && flash_id == id
        {
            let left = (*until - now) / FLASH_SECONDS;
            if left <= 0.0 {
                self.flash = None;
            } else {
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "доля 0..1 переводится в альфа-канал 0..255"
                )]
                let alpha = (left.clamp(0.0, 1.0) * 46.0) as u8;
                ui.painter().rect_filled(
                    rect,
                    4.0,
                    egui::Color32::from_rgba_unmultiplied(252, 211, 77, alpha),
                );
                ui.ctx().request_repaint();
            }
        }
    }

    /// Панель поиска и плашка «вы смотрите старые сообщения».
    pub(super) fn show_search(&mut self, ctx: &egui::Context, bridge: &mut Bridge) {
        self.poll_save(ctx);
        let Some(selected) = self.selected_channel.clone() else {
            return;
        };
        self.search.sync(bridge);
        if let Some(id) = self
            .search
            .show(ctx, bridge, selected.kind, &selected.channel)
        {
            bridge.jump_to(selected.kind, &selected.channel, &id);
        }
        if bridge.is_detached(&selected.channel) {
            // Середина ленты: слева колонка каналов, справа панель участников.
            let right = if self.show_members && self.active_space.is_some() {
                236.0
            } else {
                0.0
            };
            let shift = (322.0 - right) / 2.0;
            egui::Area::new(egui::Id::unique("detached_bar"))
                .anchor(egui::Align2::CENTER_BOTTOM, [shift, -96.0])
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(theme::bg_bubble())
                        .stroke(egui::Stroke::new(1.0, theme::border()))
                        .corner_radius(egui::CornerRadius::same(18))
                        .inner_margin(Margin::symmetric(14, 7))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(theme::muted(tr("Вы смотрите старые сообщения")));
                                if ui.button(tr("К последним")).clicked() {
                                    self.jump_to = None;
                                    bridge.return_to_present(&selected.channel);
                                }
                            });
                        });
                });
        }
    }

    /// Окно выбора эмодзи для реакции.
    pub(super) fn show_react_picker(&mut self, ui: &egui::Ui, bridge: &Bridge) {
        let Some((message_id, mut tab)) = self.react_picker.clone() else {
            return;
        };
        let mut open = true;
        let mut chosen = None;
        egui::Window::new(tr("Реакция"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                chosen = crate::screens::composer::emoji_picker(ui, &mut tab);
            });
        if let Some(glyph) = chosen {
            bridge.add_reaction(message_id, glyph.to_owned());
            self.react_picker = None;
        } else if open {
            self.react_picker = Some((message_id, tab));
        } else {
            self.react_picker = None;
        }
    }

    /// Системное сообщение беседы: строка события или карточка приглашения.
    pub(super) fn show_system_message(
        &mut self,
        ui: &mut egui::Ui,
        bridge: &Bridge,
        selected: &SelectedChannel,
        row: &MessageRow,
        event: Option<&bs_types::DmSystemEvent>,
    ) {
        ui.add_space(6.0);
        if let Some(bs_types::DmSystemEvent::SpaceInvite(invite)) = event {
            ui.horizontal(|ui| {
                ui.add_space(70.0);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(crate::tf!(
                            "{a0} приглашает в пространство",
                            a0 = row.author
                        ))
                        .size(12.5)
                        .color(theme::text_muted()),
                    );
                    if let Some(join) =
                        crate::screens::invite_card::show(ui, bridge, selected.origin, invite)
                    {
                        self.pending_join = Some(join);
                    }
                });
            });
            ui.add_space(4.0);
            return;
        }
        let text = event.map_or_else(
            || tr("Системное сообщение").to_owned(),
            |event| crate::social::system_text(event, &row.author, true),
        );
        let glyph = event
            .and_then(bs_types::DmSystemEvent::glyph)
            .unwrap_or("•");
        ui.horizontal(|ui| {
            ui.add_space(34.0);
            ui.label(RichText::new(glyph).size(14.0).color(theme::text_faint()));
            ui.add_space(6.0);
            ui.label(RichText::new(text).size(13.5).color(theme::text_muted()));
            ui.label(
                RichText::new(theme::stamp(row.created_at, now_ms()))
                    .size(11.5)
                    .color(theme::text_faint()),
            );
        });
        ui.add_space(2.0);
    }

    /// Одно сообщение: аватар и имя у первого в группе, тело, контекстное меню.
    pub(super) fn show_message(
        &mut self,
        ui: &mut egui::Ui,
        bridge: &Bridge,
        row: &MessageRow,
        view: &MessageView<'_>,
        edit_request: &mut Option<(String, String)>,
    ) {
        if let Some(line) = &row.system {
            let event = match line {
                SystemLine::Event(event) => Some(event.as_ref()),
                SystemLine::Unknown => None,
            };
            self.show_system_message(ui, bridge, view.selected, row, event);
            return;
        }
        let gap = match (view.show_header, theme::compact()) {
            (true, false) => 16.0,
            (true, true) => 6.0,
            (false, false) => 2.0,
            (false, true) => 0.0,
        };
        ui.add_space(gap);
        let (rect, hits) = self.message_body(ui, row, view.show_header);
        let response = ui.interact(rect, ui.auto_id_with(("msg", &row.id)), Sense::click());
        // После подгрузки возвращаем в поле зрения сообщение, которое читали:
        // новые страницы появляются выше него.
        if self.scroll_anchor.as_deref() == Some(row.id.as_str())
            && view.first_id != Some(row.id.as_str())
        {
            response.scroll_to_me(Some(egui::Align::TOP));
            self.scroll_anchor = None;
        }
        self.jump_and_flash(ui, &response, rect, &row.id);
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.message_click(ui, bridge, row, &hits, pos);
        }
        if response.secondary_clicked() {
            self.menu_image = response.interact_pointer_pos().and_then(|pos| {
                hits.images
                    .iter()
                    .find(|(rect, _)| rect.contains(pos))
                    .map(|(_, value)| (row.id.clone(), value.clone()))
            });
        }
        self.message_menu(ui, bridge, row, view, &response, edit_request);
    }

    /// Аватар, имя, ответ, текст, вложения и реакции; возвращает область и
    /// кликабельные места внутри неё.
    fn message_body(
        &self,
        ui: &mut egui::Ui,
        row: &MessageRow,
        show_header: bool,
    ) -> (egui::Rect, MessageHits) {
        let mut hits = MessageHits::default();
        let inner = ui.horizontal_top(|ui| {
            ui.add_space(20.0);
            if show_header {
                let response = theme::avatar(
                    ui,
                    face(&row.author, &row.face),
                    40.0,
                    None,
                    row.avatar.as_deref(),
                );
                hits.profile.push(response.rect);
            } else {
                ui.allocate_space(egui::vec2(40.0, 1.0));
            }
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                if show_header {
                    ui.horizontal(|ui| {
                        let name = ui.add(egui::Label::new(
                            RichText::new(&row.author)
                                .family(theme::bold_family())
                                .size(15.5)
                                .color(theme::text_bright()),
                        ));
                        hits.profile.push(name.rect);
                        ui.label(theme::muted(theme::stamp(row.created_at, now_ms())).size(11.5));
                    });
                }
                if let Some((author, text)) = &row.reply {
                    crate::screens::body::show_reply_quote(ui, author, text);
                }
                if let Some(sticker) = media::sticker_of(&row.body) {
                    hits.images.extend(media::show_sticker(ui, sticker));
                } else if let Some(gif) = media::gif_of(&row.body) {
                    hits.images.extend(media::show_gif(ui, gif));
                } else if !row.body.is_empty() {
                    let names = &self.names;
                    let blocks = crate::markdown::parse(&row.body, &|id| names.get(id).cloned());
                    crate::screens::body::show_text(ui, &row.id, &blocks, row.edited);
                }
                for attachment in &row.attachments {
                    hits.images
                        .extend(media::show_attachment(ui, attachment, &mut hits.downloads));
                }
                media::show_download_all(ui, &row.attachments, &mut hits.downloads);
                if !row.reactions.is_empty() {
                    hits.reactions = crate::screens::body::show_reactions(ui, &row.reactions);
                }
            });
        });
        let rect =
            egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), inner.response.rect.y_range());
        (rect, hits)
    }

    /// Щелчок по сообщению: скачать, открыть картинку, реакция или профиль.
    fn message_click(
        &mut self,
        ui: &egui::Ui,
        bridge: &Bridge,
        row: &MessageRow,
        hits: &MessageHits,
        pos: egui::Pos2,
    ) {
        if let Some((_, click)) = hits.downloads.iter().find(|(rect, _)| rect.contains(pos)) {
            if self.save_pending.is_none() {
                self.save_pending = Some(crate::download::start(ui.ctx(), click.clone()));
            }
            return;
        }
        if let Some((_, value)) = hits.images.iter().find(|(rect, _)| rect.contains(pos)) {
            self.viewer = Some(value.clone());
        }
        match hits
            .reactions
            .iter()
            .find(|(rect, _)| rect.contains(pos))
            .map(|(_, click)| click.clone())
        {
            Some(crate::screens::body::ReactionClick::Toggle(emoji, mine)) => {
                if mine {
                    bridge.remove_reaction(row.id.clone(), emoji);
                } else {
                    bridge.add_reaction(row.id.clone(), emoji);
                }
            }
            Some(crate::screens::body::ReactionClick::OpenPicker) => {
                self.react_picker = Some((row.id.clone(), 0));
            }
            None => {}
        }
        if hits.profile.iter().any(|rect| rect.contains(pos)) {
            self.profile = Some(crate::screens::profile::ProfileState::open(
                bridge,
                &row.author_id,
                &row.author,
            ));
        }
    }

    /// Контекстное меню сообщения и то, что из него выбрали.
    fn message_menu(
        &mut self,
        ui: &egui::Ui,
        bridge: &Bridge,
        row: &MessageRow,
        view: &MessageView<'_>,
        response: &egui::Response,
        edit_request: &mut Option<(String, String)>,
    ) {
        let image = self
            .menu_image
            .clone()
            .filter(|(id, _)| *id == row.id)
            .map(|(_, value)| value);
        // Избранное можно вести только по GIF-ссылкам: в сообщение уходит адрес.
        let gif_url = image
            .as_deref()
            .filter(|v| media::gif_of(&row.body) == Some(*v))
            .map(str::to_owned);
        let gif_favorite = gif_url
            .as_deref()
            .is_some_and(|url| self.composer.is_favorite_gif(bridge, url));
        let mut choice = MenuChoice::default();
        response.context_menu(|ui| {
            if let Some(value) = &image {
                image_menu(
                    ui,
                    row,
                    value,
                    gif_url.is_some().then_some(gif_favorite),
                    &mut choice,
                );
                ui.separator();
            }
            if ui.button(tr("Ответить")).clicked() {
                choice.reply = true;
                ui.close();
            }
            if ui.button(tr("Добавить реакцию…")).clicked() {
                choice.react = true;
                ui.close();
            }
            if !row.body.is_empty()
                && media::sticker_of(&row.body).is_none()
                && media::gif_of(&row.body).is_none()
                && ui.button(tr("Копировать текст")).clicked()
            {
                ui.ctx().copy_text(row.body.clone());
                ui.close();
            }
            if ui.button(tr("Отметить непрочитанным")).clicked() {
                bridge.send(bs_types::ClientEvent::MarkUnread {
                    channel_id: view.selected.channel.clone(),
                    message_id: row.id.clone(),
                });
                ui.close();
            }
            if view.manage && ui.button(tr("Изменить")).clicked() {
                *edit_request = Some((row.id.clone(), row.body.clone()));
                ui.close();
            }
            if view.manage && ui.button(tr("Удалить")).clicked() {
                bridge.delete_message(view.selected.kind, row.id.clone());
                ui.close();
            }
        });
        if choice.reply {
            self.reply = Some(ReplyTarget {
                message_id: row.id.clone(),
                author: row.author.clone(),
                preview: row.body.clone(),
            });
            self.edit = None;
        }
        if choice.react {
            self.react_picker = Some((row.id.clone(), 0));
        }
        if choice.favorite
            && let Some(url) = &gif_url
        {
            let size = crate::images::ImageHub::of(ui.ctx())
                .and_then(|hub| match hub.state(url) {
                    crate::images::ImageState::Ready(image) => Some(image.size_f32()),
                    _ => None,
                })
                .map(|[w, h]| [w.round() as u32, h.round() as u32]);
            let text = self.composer.toggle_favorite_gif(bridge, url, size);
            self.toast = Some((text, ui.input(|i| i.time) + 3.0));
        }
        if let Some(click) = choice.save
            && self.save_pending.is_none()
        {
            self.save_pending = Some(crate::download::start(ui.ctx(), click));
        }
    }
}

/// Как показывать сообщение в ленте.
pub(super) struct MessageView<'a> {
    /// Открытый канал.
    pub(super) selected: &'a SelectedChannel,
    /// Аватар и имя (первое сообщение в группе).
    pub(super) show_header: bool,
    /// Можно править и удалять.
    pub(super) manage: bool,
    /// Самое верхнее сообщение ленты.
    pub(super) first_id: Option<&'a str>,
}

/// Кликабельные места внутри сообщения.
#[derive(Default)]
struct MessageHits {
    images: Vec<media::Hit>,
    downloads: Vec<media::DownloadHit>,
    profile: Vec<egui::Rect>,
    reactions: Vec<(egui::Rect, crate::screens::body::ReactionClick)>,
}

/// Что выбрали в контекстном меню сообщения.
#[derive(Default)]
struct MenuChoice {
    reply: bool,
    react: bool,
    favorite: bool,
    save: Option<crate::download::Click>,
}

/// Пункты меню для картинки: копировать, сохранить, избранное, стикеры.
/// `gif_favorite` — `Some(уже в избранном)`, если это GIF по ссылке.
fn image_menu(
    ui: &mut egui::Ui,
    row: &MessageRow,
    value: &str,
    gif_favorite: Option<bool>,
    choice: &mut MenuChoice,
) {
    if ui.button(tr("Копировать картинку")).clicked() {
        if let Some(hub) = crate::images::ImageHub::of(ui.ctx()) {
            hub.request_raw(value, crate::images::RawPurpose::Copy);
        }
        ui.close();
    }
    if ui.button(tr("Сохранить как…")).clicked() {
        let name = row
            .attachments
            .iter()
            .find(|a| a.filename == value)
            .map_or_else(|| value.to_owned(), |a| a.name.clone());
        choice.save = Some(crate::download::Click::One(crate::download::Target {
            value: value.to_owned(),
            name,
        }));
        ui.close();
    }
    if let Some(favorite) = gif_favorite
        && ui
            .button(if favorite {
                tr("Убрать из избранных GIF")
            } else {
                tr("В избранные GIF")
            })
            .clicked()
    {
        choice.favorite = true;
        ui.close();
    }
    let can_collect = !value.starts_with("https://") || value.starts_with("sticker:");
    if can_collect && ui.button(tr("Сохранить в мои стикеры")).clicked() {
        if let Some(hub) = crate::images::ImageHub::of(ui.ctx()) {
            hub.request_raw(
                value,
                crate::images::RawPurpose::Sticker(tr("Стикер").to_owned()),
            );
        }
        ui.close();
    }
}
