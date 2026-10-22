//! Основной экран: сайдбар пространств/каналов, лента сообщений, ввод.

use bs_types::ChannelId;

use crate::bridge::Bridge;
use crate::logic::{author_name, can_send, outgoing_text};

/// Состояние экрана чата.
#[derive(Debug, Default)]
pub(crate) struct ChatScreen {
    pub(crate) selected_channel: Option<ChannelId>,
    pub(crate) draft: String,
}

impl ChatScreen {
    /// Рисует экран.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, bridge: &Bridge) {
        egui::Panel::left("channels")
            .resizable(true)
            .default_size(240.0)
            .show(ui, |ui| {
                ui.heading("Каналы");
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

        egui::Panel::bottom("composer").show(ui, |ui| {
            ui.horizontal(|ui| {
                let enabled = self.selected_channel.is_some();
                let response = ui.add_enabled(
                    enabled,
                    egui::TextEdit::singleline(&mut self.draft).hint_text("Сообщение…"),
                );
                let submitted =
                    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let clicked = ui
                    .add_enabled(
                        can_send(self.selected_channel.as_ref(), &self.draft),
                        egui::Button::new("Отправить"),
                    )
                    .clicked();
                if clicked || submitted {
                    if let (Some(channel_id), Some(content)) =
                        (self.selected_channel.clone(), outgoing_text(&self.draft))
                    {
                        bridge.send_message(channel_id, content);
                        self.draft.clear();
                        response.request_focus();
                    }
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let Some(channel_id) = self.selected_channel.clone() else {
                ui.centered_and_justified(|ui| {
                    ui.label("Выберите канал");
                });
                return;
            };

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if let Some(cache) = bridge.cache() {
                        for msg in cache.messages(&channel_id) {
                            let name =
                                author_name(msg.user.display_name.as_deref(), &msg.user.username);
                            ui.label(egui::RichText::new(name).strong());
                            ui.label(msg.message.content.as_deref().unwrap_or_default());
                            ui.add_space(4.0);
                        }
                    }
                });
        });
    }
}
