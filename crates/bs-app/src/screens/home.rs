//! «Главная»: друзья (в сети / все / заявки / добавить), панель «Сейчас
//! онлайн» и обзор пространств инстанса (Explore).
//!
//! Раскладка данных — в [`crate::social`]; здесь только отрисовка и
//! постановка запросов в [`Bridge`].

use bs_types::{ExploreSpace, SpaceVisibility, UserStatus};
use egui::{Align, CornerRadius, Layout, Margin, RichText, Sense};

use crate::bridge::BridgeRequest;
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::social::{
    friend_display_name, friend_status_text, is_friend, request_is_incoming, request_name,
    search_user_name, status_label,
};
use crate::theme;
use crate::widgets::{self, PersonRow};

/// Вкладка списка друзей.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum FriendsTab {
    /// Друзья в сети.
    #[default]
    Online,
    /// Все друзья.
    All,
    /// Входящие и исходящие заявки.
    Pending,
    /// Поиск людей и отправка заявок.
    Add,
}

/// Состояние главной: вкладка, поля поиска, флаги загрузки.
#[derive(Debug, Default)]
pub(crate) struct HomeState {
    /// Активная вкладка друзей.
    pub(crate) tab: FriendsTab,
    /// Строка поиска людей (вкладка «Добавить»).
    pub(crate) add_query: String,
    /// Строка поиска пространств.
    pub(crate) explore_query: String,
    /// Данные друзей уже запрашивались в этой сессии.
    loaded: bool,
    /// Друг, которого попросили пригласить в пространство.
    pub(crate) invite_person: Option<bs_types::Friend>,
}

impl HomeState {
    /// Один раз за сессию просит друзей, заявки и личные каналы.
    pub(crate) fn ensure_loaded(&mut self, bridge: &Bridge) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        bridge.request(BridgeRequest::Friends);
        bridge.request(BridgeRequest::FriendRequests);
        bridge.request(BridgeRequest::DmChannels);
    }

    /// Запускает поиск людей по текущей строке.
    fn search_people(&self, bridge: &Bridge) {
        let query = self.add_query.trim();
        if !query.is_empty() {
            bridge.request(BridgeRequest::SearchUsers(query.to_owned()));
        }
    }

    /// Открывает вкладку «Добавить» с уже введённым запросом.
    pub(crate) fn start_search(&mut self, query: &str, bridge: &Bridge) {
        self.tab = FriendsTab::Add;
        query.clone_into(&mut self.add_query);
        self.search_people(bridge);
    }
}

/// Шапка центральной области: плоская, высотой 52.
fn header_frame() -> egui::Frame {
    // Подложку рисует модуль, в который вложена шапка.
    egui::Frame::new().inner_margin(Margin::symmetric(18, 0))
}

/// Список друзей: шапка с вкладками и тело вкладки.
pub(crate) fn show_friends(ui: &mut egui::Ui, state: &mut HomeState, bridge: &Bridge) {
    let pending = bridge.friend_requests().len();
    egui::Panel::top("friends_header")
        .exact_size(52.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(header_frame())
        .show(ui, |ui| {
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add(widgets::flat_icon(Icon::User, 20.0, theme::text_faint()));
                ui.label(
                    RichText::new(tr("Друзья"))
                        .size(16.0)
                        .family(theme::bold_family())
                        .color(theme::text_bright()),
                );
                ui.add_space(8.0);
                let (line, _) = ui.allocate_exact_size(egui::vec2(1.0, 22.0), Sense::hover());
                ui.painter()
                    .rect_filled(line, CornerRadius::ZERO, theme::border());
                ui.add_space(8.0);
                for (tab, label) in [
                    (FriendsTab::Online, tr("В сети").to_owned()),
                    (FriendsTab::All, tr("Все").to_owned()),
                    (
                        FriendsTab::Pending,
                        if pending > 0 {
                            crate::tf!("Заявки · {pending}", pending = pending)
                        } else {
                            tr("Заявки").to_owned()
                        },
                    ),
                ] {
                    if widgets::pill_tab(ui, &label, state.tab == tab).clicked() {
                        state.tab = tab;
                        reload(bridge, tab);
                    }
                }
                if widgets::mint_button(ui, tr("Добавить"), state.tab == FriendsTab::Add).clicked()
                {
                    state.tab = FriendsTab::Add;
                }
            });
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::new().inner_margin(Margin::symmetric(24, 6)))
        .show(ui, |ui| {
            if let Some(err) = bridge.social_error() {
                ui.label(RichText::new(err).color(theme::rose()));
                ui.add_space(6.0);
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match state.tab {
                    FriendsTab::Online => friends_list(ui, bridge, true, &mut state.invite_person),
                    FriendsTab::All => friends_list(ui, bridge, false, &mut state.invite_person),
                    FriendsTab::Pending => pending_list(ui, bridge),
                    FriendsTab::Add => add_friend(ui, state, bridge),
                });
        });
}

/// Перезапрашивает данные вкладки.
fn reload(bridge: &Bridge, tab: FriendsTab) {
    match tab {
        FriendsTab::Online | FriendsTab::All => {
            bridge.request(BridgeRequest::Friends);
        }
        FriendsTab::Pending => {
            bridge.request(BridgeRequest::FriendRequests);
        }
        FriendsTab::Add => {}
    }
}

/// Друзья: только те, кто в сети, или все.
fn friends_list(
    ui: &mut egui::Ui,
    bridge: &Bridge,
    online_only: bool,
    invite: &mut Option<bs_types::Friend>,
) {
    let mut friends: Vec<_> = bridge
        .friends()
        .iter()
        .filter(|f| !online_only || f.status != UserStatus::Offline)
        .collect();
    friends.sort_by_key(|f| {
        (
            f.status == UserStatus::Offline,
            friend_display_name(f).to_lowercase(),
        )
    });
    if friends.is_empty() {
        theme::empty_state(
            ui,
            if online_only {
                tr("Сейчас никого нет в сети")
            } else {
                tr("Друзей пока нет — найдите людей во вкладке «Добавить»")
            },
        );
        return;
    }
    ui.add_space(10.0);
    widgets::section_label(
        ui,
        &format!(
            "{} — {}",
            if online_only {
                tr("В сети")
            } else {
                tr("Все друзья")
            },
            friends.len()
        ),
    );
    ui.add_space(8.0);
    for friend in friends {
        let name = friend_display_name(friend);
        let handle = format!("@{}", friend.username);
        let subtitle = friend_status_text(friend);
        widgets::person_row(
            ui,
            &PersonRow {
                name: &name,
                handle: Some(&handle),
                subtitle: Some(&subtitle),
                status: Some(friend.status),
                avatar: friend.avatar.as_deref(),
                face: (
                    Some(friend.home_user_id.as_deref().unwrap_or(&friend.id)),
                    friend.avatar_color.as_deref(),
                ),
            },
            |ui| {
                if widgets::icon_button(ui, Icon::Close, 34.0)
                    .on_hover_text(tr("Удалить из друзей"))
                    .clicked()
                {
                    bridge.request(BridgeRequest::RemoveFriend(friend.id.clone()));
                }
                if widgets::icon_button(ui, Icon::Chat, 34.0)
                    .on_hover_text(tr("Написать"))
                    .clicked()
                {
                    bridge.request(BridgeRequest::OpenDm(friend.id.clone()));
                }
                if widgets::icon_button(ui, Icon::Join, 34.0)
                    .on_hover_text(tr("Пригласить в пространство"))
                    .clicked()
                {
                    *invite = Some(friend.clone());
                }
            },
        );
    }
}

/// Входящие и исходящие заявки.
fn pending_list(ui: &mut egui::Ui, bridge: &Bridge) {
    let me = bridge.cache().map(|cache| cache.me().id.clone());
    let requests = bridge.friend_requests();
    if requests.is_empty() {
        theme::empty_state(ui, tr("Заявок нет"));
        return;
    }
    ui.add_space(10.0);
    widgets::section_label(ui, &crate::tf!("Ожидают — {a0}", a0 = requests.len()));
    ui.add_space(8.0);
    for request in requests {
        let incoming = me
            .as_deref()
            .is_some_and(|me| request_is_incoming(request, me));
        let name = request_name(request);
        widgets::person_row(
            ui,
            &PersonRow {
                name: &name,
                handle: None,
                subtitle: Some(if incoming {
                    tr("Входящая заявка")
                } else {
                    tr("Исходящая заявка")
                }),
                status: None,
                avatar: request.user.as_ref().and_then(|u| u.avatar.as_deref()),
                face: request.user.as_ref().map_or((None, None), |u| {
                    (
                        Some(u.home_user_id.as_deref().unwrap_or(&u.id)),
                        u.avatar_color.as_deref(),
                    )
                }),
            },
            |ui| {
                if incoming {
                    if widgets::icon_button(ui, Icon::Close, 34.0)
                        .on_hover_text(tr("Отклонить"))
                        .clicked()
                    {
                        bridge.request(BridgeRequest::RespondFriendRequest(
                            request.id.clone(),
                            false,
                        ));
                    }
                    if widgets::icon_button(ui, Icon::Check, 34.0)
                        .on_hover_text(tr("Принять"))
                        .clicked()
                    {
                        bridge.request(BridgeRequest::RespondFriendRequest(
                            request.id.clone(),
                            true,
                        ));
                    }
                } else if widgets::icon_button(ui, Icon::Close, 34.0)
                    .on_hover_text(tr("Отозвать"))
                    .clicked()
                {
                    bridge.request(BridgeRequest::CancelFriendRequest(request.id.clone()));
                }
            },
        );
    }
}

/// Поиск людей и отправка заявок.
fn add_friend(ui: &mut egui::Ui, state: &mut HomeState, bridge: &Bridge) {
    ui.add_space(14.0);
    ui.label(
        RichText::new(tr("Добавить в друзья"))
            .size(16.0)
            .family(theme::bold_family())
            .color(theme::text_bright()),
    );
    ui.label(theme::muted(tr("Найдите человека по имени пользователя.")).size(13.0));
    ui.add_space(10.0);
    egui::Frame::new()
        .fill(theme::bg_input())
        .corner_radius(crate::theme::rounded(9))
        .inner_margin(Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 100.0).max(80.0);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut state.add_query)
                        .hint_text(RichText::new(tr("Имя пользователя")).color(theme::text_faint()))
                        .frame(egui::Frame::NONE)
                        .desired_width(width),
                );
                let submitted =
                    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.add(theme::primary_button(tr("Найти"))).clicked() || submitted {
                    state.search_people(bridge);
                    response.request_focus();
                }
            });
        });
    ui.add_space(8.0);
    let friends = bridge.friends();
    let results = bridge.search_results();
    if results.is_empty() {
        theme::empty_state(ui, tr("Ничего не найдено"));
        return;
    }
    for user in results {
        let already = is_friend(friends, &user.id);
        let name = search_user_name(user);
        let handle = format!("@{}", user.username);
        widgets::person_row(
            ui,
            &PersonRow {
                name: &name,
                handle: Some(&handle),
                subtitle: already.then_some(tr("Уже в друзьях")),
                status: Some(user.status),
                avatar: user.avatar.as_deref(),
                face: (
                    Some(user.home_user_id.as_deref().unwrap_or(&user.id)),
                    user.avatar_color.as_deref(),
                ),
            },
            |ui| {
                if !already && ui.add(theme::positive_button(tr("В друзья"))).clicked() {
                    bridge.request(BridgeRequest::SendFriendRequest(user.id.clone()));
                }
                if widgets::icon_button(ui, Icon::Chat, 34.0)
                    .on_hover_text(tr("Написать"))
                    .clicked()
                {
                    bridge.request(BridgeRequest::OpenDm(user.id.clone()));
                }
            },
        );
    }
}

/// Правая панель «Сейчас онлайн»: друзья по статусам; клик открывает беседу.
pub(crate) fn show_active_now(ui: &mut egui::Ui, bridge: &Bridge) {
    let width = crate::columns::width(ui.ctx(), crate::columns::Side::Right);
    let panel = egui::Panel::right("active_now")
        .show_separator_line(false)
        .exact_size(width)
        .resizable(false)
        .frame(theme::module(theme::bg_chat()).inner_margin(Margin::symmetric(18, 14)))
        .show(ui, |ui| {
            ui.label(
                RichText::new(tr("Сейчас онлайн"))
                    .size(18.0)
                    .family(theme::bold_family())
                    .color(theme::text_bright()),
            );
            ui.add_space(10.0);
            let mut friends: Vec<_> = bridge.friends().iter().collect();
            friends.sort_by_key(|f| friend_display_name(f).to_lowercase());
            let (online, offline): (Vec<_>, Vec<_>) = friends
                .into_iter()
                .partition(|f| f.status != UserStatus::Offline);
            egui::ScrollArea::vertical()
                .id_salt("active_now_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if online.is_empty() && offline.is_empty() {
                        ui.label(theme::muted(tr("Здесь появятся ваши друзья")).size(13.0));
                    }
                    for (title, group) in [(tr("В сети"), &online), (tr("Не в сети"), &offline)]
                    {
                        if group.is_empty() {
                            continue;
                        }
                        ui.add_space(6.0);
                        widgets::section_label(ui, &format!("{title} — {}", group.len()));
                        ui.add_space(4.0);
                        for friend in group {
                            let name = friend_display_name(friend);
                            let subtitle = friend
                                .custom_status
                                .as_deref()
                                .map(str::trim)
                                .filter(|s| !s.is_empty());
                            let response = widgets::compact_person(
                                ui,
                                theme::Face {
                                    name: &name,
                                    key: Some(friend.home_user_id.as_deref().unwrap_or(&friend.id)),
                                    color: friend.avatar_color.as_deref(),
                                },
                                subtitle,
                                friend.status,
                                friend.avatar.as_deref(),
                            )
                            .on_hover_text(status_label(friend.status));
                            if response.clicked() {
                                bridge.request(BridgeRequest::OpenDm(friend.id.clone()));
                            }
                        }
                    }
                });
        });
    crate::columns::handle(ui, panel.response.rect, crate::columns::Side::Right);
}

/// Ширина карточки пространства в обзоре.
const CARD_WIDTH: f32 = 318.0;

/// Обзор пространств. Возвращает (инстанс, id) пространства, которое нужно открыть.
pub(crate) fn show_explore(
    ui: &mut egui::Ui,
    state: &mut HomeState,
    bridge: &mut Bridge,
    create: &mut bool,
) -> Option<(usize, String)> {
    let mut open = None;
    let mut search = false;
    egui::Panel::top("explore_header")
        .exact_size(52.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(header_frame())
        .show(ui, |ui| {
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add(widgets::flat_icon(Icon::Compass, 20.0, theme::text_faint()));
                ui.label(
                    RichText::new(tr("Обзор"))
                        .size(16.0)
                        .family(theme::bold_family())
                        .color(theme::text_bright()),
                );
                ui.add_space(10.0);
                ui.allocate_ui(egui::vec2(300.0, 32.0), |ui| {
                    let response =
                        widgets::search_box(ui, &mut state.explore_query, tr("Поиск пространств…"));
                    search = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(theme::primary_button(tr("Создать пространство")))
                        .clicked()
                    {
                        *create = true;
                    }
                });
            });
        });
    if search {
        bridge.load_explore(&state.explore_query);
    }
    let err = bridge.social_error().map(str::to_owned);
    egui::CentralPanel::default()
        .frame(egui::Frame::new().inner_margin(Margin::symmetric(24, 8)))
        .show(ui, |ui| {
            if let Some(err) = err {
                ui.label(RichText::new(err).color(theme::rose()));
                ui.add_space(6.0);
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    widgets::section_label(ui, tr("Пространства инстанса"));
                    ui.label(theme::muted(tr("Открытые сообщества на этом сервере")).size(13.0));
                    ui.add_space(10.0);
                    let Some(page) = bridge.explore().cloned() else {
                        ui.label(theme::muted(if bridge.explore_loading() {
                            tr("Загрузка…")
                        } else {
                            tr("Каталог не загружен")
                        }));
                        return;
                    };
                    if !page.discovery_enabled {
                        ui.label(theme::muted(tr(
                            "Каталог выключен администратором этого инстанса.",
                        )));
                        return;
                    }
                    if page.spaces.is_empty() {
                        theme::empty_state(ui, tr("Ничего не найдено"));
                    } else {
                        let cards: Vec<(ExploreSpace, Option<String>)> =
                            page.spaces.iter().map(|s| (s.clone(), None)).collect();
                        card_grid(ui, &cards, bridge, &mut open);
                    }
                    outer_space(ui, bridge, &mut open);
                });
        });
    open
}

/// Карточки рядами по ширине окна.
fn card_grid(
    ui: &mut egui::Ui,
    cards: &[(ExploreSpace, Option<String>)],
    bridge: &mut Bridge,
    open: &mut Option<(usize, String)>,
) {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "число колонок"
    )]
    let columns = ((ui.available_width() / (CARD_WIDTH + 16.0)) as usize).max(1);
    for row in cards.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for (space, origin) in row {
                if let Some(target) = space_card(ui, space, origin.as_deref(), bridge) {
                    *open = Some(target);
                }
            }
        });
        ui.add_space(16.0);
    }
}

/// Раздел «Другие инстансы»: общий каталог, прочитанный через свой инстанс.
fn outer_space(ui: &mut egui::Ui, bridge: &mut Bridge, open: &mut Option<(usize, String)>) {
    // Каталог не настроен или выключен — раздела нет, как в оригинале
    // (`directoryAvailable`).
    let Some(Ok(entries)) = bridge.directory().cloned() else {
        return;
    };
    ui.add_space(12.0);
    widgets::section_label(ui, tr("Другие инстансы"));
    ui.label(
        theme::muted(tr(
            "Сообщества с других серверов: вы войдёте туда своим домашним аккаунтом.",
        ))
        .size(13.0),
    );
    ui.add_space(10.0);
    let home = bridge.base_at(0).map(str::to_owned);
    let cards: Vec<(ExploreSpace, Option<String>)> = entries
        .into_iter()
        .filter(|entry| home.as_deref() != Some(entry.origin.trim_end_matches('/')))
        .map(|entry| {
            let space = ExploreSpace {
                id: entry.id,
                name: entry.name,
                icon: entry.icon,
                banner: entry.banner,
                avatar_color: entry.avatar_color,
                description: entry.description,
                visibility: entry.visibility,
                member_count: entry.member_count,
                created_at: entry.created_at,
                joined: None,
            };
            (space, Some(entry.origin))
        })
        .collect();
    if cards.is_empty() {
        theme::empty_state(ui, tr("Ничего не найдено"));
    } else {
        card_grid(ui, &cards, bridge, open);
    }
}

/// Карточка пространства. `origin` — чужой инстанс (общий каталог).
/// Возвращает (инстанс, id), если нажали «Открыть».
fn space_card(
    ui: &mut egui::Ui,
    space: &ExploreSpace,
    origin: Option<&str>,
    bridge: &mut Bridge,
) -> Option<(usize, String)> {
    let slot = origin.map_or(Some(0), |origin| {
        super::invite_card::slot_for_origin(bridge, 0, origin)
    });
    let joined = (origin.is_none() && space.joined == Some(true))
        || slot
            .and_then(|slot| bridge.cache_at(slot))
            .is_some_and(|cache| cache.space(&space.id).is_some());
    let mut action = None;
    egui::Frame::new()
        .fill(theme::bg_sidebar())
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(crate::theme::rounded(10))
        .show(ui, |ui| {
            ui.set_width(CARD_WIDTH);
            ui.vertical(|ui| {
                ui.set_min_height(210.0);
                ui.spacing_mut().item_spacing.y = 4.0;
                card_banner(
                    ui,
                    space,
                    joined,
                    origin
                        .map(str::to_owned)
                        .or_else(|| bridge.base_at(0).map(str::to_owned))
                        .as_deref(),
                );
                ui.add_space(32.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.vertical(|ui| {
                        ui.set_width(CARD_WIDTH - 28.0);
                        card_text(ui, space, origin);
                        let join = JoinTarget {
                            space,
                            origin,
                            slot,
                            joined,
                        };
                        if let Some(open) = join_button(ui, &join, bridge) {
                            action = Some(open);
                        }
                        ui.add_space(12.0);
                    });
                });
            });
        });
    action
}

/// Баннер карточки с меткой и плиткой пространства.
fn card_banner(ui: &mut egui::Ui, space: &ExploreSpace, joined: bool, origin: Option<&str>) {
    let gradient = theme::space_gradient(&space.id, space.avatar_color.as_deref());
    let color = gradient.from.lerp_to_gamma(gradient.to, 0.5);
    let (banner, _) = ui.allocate_exact_size(egui::vec2(CARD_WIDTH, 76.0), Sense::hover());
    let top = crate::theme::rounded(10).nw;
    ui.painter().rect_filled(
        banner,
        CornerRadius {
            nw: top,
            ne: top,
            sw: 0,
            se: 0,
        },
        color.gamma_multiply(0.55),
    );
    let corners = CornerRadius {
        nw: top,
        ne: top,
        sw: 0,
        se: 0,
    };
    if let (Some(origin), Some(banner_file)) =
        (origin, space.banner.as_deref().filter(|b| !b.is_empty()))
    {
        crate::images::paint_from(ui.painter(), banner, origin, banner_file, corners);
    }
    let tag = match (joined, space.visibility) {
        (true, _) => tr("В ПРОСТРАНСТВАХ"),
        (false, SpaceVisibility::Public) => tr("ОТКРЫТОЕ"),
        (false, _) => tr("ПО ЗАЯВКЕ"),
    };
    ui.painter().text(
        banner.right_top() + egui::vec2(-10.0, 12.0),
        egui::Align2::RIGHT_CENTER,
        tag,
        egui::FontId::new(10.0, theme::bold_family()),
        theme::text_bright().gamma_multiply(0.8),
    );
    let tile = egui::Rect::from_min_size(
        banner.left_bottom() + egui::vec2(14.0, -26.0),
        egui::Vec2::splat(56.0),
    );
    ui.painter().rect_filled(
        tile.expand(3.0),
        crate::theme::rounded(17),
        theme::bg_sidebar(),
    );
    let icon = origin
        .zip(space.icon.as_deref().filter(|i| !i.is_empty()))
        .is_some_and(|(origin, icon)| {
            crate::images::paint_from(ui.painter(), tile, origin, icon, crate::theme::rounded(14))
        });
    if !icon {
        theme::fill_gradient(
            ui.painter(),
            tile,
            f32::from(crate::theme::rounded(14).nw),
            gradient,
            1.0,
        );
        ui.painter().text(
            tile.center(),
            egui::Align2::CENTER_CENTER,
            theme::first_letter(&space.name),
            egui::FontId::new(20.0, theme::bold_family()),
            egui::Color32::WHITE,
        );
    }
}

/// Название, описание, число участников и адрес чужого инстанса.
fn card_text(ui: &mut egui::Ui, space: &ExploreSpace, origin: Option<&str>) {
    ui.label(
        RichText::new(theme::truncate_chars(&space.name, 30))
            .size(15.5)
            .family(theme::bold_family())
            .color(theme::text_bright()),
    );
    let description = space
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());
    ui.label(description.map_or_else(
        || {
            RichText::new(tr("Нет описания"))
                .italics()
                .size(12.5)
                .color(theme::text_faint())
        },
        |text| theme::muted(theme::truncate_chars(text, 70)).size(12.5),
    ));
    ui.label(
        theme::muted(members_text(space.member_count))
            .size(12.0)
            .color(theme::text_faint()),
    );
    ui.add_space(6.0);
    if let Some(origin) = origin {
        ui.label(
            theme::muted(theme::truncate_chars(
                origin.trim_start_matches("https://"),
                40,
            ))
            .size(11.5),
        );
    }
}

/// Куда ведёт кнопка карточки.
struct JoinTarget<'a> {
    space: &'a ExploreSpace,
    origin: Option<&'a str>,
    slot: Option<usize>,
    joined: bool,
}

/// «Открыть», «Вступить» или «Подать заявку» (с подключением к чужому
/// инстансу, если нужно). Возвращает пространство, которое надо открыть.
fn join_button(
    ui: &mut egui::Ui,
    target: &JoinTarget<'_>,
    bridge: &mut Bridge,
) -> Option<(usize, String)> {
    let JoinTarget {
        space,
        origin,
        slot,
        joined,
    } = *target;
    let remote = origin.is_some() && slot.is_none();
    let (label, fill, text) = if joined {
        (tr("Открыть"), theme::mint_bg(), theme::mint())
    } else if space.visibility == SpaceVisibility::Public {
        let label = if remote {
            tr("Подключиться и вступить")
        } else {
            tr("Вступить")
        };
        (label, theme::accent(), theme::on_accent())
    } else {
        (
            if remote {
                tr("Подключиться и подать заявку")
            } else {
                tr("Подать заявку")
            },
            egui::Color32::from_rgb(0x3d, 0x33, 0x14),
            theme::amber(),
        )
    };
    let button = egui::Button::new(RichText::new(label).color(text))
        .fill(fill)
        .corner_radius(crate::theme::rounded(7))
        .min_size(egui::vec2(ui.available_width(), 34.0));
    if !ui.add(button).clicked() {
        return None;
    }
    let by_request = space.visibility != SpaceVisibility::Public;
    match (joined, origin, slot) {
        (true, _, Some(slot)) => return Some((slot, space.id.clone())),
        (_, Some(origin), _) => bridge.connect_and_join_space(origin, &space.id, by_request),
        _ => {
            bridge.request(BridgeRequest::JoinSpace {
                id: space.id.clone(),
                by_request,
            });
        }
    }
    None
}

/// «1 участник», «2 участника», «5 участников».
fn members_text(count: u64) -> String {
    let word = match (count % 10, count % 100) {
        (_, 11..=14) => tr("участников"),
        (1, _) => tr("участник"),
        (2..=4, _) => tr("участника"),
        _ => tr("участников"),
    };
    format!("{count} {word}")
}

#[cfg(test)]
mod tests {
    use super::members_text;

    #[test]
    fn members_plural_forms() {
        assert_eq!(members_text(1), "1 участник");
        assert_eq!(members_text(3), "3 участника");
        assert_eq!(members_text(5), "5 участников");
        assert_eq!(members_text(11), "11 участников");
        assert_eq!(members_text(21), "21 участник");
        assert_eq!(members_text(112), "112 участников");
    }
}
