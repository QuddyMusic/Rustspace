//! Основной экран: сайдбар пространств/каналов/личных сообщений, лента, ввод.

use bs_core::Cache;
use bs_types::ChannelId;

use crate::bridge::{Bridge, ChannelKind, HistoryState};
use crate::logic::{
    DEFAULT_REACTION, author_name, can_send, message_actions, outgoing_text, should_ack,
    should_load_older,
};
use crate::social::dm_display_name;

/// Выбранный канал.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedChannel {
    /// Идентификатор канала внутри инстанса.
    pub(crate) channel: ChannelId,
    /// Канал пространства или личный канал: у них разные события и маршруты.
    pub(crate) kind: ChannelKind,
}

/// Канал в объединённом сайдбаре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarChannel {
    /// Идентификатор канала.
    pub(crate) id: String,
    /// Отображаемое имя (с `#`).
    pub(crate) label: String,
    /// Есть непрочитанные.
    pub(crate) unread: bool,
}

/// Пространство в сайдбаре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarSpace {
    /// Имя пространства.
    pub(crate) name: String,
    /// Каналы пространства.
    pub(crate) channels: Vec<SidebarChannel>,
}

/// Личный канал в сайдбаре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarDm {
    /// Идентификатор личного канала.
    pub(crate) id: String,
    /// Имя собеседника или название группы.
    pub(crate) label: String,
    /// Есть непрочитанные.
    pub(crate) unread: bool,
}

/// Собирает сайдбар: пространства со своими каналами в стабильном порядке.
#[must_use]
pub(crate) fn sidebar_items(cache: &Cache) -> Vec<SidebarSpace> {
    let mut spaces: Vec<_> = cache.spaces().collect();
    spaces.sort_by(|a, b| a.space.id.cmp(&b.space.id));
    spaces
        .into_iter()
        .map(|space| {
            let mut channels: Vec<SidebarChannel> = cache
                .channels(&space.space.id)
                .map(|channel| SidebarChannel {
                    id: channel.id.clone(),
                    label: format!("#{}", channel.name),
                    unread: cache.unread_count(&channel.id) > 0,
                })
                .collect();
            channels.sort_by(|a, b| a.id.cmp(&b.id));
            SidebarSpace {
                name: space.space.name.clone(),
                channels,
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
            id: dm.id.clone(),
            label: dm_display_name(dm, &me),
            unread: cache.unread_count(&dm.id) > 0,
        })
        .collect()
}

/// Одна строка ленты: одинаковая для каналов и личных сообщений.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MessageRow {
    id: String,
    author_id: String,
    author: String,
    body: String,
    edited: bool,
}

/// Строки ленты канала `channel_id` из кэша (старые → новые).
fn message_rows(cache: &Cache, kind: ChannelKind, channel_id: &str) -> Vec<MessageRow> {
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
            })
            .collect(),
    }
}

/// Состояние экрана чата.
#[derive(Debug, Default)]
pub(crate) struct ChatScreen {
    /// Выбранный канал и его инстанс.
    pub(crate) selected_channel: Option<SelectedChannel>,
    pub(crate) draft: String,
    /// Правка сообщения: его id и набранный текст.
    edit: Option<EditState>,
    /// Последняя подтверждённая пара «инстанс+канал, сообщение».
    last_ack: Option<(String, String)>,
    /// Сообщение, которое было самым старым в момент запроса истории:
    /// после подгрузки лента прокручивается к нему, чтобы не «прыгать».
    scroll_anchor: Option<String>,
    /// Листали ли ленту текущего канала вниз (см. `should_load_older`).
    was_scrolled_down: bool,
    /// Канал, для которого относятся `scroll_anchor` и `was_scrolled_down`.
    scroll_key: Option<String>,
}

/// Что именно правим.
#[derive(Debug, Clone)]
struct EditState {
    message_id: String,
}

impl ChatScreen {
    /// Открывает сообщение в режиме правки.
    pub(crate) fn begin_edit(&mut self, message_id: String, content: &str) {
        self.draft.clear();
        self.draft.push_str(content);
        self.edit = Some(EditState { message_id });
    }

    /// Отменяет правку и очищает поле.
    pub(crate) fn cancel_edit(&mut self) {
        self.edit = None;
        self.draft.clear();
    }

    /// `true`, если сейчас правится сообщение.
    pub(crate) fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// Открывает канал.
    pub(crate) fn open_channel(&mut self, selected: SelectedChannel) {
        // Правка относилась к прежнему каналу.
        if self.selected_channel.as_ref() != Some(&selected) {
            self.edit = None;
        }
        self.selected_channel = Some(selected);
    }

    /// Рисует экран. Возвращает `true`, если пользователь попросил соцэкран.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge) -> bool {
        let mut open_social = false;
        let (items, dms) = bridge.cache().map_or_else(Default::default, |cache| {
            (sidebar_items(cache), dm_sidebar_items(cache))
        });
        let mut requested: Option<SelectedChannel> = None;

        egui::Panel::left("channels")
            .resizable(true)
            .default_size(240.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Каналы");
                    if ui.button("Друзья").clicked() {
                        open_social = true;
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("sidebar")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for space in &items {
                            ui.label(egui::RichText::new(&space.name).strong());
                            for channel in &space.channels {
                                let selected =
                                    self.selected_channel.as_ref().is_some_and(|current| {
                                        current.kind == ChannelKind::Space
                                            && current.channel.as_str() == channel.id
                                    });
                                let text = sidebar_text(&channel.label, channel.unread);
                                if ui.selectable_label(selected, text).clicked() {
                                    requested = Some(SelectedChannel {
                                        channel: ChannelId::from(channel.id.clone()),
                                        kind: ChannelKind::Space,
                                    });
                                }
                            }
                            ui.add_space(4.0);
                        }

                        if !dms.is_empty() {
                            ui.separator();
                            ui.label(egui::RichText::new("Личные сообщения").strong());
                            for dm in &dms {
                                let selected =
                                    self.selected_channel.as_ref().is_some_and(|current| {
                                        current.kind == ChannelKind::Dm
                                            && current.channel.as_str() == dm.id
                                    });
                                let text = sidebar_text(&dm.label, dm.unread);
                                if ui.selectable_label(selected, text).clicked() {
                                    requested = Some(SelectedChannel {
                                        channel: ChannelId::from(dm.id.clone()),
                                        kind: ChannelKind::Dm,
                                    });
                                }
                            }
                        }
                    });
            });

        if let Some(selected) = requested {
            self.open_channel(selected);
        }

        self.show_composer(ui, bridge);
        self.show_messages(ui, bridge);
        open_social
    }
}

/// Подпись пункта сайдбара: «•» и полужирный шрифт у непрочитанных.
fn sidebar_text(label: &str, unread: bool) -> egui::RichText {
    if unread {
        egui::RichText::new(format!("• {label}")).strong()
    } else {
        egui::RichText::new(label)
    }
}

/// Строка состояния истории над лентой. Возвращает `true`, если нажали «Повторить».
fn show_history_header(ui: &mut egui::Ui, history: &HistoryState) -> bool {
    if history.loading {
        ui.label(egui::RichText::new("Загрузка истории…").weak());
    } else if let Some(error) = &history.error {
        ui.label(egui::RichText::new(format!("Не удалось загрузить историю: {error}")).weak());
        return ui.button("Повторить").clicked();
    } else if history.exhausted {
        ui.label(egui::RichText::new("Начало переписки").weak());
    }
    false
}

impl ChatScreen {
    fn show_composer(&mut self, ui: &mut egui::Ui, bridge: &Bridge) {
        egui::Panel::bottom("composer").show(ui, |ui| {
            ui.horizontal(|ui| {
                let enabled = self.selected_channel.is_some();
                let hint = if self.is_editing() {
                    "Правка сообщения…"
                } else {
                    "Сообщение…"
                };
                let response = ui.add_enabled(
                    enabled,
                    egui::TextEdit::singleline(&mut self.draft).hint_text(hint),
                );
                let submitted =
                    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let clicked = ui
                    .add_enabled(
                        can_send(
                            self.selected_channel.as_ref().map(|s| &s.channel),
                            &self.draft,
                        ),
                        egui::Button::new("Отправить"),
                    )
                    .clicked();
                if self.is_editing() && ui.button("Отмена").clicked() {
                    self.cancel_edit();
                }
                if clicked || submitted {
                    self.submit(bridge);
                    response.request_focus();
                }
            });
        });
    }

    /// Отправляет черновик: новое сообщение или правку.
    fn submit(&mut self, bridge: &Bridge) {
        let Some(content) = outgoing_text(&self.draft) else {
            return;
        };
        let Some(selected) = self.selected_channel.clone() else {
            return;
        };
        if let Some(edit) = self.edit.take() {
            bridge.edit_message(selected.kind, edit.message_id, content);
        } else {
            bridge.send_message(selected.kind, selected.channel, content);
        }
        self.draft.clear();
    }

    /// Сбрасывает состояние прокрутки, если открыли другой канал.
    fn sync_scroll_key(&mut self, selected: &SelectedChannel) {
        let key = selected.channel.clone();
        if self.scroll_key.as_ref() != Some(&key) {
            self.scroll_key = Some(key);
            self.scroll_anchor = None;
            self.was_scrolled_down = false;
        }
    }

    /// Прочитано: `ack` уходит только когда последнее сообщение изменилось.
    fn ack_latest(
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

    fn show_messages(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge) {
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(selected) = self.selected_channel.clone() else {
                ui.centered_and_justified(|ui| {
                    ui.label("Выберите канал");
                });
                return;
            };
            self.sync_scroll_key(&selected);
            let channel_id = selected.channel.clone();
            bridge.ensure_history(selected.kind, &channel_id);
            let history = bridge
                .history_state(&channel_id)
                .cloned()
                .unwrap_or_default();

            let (me, rows) = {
                let Some(cache) = bridge.cache() else {
                    return;
                };
                (
                    cache.me().id.clone(),
                    message_rows(cache, selected.kind, &channel_id),
                )
            };
            let ack_key = channel_id.clone();
            let first_id = rows.first().map(|row| row.id.clone());
            let mut retry = false;
            let mut edit_request = None;

            let output = egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    retry = show_history_header(ui, &history);

                    for row in &rows {
                        let actions = message_actions(&row.author_id, &me);
                        let message_id = row.id.clone();

                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&row.author).strong());
                            if row.edited {
                                ui.label(egui::RichText::new("(изменено)").weak());
                            }
                        });
                        let response = ui.label(row.body.clone());
                        // После подгрузки возвращаем в поле зрения сообщение,
                        // которое читали: новые страницы появляются выше него.
                        if self.scroll_anchor.as_deref() == Some(row.id.as_str())
                            && first_id.as_deref() != Some(row.id.as_str())
                        {
                            response.scroll_to_me(Some(egui::Align::TOP));
                            self.scroll_anchor = None;
                        }
                        response.context_menu(|ui| {
                            if ui.button("Реакция 👍").clicked() {
                                bridge
                                    .add_reaction(message_id.clone(), DEFAULT_REACTION.to_owned());
                                ui.close();
                            }
                            if ui.button("Убрать реакцию").clicked() {
                                bridge.remove_reaction(
                                    message_id.clone(),
                                    DEFAULT_REACTION.to_owned(),
                                );
                                ui.close();
                            }
                            if actions.manage && ui.button("Изменить").clicked() {
                                edit_request = Some((message_id.clone(), row.body.clone()));
                                ui.close();
                            }
                            if actions.manage && ui.button("Удалить").clicked() {
                                bridge.delete_message(selected.kind, message_id.clone());
                                ui.close();
                            }
                        });
                        ui.add_space(4.0);
                    }
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
        });
    }
}

#[cfg(test)]
mod tests {
    use bs_core::Cache;
    use bs_types::ReadyPayload;

    use super::{dm_sidebar_items, sidebar_items};

    fn ready(
        space_id: &str,
        space_name: &str,
        channel_id: &str,
        channel_name: &str,
    ) -> ReadyPayload {
        serde_json::from_value(serde_json::json!({
            "user": {
                "id": "u1", "username": "alice", "displayName": null, "avatar": null,
                "banner": null, "accentColor": null, "avatarColor": "mint", "bio": null,
                "status": "online", "customStatus": null, "isAdmin": false,
                "createdAt": 1_750_000_000_000_i64, "homeInstance": null,
                "homeUserId": null, "replicatedInstances": []
            },
            "spaces": [{
                "id": space_id, "name": space_name, "icon": null,
                "ownerId": "u1", "visibility": "public",
                "directoryListed": false, "createdAt": 1_750_000_000_000_i64,
                "channels": [{
                    "id": channel_id, "name": channel_name, "spaceId": space_id,
                    "type": "text", "position": 0, "createdAt": 1_750_000_000_000_i64
                }],
                "categories": [], "members": [], "roles": []
            }]
        }))
        .unwrap_or_else(|err| panic!("готовый ready должен разбираться: {err}"))
    }

    #[test]
    fn sidebar_lists_space_and_channel() {
        let cache = Cache::from_ready(ready("s1", "Работа", "c1", "general"));
        let items = sidebar_items(&cache);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Работа");
        assert_eq!(items[0].channels.len(), 1);
        assert_eq!(items[0].channels[0].label, "#general");
        assert!(!items[0].channels[0].unread);
    }

    #[test]
    fn dm_sidebar_lists_private_channels_newest_first() {
        let mut payload = ready("s1", "Работа", "c1", "general");
        payload.dm_channels = serde_json::from_value(serde_json::json!([
            {"id": "100", "federatedId": null, "ownerId": null, "ownerHomeUserId": null,
             "ownerHomeInstance": null, "createdAt": 1, "metadataUpdatedAt": 1,
             "members": [], "name": "Старая группа"},
            {"id": "200", "federatedId": null, "ownerId": null, "ownerHomeUserId": null,
             "ownerHomeInstance": null, "createdAt": 1, "metadataUpdatedAt": 1,
             "members": [], "name": "Новая группа"}
        ]))
        .expect("dm_channels");
        let dms = dm_sidebar_items(&Cache::from_ready(payload));
        let labels: Vec<_> = dms.iter().map(|dm| dm.label.as_str()).collect();
        assert_eq!(labels, ["Новая группа", "Старая группа"]);
    }

    #[test]
    fn dm_sidebar_is_empty_without_private_channels() {
        let cache = Cache::from_ready(ready("s1", "A", "c1", "g"));
        assert!(dm_sidebar_items(&cache).is_empty());
    }
}
