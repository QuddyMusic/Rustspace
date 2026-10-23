//! Основной экран: сайдбар пространств/каналов, лента сообщений, ввод.

use bs_types::ChannelId;

use crate::bridge::Bridge;
use crate::logic::{
    DEFAULT_REACTION, author_name, can_send, message_actions, outgoing_text, should_ack,
};

/// Состояние экрана чата.
#[derive(Debug, Default)]
pub(crate) struct ChatScreen {
    pub(crate) selected_channel: Option<ChannelId>,
    pub(crate) draft: String,
    /// Правка сообщения: его id и набранный текст.
    edit: Option<EditState>,
    /// Последняя подтверждённая пара «канал, сообщение».
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
        // Текст сразу в поле ввода: иначе править пришлось бы с пустого места.
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

    /// Рисует экран. Возвращает `true`, если пользователь попросил соцэкран.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, bridge: &Bridge) -> bool {
        let mut open_social = false;
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
                if let Some(cache) = bridge.cache() {
                    for space in cache.spaces() {
                        ui.label(egui::RichText::new(&space.space.name).strong());
                        for channel in cache.channels(&space.space.id) {
                            let selected = self.selected_channel.as_deref() == Some(&channel.id);
                            if ui.selectable_label(selected, &channel.name).clicked() {
                                self.selected_channel = Some(channel.id.clone());
                            }
                        }
                        ui.add_space(4.0);
                    }
                }
            });

        self.show_composer(ui, bridge);
        self.show_messages(ui, bridge);
        open_social
    }

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
                        can_send(self.selected_channel.as_ref(), &self.draft),
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
        } else if let Some(channel_id) = self.selected_channel.clone() {
            bridge.send_message(channel_id, content);
        } else {
            return;
        }
        self.draft.clear();
    }

    fn show_messages(&mut self, ui: &mut egui::Ui, bridge: &Bridge) {
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(channel_id) = self.selected_channel.clone() else {
                ui.centered_and_justified(|ui| {
                    ui.label("Выберите канал");
                });
                return;
            };
            let Some(cache) = bridge.cache() else {
                return;
            };
            let me = cache.me().id.clone();

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
                    &channel_id,
                    &message_id,
                ) {
                    bridge.ack_channel(channel_id.clone(), message_id.clone());
                    self.last_ack = Some((channel_id.clone(), message_id));
                }
            }
        });
    }
}
