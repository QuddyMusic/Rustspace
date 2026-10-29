//! Экран социальных функций GUI: друзья, заявки, поиск, личные каналы.
//!
//! Вся раскладка данных — в [`crate::social`]; здесь только отрисовка
//! и постановка запросов в [`Bridge`].

use crate::bridge::{Bridge, BridgeRequest};
use crate::social::{
    SocialTab, dm_display_name, dm_last_text, friend_display_name, friend_status_text, is_friend,
    request_is_incoming, request_name, search_user_name,
};

/// Состояние социального экрана.
#[derive(Debug, Default)]
pub(crate) struct SocialScreen {
    pub(crate) tab: SocialTab,
    /// Строка поиска пользователей.
    pub(crate) query: String,
    /// Личный канал, который пользователь попросил открыть в чате.
    open_channel: Option<String>,
    /// Загружены ли данные активной вкладки хотя бы раз.
    loaded: bool,
}

impl SocialScreen {
    /// Сбрасывает состояние при новом логине.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// `true` ровно один раз — если пользователь выбрал канал для открытия.
    pub(crate) fn take_open_channel(&mut self) -> Option<String> {
        self.open_channel.take()
    }

    /// Запрашивает данные активной вкладки, если они ещё не загружались.
    fn ensure_loaded(&mut self, bridge: &Bridge) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        self.reload(bridge);
    }

    /// Перезапрашивает данные активной вкладки.
    fn reload(&self, bridge: &Bridge) {
        match self.tab {
            SocialTab::Friends => {
                bridge.request(BridgeRequest::Friends);
            }
            SocialTab::Requests => {
                bridge.request(BridgeRequest::FriendRequests);
            }
            SocialTab::Dms => {
                bridge.request(BridgeRequest::DmChannels);
            }
            SocialTab::Search => self.search(bridge),
        }
    }

    /// Запускает поиск по текущей строке запроса.
    fn search(&self, bridge: &Bridge) {
        let query = self.query.trim();
        if !query.is_empty() {
            bridge.request(BridgeRequest::SearchUsers(query.to_owned()));
        }
    }

    /// Рисует экран. Возвращает `true`, если пользователь попросил вернуться в чат.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, bridge: &Bridge) -> bool {
        self.ensure_loaded(bridge);
        let mut close = false;
        egui::Panel::left("social_tabs")
            .resizable(true)
            .default_size(180.0)
            .show(ui, |ui| {
                ui.heading("Социальное");
                if ui.button("← В чат").clicked() {
                    close = true;
                }
                ui.separator();
                for tab in [
                    SocialTab::Friends,
                    SocialTab::Requests,
                    SocialTab::Search,
                    SocialTab::Dms,
                ] {
                    if ui.selectable_label(self.tab == tab, tab.title()).clicked()
                        && self.tab != tab
                    {
                        self.tab = tab;
                        self.reload(bridge);
                    }
                }
                ui.separator();
                if ui.button("Обновить").clicked() {
                    self.reload(bridge);
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(err) = bridge.social_error() {
                ui.colored_label(egui::Color32::RED, err);
                ui.separator();
            }
            match self.tab {
                SocialTab::Friends => {
                    let friends = bridge.friends();
                    if friends.is_empty() {
                        ui.label("Друзей пока нет");
                    }
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for friend in friends {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(friend_display_name(friend)).strong(),
                                    );
                                    ui.label(
                                        egui::RichText::new(friend_status_text(friend)).weak(),
                                    );
                                    if ui.button("Написать").clicked() {
                                        bridge.request(BridgeRequest::OpenDm(friend.id.clone()));
                                    }
                                    if ui.button("Удалить").clicked() {
                                        bridge.request(BridgeRequest::RemoveFriend(
                                            friend.id.clone(),
                                        ));
                                    }
                                });
                            }
                        });
                }
                SocialTab::Requests => {
                    let me = bridge.cache().map(|cache| cache.me().id.clone());
                    let requests = bridge.friend_requests();
                    if requests.is_empty() {
                        ui.label("Заявок нет");
                    }
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for request in requests {
                                let incoming = me
                                    .as_deref()
                                    .is_some_and(|me| request_is_incoming(request, me));
                                ui.horizontal(|ui| {
                                    ui.label(request_name(request));
                                    if incoming {
                                        if ui.button("Принять").clicked() {
                                            bridge.request(BridgeRequest::RespondFriendRequest(
                                                request.id.clone(),
                                                true,
                                            ));
                                        }
                                        if ui.button("Отклонить").clicked() {
                                            bridge.request(BridgeRequest::RespondFriendRequest(
                                                request.id.clone(),
                                                false,
                                            ));
                                        }
                                    } else if ui.button("Отозвать").clicked() {
                                        bridge.request(BridgeRequest::CancelFriendRequest(
                                            request.id.clone(),
                                        ));
                                    }
                                });
                            }
                        });
                }
                SocialTab::Search => {
                    ui.horizontal(|ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.query)
                                .hint_text("имя пользователя"),
                        );
                        let submitted =
                            response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("Найти").clicked() || submitted {
                            self.search(bridge);
                            response.request_focus();
                        }
                    });
                    ui.separator();
                    let friends = bridge.friends();
                    let results = bridge.search_results();
                    if results.is_empty() {
                        ui.label("Ничего не найдено");
                    }
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for user in results {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(search_user_name(user)).strong());
                                    if is_friend(friends, &user.id) {
                                        ui.label(egui::RichText::new("в друзьях").weak());
                                    } else if ui.button("В друзья").clicked() {
                                        bridge.request(BridgeRequest::SendFriendRequest(
                                            user.id.clone(),
                                        ));
                                    }
                                    if ui.button("Написать").clicked() {
                                        bridge.request(BridgeRequest::OpenDm(user.id.clone()));
                                    }
                                });
                            }
                        });
                }
                SocialTab::Dms => {
                    let me = bridge.cache().map(|cache| cache.me().id.clone());
                    let channels = bridge.dm_channels();
                    if channels.is_empty() {
                        ui.label("Личных каналов нет");
                    }
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for channel in channels {
                                let name = me.as_deref().map_or_else(
                                    || channel.id.clone(),
                                    |me| dm_display_name(channel, me),
                                );
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(name).strong());
                                    if let Some(last) = dm_last_text(channel) {
                                        ui.label(egui::RichText::new(last).weak());
                                    }
                                    if ui.button("Открыть").clicked() {
                                        self.open_channel = Some(channel.id.clone());
                                    }
                                });
                            }
                        });
                }
            }
        });
        close
    }
}
