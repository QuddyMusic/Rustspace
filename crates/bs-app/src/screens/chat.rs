//! Основной экран: сайдбар пространств/каналов, лента сообщений, ввод.

use bs_core::MultiCache;
use bs_types::{ChannelId, Origin};

use crate::bridge::Bridge;
use crate::logic::{
    DEFAULT_REACTION, author_name, can_send, message_actions, outgoing_text, should_ack,
};

/// Выбранный канал вместе с инстансом, которому он принадлежит.
///
/// Без origin выбор был бы неоднозначным: у двух инстансов легко найдутся
/// каналы с одинаковыми именами и даже id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedChannel {
    /// Инстанс-владелец канала.
    pub(crate) origin: Origin,
    /// Идентификатор канала внутри инстанса.
    pub(crate) channel: ChannelId,
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

/// Пространство в объединённом сайдбаре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarSpace {
    /// Инстанс-владелец.
    pub(crate) origin: Origin,
    /// Имя пространства.
    pub(crate) name: String,
    /// Каналы пространства.
    pub(crate) channels: Vec<SidebarChannel>,
}

/// Собирает объединённый сайдбар по всем инстансам.
///
/// Порядок стабилен: домашний инстанс первым, затем удалённые по строковому
/// представлению — так список не «прыгает» между кадрами.
#[must_use]
pub(crate) fn sidebar_items(multi: &MultiCache) -> Vec<SidebarSpace> {
    let mut origins = multi.origins();
    origins.sort_by(|a, b| {
        b.is_home()
            .cmp(&a.is_home())
            .then_with(|| a.as_web_str().cmp(&b.as_web_str()))
    });

    let mut items = Vec::new();
    for origin in origins {
        let Some(cache) = multi.cache(&origin) else {
            continue;
        };
        let mut spaces: Vec<_> = cache.spaces().collect();
        spaces.sort_by(|a, b| a.space.id.cmp(&b.space.id));
        for space in spaces {
            let mut channels: Vec<SidebarChannel> = cache
                .channels(&space.space.id)
                .map(|channel| SidebarChannel {
                    id: channel.id.clone(),
                    label: format!("#{}", channel.name),
                    unread: cache.unread_count(&channel.id) > 0,
                })
                .collect();
            channels.sort_by(|a, b| a.id.cmp(&b.id));
            items.push(SidebarSpace {
                origin: origin.clone(),
                name: space.space.name.clone(),
                channels,
            });
        }
    }
    items
}

/// Короткая метка инстанса для показа рядом с чужими элементами.
#[must_use]
pub(crate) fn origin_label(origin: &Origin) -> String {
    match origin {
        Origin::Home => "дом".to_owned(),
        Origin::Remote(url) => url.host_str().unwrap_or(url.as_str()).to_owned(),
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

    /// Открывает канал, переключая инстанс при необходимости.
    pub(crate) fn open_channel(&mut self, selected: SelectedChannel, bridge: &mut Bridge) {
        bridge.set_active(selected.origin.clone());
        self.selected_channel = Some(selected);
    }

    /// Рисует экран. Возвращает `true`, если пользователь попросил соцэкран.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, bridge: &mut Bridge) -> bool {
        let mut open_social = false;
        let items = sidebar_items(bridge.multi());
        let active = bridge.active().clone();
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
                for space in &items {
                    let mut title = egui::RichText::new(&space.name).strong();
                    if space.origin != active {
                        title = title.weak();
                    }
                    ui.label(title);
                    for channel in &space.channels {
                        let selected = self.selected_channel.as_ref().is_some_and(|current| {
                            current.origin == space.origin && current.channel.as_str() == channel.id
                        });
                        let mut text = egui::RichText::new(&channel.label);
                        if channel.unread {
                            text = text.strong();
                        }
                        let mut label = String::new();
                        if channel.unread {
                            label.push_str("• ");
                        }
                        label.push_str(&channel.label);
                        let text = if space.origin == active {
                            text
                        } else {
                            egui::RichText::new(format!("{label} @{}", origin_label(&space.origin)))
                        };
                        let response = ui.selectable_label(selected, text);
                        if response.clicked() {
                            requested = Some(SelectedChannel {
                                origin: space.origin.clone(),
                                channel: ChannelId::from(channel.id.clone()),
                            });
                        }
                    }
                    ui.add_space(4.0);
                }
            });

        if let Some(selected) = requested {
            self.open_channel(selected, bridge);
        }

        self.show_composer(ui, bridge);
        self.show_messages(ui, bridge);
        open_social
    }
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
        if let Some(edit) = self.edit.take() {
            bridge.edit_message(edit.message_id, content);
        } else if let Some(selected) = self.selected_channel.clone() {
            bridge.send_message(selected.channel, content);
        } else {
            return;
        }
        self.draft.clear();
    }

    fn show_messages(&mut self, ui: &mut egui::Ui, bridge: &Bridge) {
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(selected) = self.selected_channel.clone() else {
                ui.centered_and_justified(|ui| {
                    ui.label("Выберите канал");
                });
                return;
            };
            let channel_id = selected.channel.clone();
            let Some(cache) = bridge.multi().cache(&selected.origin) else {
                return;
            };
            let me = cache.me().id.clone();
            let ack_key = format!("{}|{channel_id}", selected.origin.as_web_str());

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    let mut edit_request = None;
                    for msg in cache.messages(&channel_id) {
                        let name =
                            author_name(msg.user.display_name.as_deref(), &msg.user.username);
                        let actions = message_actions(&msg.user.id, &me);
                        let body = msg
                            .message
                            .content
                            .as_deref()
                            .unwrap_or_default()
                            .to_owned();
                        let message_id = msg.message.id.clone();

                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(name).strong());
                            if msg.message.edited_at.is_some() {
                                ui.label(egui::RichText::new("(изменено)").weak());
                            }
                        });
                        let response = ui.label(body.clone());
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
                                edit_request = Some((message_id.clone(), body.clone()));
                                ui.close();
                            }
                            if actions.manage && ui.button("Удалить").clicked() {
                                bridge.delete_message(message_id.clone());
                                ui.close();
                            }
                        });
                        ui.add_space(4.0);
                    }
                    if let Some((id, content)) = edit_request {
                        self.begin_edit(id, &content);
                    }
                });

            // Прочитано: только когда последнее сообщение изменилось.
            if let Some(last) = cache.messages(&channel_id).last() {
                let message_id = last.message.id.clone();
                let ack = self.last_ack.as_ref();
                if should_ack(
                    ack.map(|(c, m)| (c.as_str(), m.as_str())),
                    &ack_key,
                    &message_id,
                ) {
                    bridge.ack_channel(channel_id.clone(), message_id.clone());
                    self.last_ack = Some((ack_key, message_id));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use bs_core::MultiCache;
    use bs_types::{Origin, ReadyPayload};

    use super::{origin_label, sidebar_items};

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
    fn empty_multi_gives_empty_sidebar() {
        assert!(sidebar_items(&MultiCache::new()).is_empty());
    }

    #[test]
    fn single_instance_lists_space_and_channel() {
        let multi =
            MultiCache::from_ready([(Origin::Home, ready("s1", "Работа", "c1", "general"))]);
        let items = sidebar_items(&multi);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Работа");
        assert_eq!(items[0].channels.len(), 1);
        assert_eq!(items[0].channels[0].label, "#general");
        assert!(!items[0].channels[0].unread);
    }

    #[test]
    fn two_instances_keep_home_first() {
        let remote = Origin::parse_web_str("https://chat.example.com").expect("origin");
        let multi = MultiCache::from_ready([
            (remote.clone(), ready("s2", "Чужое", "c2", "random")),
            (Origin::Home, ready("s1", "Работа", "c1", "general")),
        ]);
        let items = sidebar_items(&multi);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].origin, Origin::Home, "домашний инстанс первым");
        assert_eq!(items[1].origin, remote);
    }

    #[test]
    fn same_space_names_are_distinguished_by_origin() {
        let remote = Origin::parse_web_str("https://chat.example.com").expect("origin");
        let multi = MultiCache::from_ready([
            (Origin::Home, ready("s1", "Работа", "c1", "general")),
            (remote, ready("s1", "Работа", "c1", "general")),
        ]);
        let items = sidebar_items(&multi);
        assert_eq!(items.len(), 2, "одинаковые имена не схлопываются");
        assert_ne!(items[0].origin, items[1].origin);
    }

    #[test]
    fn origin_label_prefers_host() {
        let remote = Origin::parse_web_str("https://chat.example.com").expect("origin");
        assert_eq!(origin_label(&remote), "chat.example.com");
        assert_eq!(origin_label(&Origin::Home), "дом");
    }
}
