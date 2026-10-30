//! Экран социальных функций GUI: друзья, заявки, поиск, личные каналы.
//!
//! Вся раскладка данных — в [`crate::social`]; здесь только отрисовка
//! и постановка запросов в [`Bridge`].

use crate::bridge::{Bridge, BridgeRequest};
use crate::social::{
    SocialTab, dm_display_name, dm_last_text, friend_display_name, friend_status_text, is_friend,
    request_is_incoming, request_name, search_user_name,
};
use crate::theme;

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
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, bridge: &Bridge) -> bool {
        self.ensure_loaded(bridge);
        let mut close = false;
        self.show_tabs(ui, bridge, &mut close);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_CHAT)
                    .inner_margin(egui::Margin::symmetric(28, 20)),
            )
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(self.tab.title())
                        .size(20.0)
                        .strong()
                        .color(theme::TEXT_BRIGHT),
                );
                ui.add_space(10.0);
                if let Some(err) = bridge.social_error() {
                    ui.label(egui::RichText::new(err).color(theme::ROSE));
                    ui.add_space(6.0);
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.tab {
                        SocialTab::Friends => Self::show_friends(ui, bridge),
                        SocialTab::Requests => Self::show_requests(ui, bridge),
                        SocialTab::Search => self.show_search(ui, bridge),
                        SocialTab::Dms => self.show_dms(ui, bridge),
                    });
            });
        close
    }

    /// Левая колонка: возврат в чат и вкладки.
    fn show_tabs(&mut self, ui: &mut egui::Ui, bridge: &Bridge, close: &mut bool) {
        egui::Panel::left("social_tabs")
            .exact_size(220.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(8, 14)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    if ui.button("← В чат").clicked() {
                        *close = true;
                    }
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    ui.label(theme::section_title("Общение"));
                });
                for tab in [
                    SocialTab::Friends,
                    SocialTab::Requests,
                    SocialTab::Search,
                    SocialTab::Dms,
                ] {
                    if theme::list_row(ui, tab.title(), self.tab == tab, false).clicked()
                        && self.tab != tab
                    {
                        self.tab = tab;
                        self.reload(bridge);
                    }
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    if ui.small_button("Обновить").clicked() {
                        self.reload(bridge);
                    }
                });
            });
    }

    fn show_friends(ui: &mut egui::Ui, bridge: &Bridge) {
        let friends = bridge.friends();
        if friends.is_empty() {
            theme::empty_state(ui, "Друзей пока нет — найдите их во вкладке «Поиск»");
        }
        for friend in friends {
            let status = friend_status_text(friend);
            theme::person_card(ui, &friend_display_name(friend), Some(&status), |ui| {
                if ui.button("Удалить").clicked() {
                    bridge.request(BridgeRequest::RemoveFriend(friend.id.clone()));
                }
                if ui.add(theme::primary_button("Написать")).clicked() {
                    bridge.request(BridgeRequest::OpenDm(friend.id.clone()));
                }
            });
        }
    }

    fn show_requests(ui: &mut egui::Ui, bridge: &Bridge) {
        let me = bridge.cache().map(|cache| cache.me().id.clone());
        let requests = bridge.friend_requests();
        if requests.is_empty() {
            theme::empty_state(ui, "Заявок нет");
        }
        for request in requests {
            let incoming = me
                .as_deref()
                .is_some_and(|me| request_is_incoming(request, me));
            let subtitle = if incoming {
                "Входящая заявка"
            } else {
                "Исходящая заявка"
            };
            theme::person_card(ui, &request_name(request), Some(subtitle), |ui| {
                if incoming {
                    if ui.button("Отклонить").clicked() {
                        bridge.request(BridgeRequest::RespondFriendRequest(
                            request.id.clone(),
                            false,
                        ));
                    }
                    if ui.add(theme::positive_button("Принять")).clicked() {
                        bridge.request(BridgeRequest::RespondFriendRequest(
                            request.id.clone(),
                            true,
                        ));
                    }
                } else if ui.button("Отозвать").clicked() {
                    bridge.request(BridgeRequest::CancelFriendRequest(request.id.clone()));
                }
            });
        }
    }

    fn show_search(&mut self, ui: &mut egui::Ui, bridge: &Bridge) {
        egui::Frame::new()
            .fill(theme::BG_INPUT)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 96.0).max(80.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text("Имя пользователя")
                            .frame(egui::Frame::NONE)
                            .align(egui::Align2::LEFT_CENTER)
                            .desired_width(width),
                    );
                    let submitted =
                        response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.add(theme::primary_button("Найти")).clicked() || submitted {
                        self.search(bridge);
                        response.request_focus();
                    }
                });
            });
        ui.add_space(10.0);
        let friends = bridge.friends();
        let results = bridge.search_results();
        if results.is_empty() {
            theme::empty_state(ui, "Ничего не найдено");
        }
        for user in results {
            let already = is_friend(friends, &user.id);
            let subtitle = if already { "В друзьях" } else { "" };
            theme::person_card(ui, &search_user_name(user), Some(subtitle), |ui| {
                if ui.button("Написать").clicked() {
                    bridge.request(BridgeRequest::OpenDm(user.id.clone()));
                }
                if !already && ui.add(theme::positive_button("В друзья")).clicked() {
                    bridge.request(BridgeRequest::SendFriendRequest(user.id.clone()));
                }
            });
        }
    }

    fn show_dms(&mut self, ui: &mut egui::Ui, bridge: &Bridge) {
        let me = bridge.cache().map(|cache| cache.me().id.clone());
        let channels = bridge.dm_channels();
        if channels.is_empty() {
            theme::empty_state(ui, "Личных каналов нет");
        }
        for channel in channels {
            let name = me
                .as_deref()
                .map_or_else(|| channel.id.clone(), |me| dm_display_name(channel, me));
            let last = dm_last_text(channel);
            theme::person_card(ui, &name, last.as_deref(), |ui| {
                if ui.add(theme::primary_button("Открыть")).clicked() {
                    self.open_channel = Some(channel.id.clone());
                }
            });
        }
    }
}
