//! Папки личных бесед: вкладки над списком.
//!
//! Автоматические вкладки — «Все», «Непрочитанные», «Люди», «Группы» и по
//! вкладке на каждый мост («Telegram», «Matrix», «Discord») — появляются,
//! только когда в них есть беседы. Свои папки создаются кнопкой «+» или из
//! меню беседы («В папку»); ПКМ по своей папке — переименовать или удалить.
//! Всё хранится в настройках клиента (`prefs.json`).

use std::collections::BTreeSet;

use egui::{Align2, FontId, RichText, Sense};
use serde::{Deserialize, Serialize};

use super::SidebarDm;
use crate::i18n::tr;
use crate::{prefs, theme};

/// Своя папка.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Folder {
    pub(crate) id: u32,
    pub(crate) name: String,
    /// Идентификаторы личных каналов.
    #[serde(default)]
    pub(crate) chats: BTreeSet<String>,
}

/// Папки и выбранная вкладка (в `prefs.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ChatFolders {
    pub(crate) folders: Vec<Folder>,
    /// Ключ выбранной вкладки (см. [`Tab::key`]).
    pub(crate) tab: String,
}

/// Вкладка над списком бесед.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Tab {
    All,
    Unread,
    People,
    Groups,
    /// Беседы одного моста («Telegram»).
    Network(String),
    Folder(u32),
}

impl Tab {
    fn key(&self) -> String {
        match self {
            Self::All => "all".to_owned(),
            Self::Unread => "unread".to_owned(),
            Self::People => "people".to_owned(),
            Self::Groups => "groups".to_owned(),
            Self::Network(name) => format!("net:{name}"),
            Self::Folder(id) => format!("folder:{id}"),
        }
    }

    fn parse(key: &str) -> Self {
        match key {
            "unread" => Self::Unread,
            "people" => Self::People,
            "groups" => Self::Groups,
            _ => key
                .strip_prefix("net:")
                .map(|name| Self::Network(name.to_owned()))
                .or_else(|| {
                    key.strip_prefix("folder:")
                        .and_then(|id| id.parse().ok())
                        .map(Self::Folder)
                })
                .unwrap_or(Self::All),
        }
    }

    /// Беседа во вкладке.
    pub(crate) fn matches(&self, dm: &SidebarDm, folders: &ChatFolders) -> bool {
        match self {
            Self::All => true,
            Self::Unread => dm.unread,
            Self::People => dm.network.is_none() && !dm.group,
            Self::Groups => dm.network.is_none() && dm.group,
            Self::Network(name) => dm.network == Some(name.as_str()),
            Self::Folder(id) => folders
                .folders
                .iter()
                .any(|folder| folder.id == *id && folder.chats.contains(&dm.id)),
        }
    }
}

/// Вкладки, в которых что-то есть (и выбранная, даже пустая).
fn visible_tabs(dms: &[SidebarDm], folders: &ChatFolders, selected: &Tab) -> Vec<(Tab, String)> {
    let mut tabs = vec![(Tab::All, tr("Все").to_owned())];
    let mut add = |tab: Tab, label: String| {
        if &tab == selected || dms.iter().any(|dm| tab.matches(dm, folders)) {
            tabs.push((tab, label));
        }
    };
    add(Tab::Unread, tr("Непрочитанные").to_owned());
    add(Tab::People, tr("Люди").to_owned());
    add(Tab::Groups, tr("Группы").to_owned());
    let networks: BTreeSet<&str> = dms.iter().filter_map(|dm| dm.network).collect();
    for name in networks {
        add(Tab::Network(name.to_owned()), name.to_owned());
    }
    for folder in &folders.folders {
        tabs.push((Tab::Folder(folder.id), folder.name.clone()));
    }
    tabs
}

/// Состояние окошек «новая папка» и «переименовать».
#[derive(Debug, Clone, Default)]
struct FolderUi {
    /// Имя новой или переименовываемой папки и чья она (`None` — новая).
    editing: Option<(Option<u32>, String)>,
    /// Поле имени уже получило фокус (просим его один раз, иначе Enter
    /// не отпускает поле и не срабатывает).
    focused: bool,
}

fn rename_request_id() -> egui::Id {
    egui::Id::unique("bs-folder-rename")
}

/// Полоса вкладок. Возвращает выбранную вкладку.
pub(crate) fn tabs_bar(ui: &mut egui::Ui, dms: &[SidebarDm]) -> Tab {
    let id = egui::Id::unique("bs-folder-ui");
    let mut state = ui.data(|d| d.get_temp::<FolderUi>(id)).unwrap_or_default();
    let tab = tabs_bar_with(ui, dms, &mut state);
    ui.data_mut(|d| d.insert_temp(id, state));
    tab
}

fn tabs_bar_with(ui: &mut egui::Ui, dms: &[SidebarDm], state: &mut FolderUi) -> Tab {
    let folders = prefs::get().chat_folders;
    let mut current = Tab::parse(&folders.tab);
    // Выбранной своей папки больше нет — показываем все.
    if let Tab::Folder(id) = current
        && !folders.folders.iter().any(|folder| folder.id == id)
    {
        current = Tab::All;
    }
    if let Some(id) = ui.data_mut(|d| d.remove_temp::<u32>(rename_request_id()))
        && let Some(folder) = folders.folders.iter().find(|folder| folder.id == id)
    {
        state.editing = Some((Some(id), folder.name.clone()));
    }
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 10,
            right: 6,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for (tab, label) in visible_tabs(dms, &folders, &current) {
                    let unread = tab != Tab::Unread
                        && dms.iter().any(|dm| dm.unread && tab.matches(dm, &folders));
                    let response = chip(ui, &label, tab == current, unread);
                    if response.clicked() {
                        prefs::update(|p| p.chat_folders.tab = tab.key());
                        current = tab.clone();
                    }
                    if let Tab::Folder(id) = tab {
                        folder_menu(&response, id, &label, state);
                    }
                }
                if chip(ui, "+", false, false)
                    .on_hover_text(tr("Новая папка"))
                    .clicked()
                {
                    state.editing = Some((None, String::new()));
                }
            })
        });
    edit_window(ui.ctx(), state);
    current
}

/// Маленькая вкладка-«таблетка» с точкой непрочитанного.
fn chip(ui: &mut egui::Ui, label: &str, selected: bool, unread: bool) -> egui::Response {
    let font = FontId::proportional(12.5);
    let color = if selected {
        theme::text_bright()
    } else {
        theme::text_muted()
    };
    let galley = ui
        .painter()
        .layout_no_wrap(theme::truncate_chars(label, 18), font, color);
    let dot = if unread { 8.0 } else { 0.0 };
    let size = galley.size() + egui::vec2(16.0 + dot, 8.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = if selected {
        theme::bg_active()
    } else if response.hovered() {
        theme::bg_hover()
    } else {
        theme::bg_input()
    };
    ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
    let text_at = rect.left_center() + egui::vec2(8.0, -galley.size().y / 2.0);
    ui.painter().galley(text_at, galley, color);
    if unread {
        ui.painter().circle_filled(
            rect.right_center() - egui::vec2(8.0, 0.0),
            3.0,
            theme::rose(),
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// ПКМ по своей папке.
fn folder_menu(response: &egui::Response, id: u32, name: &str, state: &mut FolderUi) {
    response.context_menu(|ui| {
        crate::widgets::menu_spacing(ui);
        if ui.button(tr("Переименовать")).clicked() {
            state.editing = Some((Some(id), name.to_owned()));
            ui.close();
        }
        if ui
            .button(RichText::new(tr("Удалить папку")).color(theme::rose()))
            .clicked()
        {
            prefs::update(|p| {
                p.chat_folders.folders.retain(|folder| folder.id != id);
                if Tab::parse(&p.chat_folders.tab) == Tab::Folder(id) {
                    p.chat_folders.tab = Tab::All.key();
                }
            });
            ui.close();
        }
    });
}

/// Окошко имени папки.
fn edit_window(ctx: &egui::Context, state: &mut FolderUi) {
    let focus = !state.focused;
    state.focused = state.editing.is_some();
    let Some((target, name)) = &mut state.editing else {
        return;
    };
    let title = if target.is_some() {
        tr("Переименовать папку")
    } else {
        tr("Новая папка")
    };
    let mut done = None;
    let mut open = true;
    egui::Window::new(title)
        .id(egui::Id::unique("bs-folder-edit"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            let field = ui.add(
                egui::TextEdit::singleline(name)
                    .hint_text(tr("Название"))
                    .char_limit(32)
                    .desired_width(220.0),
            );
            if focus {
                field.request_focus();
            }
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ok =
                    ui.add_enabled(!name.trim().is_empty(), egui::Button::new(tr("Сохранить")));
                if (ok.clicked() || enter) && !name.trim().is_empty() {
                    done = Some(name.trim().to_owned());
                }
                if ui.button(tr("Отмена")).clicked() {
                    done = Some(String::new());
                }
            });
        });
    if !open {
        state.editing = None;
        return;
    }
    let Some(name) = done else { return };
    let target = *target;
    state.editing = None;
    if name.is_empty() {
        return;
    }
    prefs::update(|p| {
        if let Some(id) = target {
            if let Some(folder) = p.chat_folders.folders.iter_mut().find(|f| f.id == id) {
                folder.name = name;
            }
            return;
        }
        let id = next_id(&p.chat_folders);
        p.chat_folders.folders.push(Folder {
            id,
            name,
            chats: BTreeSet::new(),
        });
        p.chat_folders.tab = Tab::Folder(id).key();
    });
}

fn next_id(folders: &ChatFolders) -> u32 {
    folders
        .folders
        .iter()
        .map(|f| f.id)
        .max()
        .map_or(1, |id| id + 1)
}

/// Пункт «В папку» меню беседы.
pub(crate) fn dm_menu(ui: &mut egui::Ui, dm_id: &str) {
    ui.menu_button(tr("В папку"), |ui| {
        let folders = prefs::get().chat_folders;
        for folder in &folders.folders {
            let mut inside = folder.chats.contains(dm_id);
            if ui.checkbox(&mut inside, &folder.name).changed() {
                let id = folder.id;
                prefs::update(|p| {
                    if let Some(folder) = p.chat_folders.folders.iter_mut().find(|f| f.id == id) {
                        if inside {
                            folder.chats.insert(dm_id.to_owned());
                        } else {
                            folder.chats.remove(dm_id);
                        }
                    }
                });
            }
        }
        if !folders.folders.is_empty() {
            ui.separator();
        }
        if ui.button(tr("Новая папка…")).clicked() {
            let mut created = 0;
            prefs::update(|p| {
                let id = next_id(&p.chat_folders);
                let mut chats = BTreeSet::new();
                chats.insert(dm_id.to_owned());
                p.chat_folders.folders.push(Folder {
                    id,
                    name: crate::tf!("Папка {n}", n = id.to_string()),
                    chats,
                });
                created = id;
            });
            ui.data_mut(|d| d.insert_temp(rename_request_id(), created));
            ui.close();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dm(id: &str, network: Option<&'static str>, group: bool, unread: bool) -> SidebarDm {
        SidebarDm {
            origin: 0,
            key: None,
            id: id.to_owned(),
            label: id.to_owned(),
            unread,
            status: None,
            preview: None,
            at: None,
            avatar: None,
            face: (id.to_owned(), None),
            bridged: network.is_some(),
            network,
            group,
        }
    }

    #[test]
    fn tabs_sort_chats() {
        let mut folders = ChatFolders::default();
        folders.folders.push(Folder {
            id: 1,
            name: "Работа".to_owned(),
            chats: ["b".to_owned()].into(),
        });
        let (person, group, tg) = (
            dm("a", None, false, true),
            dm("b", None, true, false),
            dm("c", Some("Telegram"), false, false),
        );
        assert!(Tab::People.matches(&person, &folders));
        assert!(!Tab::People.matches(&tg, &folders));
        assert!(Tab::Groups.matches(&group, &folders));
        assert!(Tab::Network("Telegram".to_owned()).matches(&tg, &folders));
        assert!(Tab::Unread.matches(&person, &folders));
        assert!(Tab::Folder(1).matches(&group, &folders));
        assert!(!Tab::Folder(1).matches(&person, &folders));
        let dms = [person, group, tg];
        let shown: Vec<_> = visible_tabs(&dms, &folders, &Tab::All)
            .into_iter()
            .map(|(tab, _)| tab.key())
            .collect();
        assert_eq!(
            shown,
            [
                "all",
                "unread",
                "people",
                "groups",
                "net:Telegram",
                "folder:1"
            ]
        );
    }

    #[test]
    fn keys_round_trip() {
        for tab in [
            Tab::All,
            Tab::Unread,
            Tab::People,
            Tab::Groups,
            Tab::Network("Matrix".to_owned()),
            Tab::Folder(7),
        ] {
            assert_eq!(Tab::parse(&tab.key()), tab);
        }
        assert_eq!(Tab::parse("мусор"), Tab::All);
    }
}
