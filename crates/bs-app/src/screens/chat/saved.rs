//! «Избранное», закреплённые сообщения и окно закладок.
//!
//! «Избранное» — переписка с самим собой на домашнем инстансе (как в
//! Telegram). Закрепы — сообщения, закреплённые в чате: общие видят все,
//! личные (в переписке один на один) — только вы. Закладки — личные копии
//! сообщений из всех чатов с группировкой по дате, людям, чатам и медиа.

use bs_api::{Bookmark, BookmarkStatus, ChatKind};
use egui::{Margin, RichText, Sense, Vec2};

use super::{ChannelKind, ChatScreen, Nav, SelectedChannel};
use crate::bridge::extras::ExtrasOp;
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::theme;
use crate::widgets;

/// Действие из меню сообщения: выполняется в начале следующего кадра.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ExtrasAction {
    /// Закрепить (`on`) или открепить; для всех или для себя.
    Pin {
        message: String,
        shared: bool,
        on: bool,
    },
    /// Добавить или убрать закладку.
    Bookmark { message: String, on: bool },
    /// Переслать текст в «Избранное».
    ToSaved { text: String },
}

/// Вид чата для закрепов и закладок.
pub(super) const fn chat_kind(kind: ChannelKind) -> ChatKind {
    match kind {
        ChannelKind::Space => ChatKind::Channel,
        ChannelKind::Dm => ChatKind::Dm,
    }
}

/// Как группировать закладки.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Grouping {
    /// По дню сохранения.
    #[default]
    Date,
    /// По автору сообщения.
    Author,
    /// По чату.
    Chat,
}

/// Окно закладок.
#[derive(Debug, Default)]
pub(super) struct BookmarksUi {
    grouping: Grouping,
    /// Только с вложениями.
    media: bool,
    /// Старые сверху.
    oldest_first: bool,
    query: String,
}

/// Что из списка закрепов в меню доступно для сообщения.
#[allow(
    clippy::struct_excessive_bools,
    reason = "ответы «да/нет» для пунктов меню"
)]
pub(super) struct PinMenu {
    /// Чат — переписка один на один (можно закрепить для себя).
    pub(super) personal: bool,
    /// Можно закрепить для всех.
    pub(super) can_share: bool,
    /// Уже закреплено для всех.
    pub(super) shared: bool,
    /// Уже закреплено для себя.
    pub(super) mine: bool,
    /// В закладках.
    pub(super) marked: bool,
}

impl ChatScreen {
    /// «Избранное» на домашнем инстансе, если известно.
    pub(super) fn saved_chat(bridge: &Bridge) -> Option<String> {
        bridge.session_at(0)?.extras().saved.clone()
    }

    /// Открыто ли «Избранное».
    pub(super) fn is_saved_open(&self, bridge: &Bridge) -> bool {
        self.selected_channel.as_ref().is_some_and(|selected| {
            selected.origin == 0
                && selected.kind == ChannelKind::Dm
                && Self::saved_chat(bridge).as_deref() == Some(selected.channel.as_str())
        })
    }

    /// Открыть «Избранное» (или дождаться ответа сервера и открыть).
    pub(super) fn open_saved(&mut self, items: &[super::SidebarSpace], bridge: &mut Bridge) {
        if let Some(saved) = Self::saved_chat(bridge) {
            self.apply_nav(
                Nav::Channel(SelectedChannel {
                    origin: 0,
                    channel: saved,
                    kind: ChannelKind::Dm,
                }),
                items,
                bridge,
            );
        } else if let Some(home) = bridge.session_at_mut(0) {
            if home.extras().saved_missing {
                self.toast = Some((
                    tr("Сервер не поддерживает «Избранное» (нужен rustspace-server)").to_owned(),
                    0.0,
                ));
            }
            home.extras_mut().open_saved = true;
            home.ensure_saved();
        }
    }

    /// Начало кадра: запросы, переходы, уведомления.
    pub(super) fn drive_extras(&mut self, bridge: &mut Bridge, now: f64) -> Option<Nav> {
        let mut nav = None;
        if let Some(home) = bridge.session_at_mut(0) {
            home.ensure_saved();
            let extras = home.extras_mut();
            if extras.open_saved
                && let Some(saved) = extras.saved.clone()
            {
                extras.open_saved = false;
                nav = Some(Nav::Channel(SelectedChannel {
                    origin: 0,
                    channel: saved,
                    kind: ChannelKind::Dm,
                }));
            }
        }
        for origin in bridge.live_origins() {
            let notices = bridge
                .session_at_mut(origin)
                .map(|s| std::mem::take(&mut s.extras_mut().notices))
                .unwrap_or_default();
            if let Some(last) = notices.into_iter().last() {
                self.toast = Some((last.unwrap_or_else(|err| err), now + 3.0));
            }
        }
        if let Some(selected) = self.selected_channel.clone()
            && let Some(session) = bridge.session_at_mut(selected.origin)
        {
            let kind = chat_kind(selected.kind);
            session.ensure_pins(kind, &selected.channel);
            session.ensure_marks(kind, &selected.channel);
            if let Some(action) = self.extras_action.take() {
                self.run_extras(session, &selected, action);
            }
        }
        if let Some((target, message)) = self.pending_jump.clone()
            && self.selected_channel.as_ref() == Some(&target)
        {
            self.pending_jump = None;
            let previous = bridge.focus_index();
            bridge.focus_on(target.origin);
            bridge.jump_to(target.kind, &target.channel, &message);
            bridge.focus_on(previous);
        }
        nav
    }

    fn run_extras(
        &mut self,
        session: &mut crate::bridge::Session,
        selected: &SelectedChannel,
        action: ExtrasAction,
    ) {
        let kind = chat_kind(selected.kind);
        match action {
            ExtrasAction::Pin {
                message,
                shared,
                on,
            } => session.set_pin(ExtrasOp::Pin {
                kind,
                channel: selected.channel.clone(),
                message,
                shared,
                on,
            }),
            ExtrasAction::Bookmark { message, on } => {
                session.set_bookmark(kind, &selected.channel, &message, on);
            }
            ExtrasAction::ToSaved { text } => {
                if let Some(saved) = session.extras().saved.clone() {
                    session.send(bs_types::ClientEvent::DmMessageCreate {
                        dm_channel_id: saved,
                        content: Some(text),
                        attachments: None,
                        reply_to_id: None,
                    });
                    self.toast = Some((tr("Сохранено в «Избранное»").to_owned(), 0.0));
                }
            }
        }
    }

    /// Что показать в меню сообщения про закрепы и закладки.
    pub(super) fn pin_menu(
        bridge: &Bridge,
        selected: &SelectedChannel,
        message: &str,
    ) -> Option<PinMenu> {
        let manage_messages = can_manage_messages(bridge, selected);
        let session = bridge.session_at(selected.origin)?;
        let extras = session.extras();
        let key = (chat_kind(selected.kind), selected.channel.clone());
        let pins = extras.pins.get(&key);
        let pinned = |shared: bool| {
            pins.is_some_and(|list| {
                list.iter()
                    .any(|p| p.message_id == message && p.shared == shared)
            })
        };
        let personal = selected.kind == ChannelKind::Dm
            && bridge
                .cache_at(selected.origin)
                .and_then(|c| c.dm_channel(&selected.channel))
                .is_some_and(|dm| dm.owner_id.is_none() && dm.members.len() <= 2);
        Some(PinMenu {
            personal,
            can_share: selected.kind == ChannelKind::Dm || manage_messages,
            shared: pinned(true),
            mine: pinned(false),
            marked: extras
                .marks
                .get(&key)
                .is_some_and(|set| set.contains(message)),
        })
    }

    /// Панель закрепов над лентой: последний закреп, по щелчку — следующий;
    /// справа — список всех закрепов чата.
    pub(super) fn show_pins_bar(&mut self, ui: &mut egui::Ui, bridge: &Bridge) {
        let Some(selected) = self.selected_channel.clone() else {
            return;
        };
        let Some(pins) = bridge.session_at(selected.origin).and_then(|s| {
            s.extras()
                .pins
                .get(&(chat_kind(selected.kind), selected.channel.clone()))
                .cloned()
        }) else {
            return;
        };
        if pins.is_empty() {
            return;
        }
        let base = bridge.base_at(selected.origin).map(str::to_owned);
        let index = self.pin_index % pins.len();
        let pin = &pins[index];
        let (author, text) = pin_line(pin);
        let thumb = pin_thumb(pin, base.as_deref());
        let mut jump = None;
        ui.horizontal(|ui| {
            let list_width = 44.0;
            let title = {
                let title = if pins.len() > 1 {
                    crate::tf!(
                        "Закреплено {n} из {total}",
                        n = index + 1,
                        total = pins.len()
                    )
                } else {
                    tr("Закреплённое сообщение").to_owned()
                };
                if pin.shared {
                    title
                } else {
                    format!("{title} · {}", tr("только для вас"))
                }
            };
            let frame = pin_card(
                ui,
                &title,
                &format!("{author}: {text}"),
                thumb.as_deref(),
                list_width,
            );
            let response = ui.interact(
                frame.response.rect,
                ui.scope_id().with("pins_bar"),
                Sense::click(),
            );
            if response.clicked() {
                jump = Some(pin.message_id.clone());
                self.pin_index = (index + 1) % pins.len();
            }
            response.on_hover_text(tr("Щёлкните: к сообщению, затем к следующему закрепу"));
            let button = egui::Button::new(
                RichText::new(format!("▾ {}", pins.len()))
                    .size(13.0)
                    .color(theme::text_muted()),
            )
            .min_size(Vec2::new(list_width, frame.response.rect.height()))
            .fill(theme::bg_input())
            .corner_radius(crate::theme::rounded(10));
            let list = ui
                .add(button)
                .on_hover_text(tr("Все закреплённые сообщения"));
            egui::Popup::menu(&list).width(360.0).show(|ui| {
                if let Some(id) = pins_list(ui, &pins, base.as_deref()) {
                    jump = Some(id);
                    ui.close();
                }
            });
        });
        if let Some(message) = jump {
            self.pending_jump = Some((selected, message));
        }
        ui.add_space(4.0);
    }

    /// Окно закладок: со всех подключённых инстансов сразу.
    pub(super) fn show_bookmarks(&mut self, ctx: &egui::Context, bridge: &mut Bridge) {
        let Some(state) = self.bookmarks.as_mut() else {
            return;
        };
        let origins = bridge.live_origins();
        let (mut all, error, more, loaded) = gather_bookmarks(bridge, &origins);
        all.sort_by_key(|(_, b)| std::cmp::Reverse(b.created_at));
        let query = state.query.trim().to_lowercase();
        let mut shown: Vec<(usize, Bookmark)> = all
            .into_iter()
            .filter(|(_, b)| !state.media || !b.attachments.is_empty())
            .filter(|(_, b)| {
                query.is_empty()
                    || b.content
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query)
                    || b.author_name.to_lowercase().contains(&query)
                    || b.channel_name
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query)
            })
            .collect();
        if state.oldest_first {
            shown.reverse();
        }
        let list: Vec<Bookmark> = shown.iter().map(|(_, b)| b.clone()).collect();
        let origin_of = |bookmark: &Bookmark| {
            shown
                .iter()
                .find(|(_, b)| b.id == bookmark.id && b.message_id == bookmark.message_id)
                .map_or(0, |(origin, _)| *origin)
        };
        let mut clicks = Clicks::default();
        let modal = egui::Modal::new(egui::Id::unique("bookmarks")).show(ctx, |ui| {
            bookmarks_body(
                ui,
                state,
                &list,
                &Loaded {
                    error: error.as_deref(),
                    more,
                    done: loaded,
                },
                &mut clicks,
            );
        });
        let Clicks {
            close: closed,
            open,
            remove,
            load_more,
        } = clicks;
        let mut close = closed;
        if load_more {
            for &origin in &origins {
                if let Some(session) = bridge.session_at_mut(origin)
                    && session.extras().bookmarks_next.is_some()
                {
                    session.load_bookmarks(true);
                }
            }
        }
        if let Some(bookmark) = remove
            && let Some(session) = bridge.session_at_mut(origin_of(&bookmark))
        {
            session.set_bookmark(
                bookmark.kind,
                &bookmark.channel_id,
                &bookmark.message_id,
                false,
            );
        }
        if let Some(bookmark) = open {
            let target = SelectedChannel {
                origin: origin_of(&bookmark),
                channel: bookmark.channel_id.clone(),
                kind: match bookmark.kind {
                    ChatKind::Channel => ChannelKind::Space,
                    ChatKind::Dm => ChannelKind::Dm,
                },
            };
            self.pending_jump = Some((target.clone(), bookmark.message_id));
            self.pending_nav = Some((target, bookmark.space_id));
            close = true;
        }
        if close || modal.should_close() {
            self.bookmarks = None;
        }
    }
}

/// Закладки всех инстансов: список, ошибка домашнего, есть ли ещё, загружено ли.
fn gather_bookmarks(
    bridge: &mut Bridge,
    origins: &[usize],
) -> (Vec<(usize, Bookmark)>, Option<String>, bool, bool) {
    let mut all = Vec::new();
    let mut error = None;
    let mut more = false;
    let mut loaded = true;
    for &origin in origins {
        let Some(session) = bridge.session_at_mut(origin) else {
            continue;
        };
        if !session.extras().bookmarks_loaded && session.extras().bookmarks_error.is_none() {
            session.load_bookmarks(false);
        }
        let extras = session.extras();
        // Ошибку показываем только домашнего инстанса: чужой может быть
        // недоступен, остальные закладки от этого не пропадают.
        if origin == 0 {
            error.clone_from(&extras.bookmarks_error);
        }
        more |= extras.bookmarks_next.is_some();
        loaded &= extras.bookmarks_loaded || extras.bookmarks_error.is_some();
        all.extend(extras.bookmarks.iter().cloned().map(|b| (origin, b)));
    }
    (all, error, more, loaded)
}

/// Что щёлкнули в окне закладок.
#[derive(Default)]
struct Clicks {
    close: bool,
    open: Option<Bookmark>,
    remove: Option<Bookmark>,
    load_more: bool,
}

/// Состояние загрузки списка.
struct Loaded<'a> {
    error: Option<&'a str>,
    more: bool,
    done: bool,
}

/// Содержимое окна закладок.
fn bookmarks_body(
    ui: &mut egui::Ui,
    state: &mut BookmarksUi,
    list: &[Bookmark],
    loaded: &Loaded<'_>,
    clicks: &mut Clicks,
) {
    ui.set_width(620.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
        crate::icons::paint(
            ui.painter(),
            rect.center(),
            20.0,
            Icon::Bookmark,
            theme::accent(),
        );
        ui.label(
            RichText::new(tr("Закладки"))
                .size(19.0)
                .family(theme::bold_family())
                .color(theme::text_bright()),
        );
    });
    ui.add_space(6.0);
    bookmark_filters(ui, state);
    ui.add_space(6.0);
    let height = (ui.ctx().content_rect().height() - 260.0).max(240.0);
    egui::ScrollArea::vertical()
        .max_height(height)
        .show(ui, |ui| {
            if let Some(err) = loaded.error {
                ui.label(RichText::new(err).color(theme::rose()));
            } else if !loaded.done {
                ui.spinner();
            } else if list.is_empty() {
                ui.label(theme::muted(tr(
                    "Закладок пока нет. Добавить — «В закладки» в меню сообщения.",
                )));
            }
            let mut current: Option<String> = None;
            for bookmark in list {
                let group = group_label(state.grouping, bookmark);
                if current.as_deref() != Some(group.as_str()) {
                    ui.add_space(8.0);
                    widgets::section_label(ui, &group);
                    current = Some(group);
                }
                match bookmark_row(ui, bookmark) {
                    Some(RowClick::Open) => clicks.open = Some(bookmark.clone()),
                    Some(RowClick::Remove) => clicks.remove = Some(bookmark.clone()),
                    None => {}
                }
            }
            if loaded.more && ui.button(tr("Показать ещё")).clicked() {
                clicks.load_more = true;
            }
        });
    ui.add_space(8.0);
    if ui.button(tr("Закрыть")).clicked() {
        clicks.close = true;
    }
}

/// Может ли пользователь управлять сообщениями канала (общие закрепы).
fn can_manage_messages(bridge: &Bridge, selected: &SelectedChannel) -> bool {
    use bs_permissions::{Permissions, has_permission_bit, parse_permission_string};
    if selected.kind != ChannelKind::Space {
        return false;
    }
    let Some(cache) = bridge.cache_at(selected.origin) else {
        return false;
    };
    let Some(channel) = cache.channel(&selected.channel) else {
        return false;
    };
    let owner = cache
        .space(&channel.space_id)
        .is_some_and(|space| space.space.owner_id == cache.me().id);
    let bits = channel
        .my_permissions
        .as_deref()
        .and_then(|raw| parse_permission_string(raw).ok())
        .unwrap_or(Permissions::empty());
    owner
        || has_permission_bit(bits, Permissions::MANAGE_MESSAGES)
        || has_permission_bit(bits, Permissions::ADMINISTRATOR)
}

/// Пункты меню сообщения: закрепить, в закладки, в «Избранное».
pub(super) fn menu_items(
    ui: &mut egui::Ui,
    menu: &PinMenu,
    message: &str,
    text: Option<&str>,
    in_saved: bool,
) -> Option<ExtrasAction> {
    let mut chosen = None;
    let mut item = |ui: &mut egui::Ui, label: &str, action: ExtrasAction| {
        if ui.button(label).clicked() {
            chosen = Some(action);
            ui.close();
        }
    };
    let pin = |shared: bool, on: bool| ExtrasAction::Pin {
        message: message.to_owned(),
        shared,
        on,
    };
    if menu.shared && menu.can_share {
        item(ui, tr("Открепить"), pin(true, false));
    } else if !menu.shared && menu.can_share {
        let label = if menu.personal {
            tr("Закрепить для обоих")
        } else if in_saved {
            tr("Закрепить")
        } else {
            tr("Закрепить для всех")
        };
        item(ui, label, pin(true, true));
    }
    if menu.personal {
        if menu.mine {
            item(ui, tr("Открепить у себя"), pin(false, false));
        } else {
            item(ui, tr("Закрепить для себя"), pin(false, true));
        }
    }
    let bookmark = |on: bool| ExtrasAction::Bookmark {
        message: message.to_owned(),
        on,
    };
    if menu.marked {
        item(ui, tr("Убрать из закладок"), bookmark(false));
    } else {
        item(ui, tr("В закладки"), bookmark(true));
    }
    if !in_saved && let Some(text) = text.filter(|t| !t.trim().is_empty()) {
        item(
            ui,
            tr("Сохранить в «Избранное»"),
            ExtrasAction::ToSaved {
                text: text.to_owned(),
            },
        );
    }
    chosen
}

/// Строка поиска, группировка, сортировка и фильтр «с медиа».
fn bookmark_filters(ui: &mut egui::Ui, state: &mut BookmarksUi) {
    ui.add(
        egui::TextEdit::singleline(&mut state.query)
            .hint_text(tr("Искать в закладках"))
            .desired_width(f32::INFINITY)
            .margin(Vec2::new(10.0, 6.0)),
    );
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(theme::muted(tr("Группировать:")));
        for (grouping, label) in [
            (Grouping::Date, tr("по дате")),
            (Grouping::Author, tr("по людям")),
            (Grouping::Chat, tr("по чатам")),
        ] {
            ui.selectable_value(&mut state.grouping, grouping, label);
        }
        ui.separator();
        ui.checkbox(&mut state.media, tr("С медиа"));
        ui.checkbox(&mut state.oldest_first, tr("Старые сверху"));
    });
}

/// Заголовок группы закладки.
fn group_label(grouping: Grouping, bookmark: &Bookmark) -> String {
    match grouping {
        Grouping::Date => theme::day_label(bookmark.created_at),
        Grouping::Author => bookmark.author_name.clone(),
        Grouping::Chat => match (&bookmark.space_name, &bookmark.channel_name) {
            (Some(space), Some(channel)) => format!("{space} · #{channel}"),
            (None, Some(channel)) => channel.clone(),
            _ => tr("Чат").to_owned(),
        },
    }
}

/// Щелчок по закладке.
enum RowClick {
    Open,
    Remove,
}

/// Закладка: автор, где, когда, текст, вложения и состояние оригинала.
fn bookmark_row(ui: &mut egui::Ui, bookmark: &Bookmark) -> Option<RowClick> {
    let mut click = None;
    let frame = egui::Frame::new()
        .fill(theme::bg_input())
        .corner_radius(crate::theme::rounded(10))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&bookmark.author_name)
                        .family(theme::bold_family())
                        .color(theme::text_bright()),
                );
                let place = match (&bookmark.space_name, &bookmark.channel_name) {
                    (Some(space), Some(channel)) => format!("{space} · #{channel}"),
                    (None, Some(channel)) => channel.clone(),
                    _ => String::new(),
                };
                ui.label(
                    theme::muted(format!(
                        "{place} · {}",
                        theme::short_stamp(bookmark.message_created_at, super::now_ms())
                    ))
                    .size(12.0),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(widgets::flat_icon(Icon::Close, 12.0, theme::text_faint()))
                        .on_hover_text(tr("Убрать из закладок"))
                        .clicked()
                    {
                        click = Some(RowClick::Remove);
                    }
                    let status = match bookmark.status {
                        BookmarkStatus::Ok => None,
                        BookmarkStatus::Edited => Some(tr("изменено")),
                        BookmarkStatus::Deleted => Some(tr("удалено")),
                        BookmarkStatus::Unavailable => Some(tr("нет доступа")),
                    };
                    if let Some(status) = status {
                        ui.label(RichText::new(status).size(11.5).color(theme::amber()));
                    }
                });
            });
            if let Some(text) = bookmark.content.as_deref().filter(|t| !t.trim().is_empty()) {
                let short: String = text.chars().take(600).collect();
                let blocks = crate::markdown::parse(&short, &|_| None);
                crate::screens::body::show_text(ui, &format!("bm{}", bookmark.id), &blocks, false);
            }
            for att in &bookmark.attachments {
                ui.label(theme::muted(format!("📎 {}", att.original_name)).size(12.0));
            }
        });
    let response = ui.interact(
        frame.response.rect,
        ui.scope_id().with(("bookmark", &bookmark.id)),
        Sense::click(),
    );
    if click.is_none() && response.clicked() && bookmark.status != BookmarkStatus::Deleted {
        click = Some(RowClick::Open);
    }
    ui.add_space(4.0);
    click
}

/// Карточка закрепа в панели: значок, миниатюра, заголовок и строка.
fn pin_card(
    ui: &mut egui::Ui,
    title: &str,
    line: &str,
    thumb: Option<&str>,
    reserve: f32,
) -> egui::InnerResponse<()> {
    egui::Frame::new()
        .fill(theme::bg_input())
        .corner_radius(crate::theme::rounded(10))
        .inner_margin(Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width() - reserve - 24.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                crate::icons::paint(
                    ui.painter(),
                    rect.center(),
                    16.0,
                    Icon::Pin,
                    theme::accent(),
                );
                if let Some(value) = thumb {
                    pin_preview(ui, value, 34.0);
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(
                        RichText::new(title)
                            .size(11.5)
                            .family(theme::bold_family())
                            .color(theme::accent()),
                    );
                    theme::label_fit(
                        ui,
                        RichText::new(line).size(13.0).color(theme::text()),
                        ui.available_width() - 8.0,
                    );
                });
            });
        })
}

/// Автор и текст закрепа; без текста — что во вложении.
fn pin_line(pin: &bs_api::Pin) -> (String, String) {
    let author = pin.message["user"]["displayName"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| pin.message["user"]["username"].as_str())
        .unwrap_or_default()
        .to_owned();
    let text = pin.message["content"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map_or_else(
            || {
                let first = &pin.message["attachments"][0];
                let mime = first["mimetype"].as_str().unwrap_or_default();
                if mime.starts_with("image/") {
                    tr("Фото").to_owned()
                } else if mime.starts_with("video/") {
                    tr("Видео").to_owned()
                } else if let Some(name) = first["originalName"].as_str() {
                    crate::tf!("Файл: {a0}", a0 = name)
                } else {
                    tr("Вложение").to_owned()
                }
            },
            |t| t.replace('\n', " "),
        );
    (author, text)
}

/// Миниатюра первого фото или видео закрепа (полным адресом инстанса).
fn pin_thumb(pin: &bs_api::Pin, base: Option<&str>) -> Option<String> {
    let attachment = pin.message["attachments"].as_array()?.iter().find(|a| {
        let mime = a["mimetype"].as_str().unwrap_or_default();
        mime.starts_with("image/") || mime.starts_with("video/")
    })?;
    let value = attachment["thumbnailFilename"]
        .as_str()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            attachment["mimetype"]
                .as_str()
                .filter(|m| m.starts_with("image/"))
                .and_then(|_| attachment["filename"].as_str())
        })?;
    // Спойлер не раскрываем в миниатюре.
    if crate::media::is_spoiler_name(attachment["originalName"].as_str().unwrap_or_default()) {
        return None;
    }
    Some(base.map_or_else(
        || value.to_owned(),
        |base| crate::images::absolute(base, value),
    ))
}

/// Квадратная миниатюра.
fn pin_preview(ui: &mut egui::Ui, value: &str, side: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    ui.painter()
        .rect_filled(rect, crate::theme::rounded(6), theme::bg_hover());
    crate::images::paint_value(
        ui.painter(),
        rect,
        value,
        crate::theme::rounded(6),
        false,
        egui::Color32::WHITE,
    );
}

/// Список всех закрепов чата; возвращает сообщение, к которому перейти.
fn pins_list(ui: &mut egui::Ui, pins: &[bs_api::Pin], base: Option<&str>) -> Option<String> {
    let mut chosen = None;
    ui.label(
        RichText::new(crate::tf!("Закреплено: {a0}", a0 = pins.len()))
            .size(12.0)
            .family(theme::bold_family())
            .color(theme::text_muted()),
    );
    egui::ScrollArea::vertical()
        .max_height(380.0)
        .show(ui, |ui| {
            for pin in pins {
                let (author, text) = pin_line(pin);
                let row = ui.horizontal(|ui| {
                    ui.set_width(ui.available_width());
                    if let Some(value) = pin_thumb(pin, base) {
                        pin_preview(ui, &value, 40.0);
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        let mut head = author.clone();
                        if !pin.shared {
                            head = format!("{head} · {}", tr("только для вас"));
                        }
                        ui.label(
                            RichText::new(head)
                                .size(12.5)
                                .family(theme::bold_family())
                                .color(theme::text_bright()),
                        );
                        theme::label_fit(
                            ui,
                            RichText::new(text).size(12.5).color(theme::text()),
                            ui.available_width() - 4.0,
                        );
                    });
                });
                let response = ui.interact(
                    row.response.rect,
                    ui.scope_id().with(("pin_row", &pin.message_id)),
                    Sense::click(),
                );
                if response.hovered() {
                    ui.painter().rect_filled(
                        row.response.rect.expand(2.0),
                        crate::theme::rounded(6),
                        theme::bg_hover().gamma_multiply(0.5),
                    );
                }
                if response.clicked() {
                    chosen = Some(pin.message_id.clone());
                }
                ui.add_space(4.0);
            }
        });
    chosen
}
