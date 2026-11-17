//! Стикерпаки в выборе стикеров: вкладки паков, создание и правка своих,
//! добавление чужих — из сообщения со стикером.

use bs_api::{PersonalSticker, StickerPack, StickerPacks};
use egui::{Color32, CornerRadius, RichText, Sense, Vec2};

use crate::bridge::{BridgeRequest, PackError, PackOp};
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::{images, theme, widgets};

/// Открытая вкладка выбора стикеров.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Tab {
    /// Личные стикеры.
    #[default]
    Mine,
    /// Пак.
    Pack(String),
    /// Новый пак.
    New,
}

/// Найденные по стикеру паки.
#[derive(Debug, Default)]
struct Found {
    loading: bool,
    packs: Vec<StickerPack>,
    error: Option<String>,
}

/// Паки инстанса в фокусе.
#[derive(Debug, Default)]
pub(crate) struct Packs {
    list: Option<StickerPacks>,
    asked: bool,
    /// Сервер без паков: вкладки не показываем.
    unsupported: bool,
    error: Option<String>,
    tab: Tab,
    new_name: String,
    rename: Option<String>,
    found: Option<Found>,
}

/// Что выбрали во вкладке пака.
#[derive(Debug, Default)]
pub(crate) struct PackOutcome {
    /// Стикер для отправки.
    pub(crate) chosen: Option<String>,
    /// Загрузить картинку сразу в этот пак.
    pub(crate) upload_into: Option<String>,
}

/// Идентификатор стикера из токена `sticker:<url>/api/stickers/assets/<id>.webp`.
#[must_use]
pub(crate) fn sticker_id(token: &str) -> Option<&str> {
    let id = token
        .trim()
        .strip_prefix("sticker:")?
        .rsplit('/')
        .next()?
        .strip_suffix(".webp")?;
    (id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then_some(id)
}

fn pack_title(name: &str) -> RichText {
    RichText::new(name)
        .family(theme::bold_family())
        .size(15.0)
        .color(theme::text_bright())
}

/// Сетка стикеров; `each` получает отклик ячейки, стикер и его номер.
pub(crate) fn grid(
    ui: &mut egui::Ui,
    salt: &str,
    items: &[PersonalSticker],
    mut each: impl FnMut(&egui::Response, &PersonalSticker, usize),
) {
    egui::ScrollArea::vertical()
        .id_salt(salt)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let cell = 88.0;
            let columns = ((ui.available_width() / cell).floor() as usize).max(1);
            for (row_index, row) in items.chunks(columns).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    for (col, sticker) in row.iter().enumerate() {
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
                        if response.hovered() {
                            ui.painter()
                                .rect_filled(rect, theme::rounded(10), theme::bg_active());
                        }
                        images::paint_value(
                            ui.painter(),
                            rect.shrink(6.0),
                            &sticker.token,
                            CornerRadius::ZERO,
                            true,
                            Color32::WHITE,
                        );
                        let response = response.on_hover_text(&sticker.name);
                        each(&response, sticker, row_index * columns + col);
                    }
                });
            }
        });
}

impl Packs {
    /// Ответ моста на действие с паками.
    pub(crate) fn apply(&mut self, result: Result<StickerPacks, PackError>) {
        match result {
            Ok(list) => {
                if let Tab::Pack(id) = &self.tab
                    && !list.own.iter().chain(&list.added).any(|p| &p.id == id)
                {
                    self.tab = Tab::Mine;
                }
                // Найденные паки показывают, добавлен ли пак: обновим отметку.
                if let Some(found) = &mut self.found {
                    for pack in &mut found.packs {
                        pack.added = list.added.iter().any(|p| p.id == pack.id);
                    }
                }
                self.list = Some(list);
                self.error = None;
                self.unsupported = false;
            }
            Err(PackError::Unsupported) => {
                self.unsupported = true;
                self.list = Some(StickerPacks::default());
            }
            Err(PackError::Failed(message)) => self.error = Some(message),
        }
    }

    /// Найденные паки со стикером.
    pub(crate) fn apply_found(&mut self, result: Result<Vec<StickerPack>, String>) {
        let found = self.found.get_or_insert_with(Found::default);
        found.loading = false;
        match result {
            Ok(packs) => {
                found.packs = packs;
                found.error = None;
            }
            Err(message) => found.error = Some(message),
        }
    }

    /// Искать паки со стикером из сообщения.
    pub(crate) fn find(&mut self, bridge: &Bridge, token: &str) {
        let Some(id) = sticker_id(token) else {
            self.found = Some(Found {
                error: Some(tr("Этот стикер не из пака этого инстанса.").to_owned()),
                ..Found::default()
            });
            return;
        };
        self.found = Some(Found {
            loading: true,
            ..Found::default()
        });
        bridge.request(BridgeRequest::StickerPacks(PackOp::With(id.to_owned())));
    }

    fn ensure(&mut self, bridge: &Bridge) {
        if !self.asked {
            self.asked = bridge.request(BridgeRequest::StickerPacks(PackOp::List));
        }
    }

    fn op(bridge: &Bridge, op: PackOp) {
        bridge.request(BridgeRequest::StickerPacks(op));
    }

    /// Открыты личные стикеры?
    pub(crate) fn on_mine(&self) -> bool {
        self.tab == Tab::Mine
    }

    /// Ряд вкладок: «Мои», паки, «+».
    pub(crate) fn tabs(&mut self, ui: &mut egui::Ui, bridge: &Bridge) {
        self.ensure(bridge);
        if self.unsupported {
            return;
        }
        let mut next = None;
        egui::ScrollArea::horizontal()
            .id_salt("sticker_pack_tabs")
            .max_height(34.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if widgets::pill_tab(ui, tr("Мои"), self.tab == Tab::Mine).clicked() {
                        next = Some(Tab::Mine);
                    }
                    if let Some(list) = &self.list {
                        for pack in list.own.iter().chain(&list.added) {
                            let selected = self.tab == Tab::Pack(pack.id.clone());
                            if widgets::pill_tab(ui, &pack.name, selected).clicked() {
                                next = Some(Tab::Pack(pack.id.clone()));
                            }
                        }
                    }
                    if widgets::pill_tab(ui, "+", self.tab == Tab::New)
                        .on_hover_text(tr("Новый стикерпак"))
                        .clicked()
                    {
                        next = Some(Tab::New);
                    }
                });
            });
        if let Some(tab) = next {
            self.rename = None;
            self.tab = tab;
        }
        if let Some(error) = &self.error {
            ui.label(RichText::new(error).color(theme::rose()).size(12.5));
        }
        ui.add_space(4.0);
    }

    /// Пункты контекстного меню личного стикера: положить в свой пак.
    pub(crate) fn mine_menu(&self, ui: &mut egui::Ui, bridge: &Bridge, sticker: &PersonalSticker) {
        let Some(list) = self.list.as_ref().filter(|l| !l.own.is_empty()) else {
            return;
        };
        ui.menu_button(tr("Добавить в стикерпак"), |ui| {
            for pack in &list.own {
                let inside = pack.stickers.iter().any(|s| s.id == sticker.id);
                if ui
                    .add_enabled(!inside, egui::Button::new(&pack.name))
                    .clicked()
                {
                    Self::op(
                        bridge,
                        PackOp::Put {
                            pack: pack.id.clone(),
                            sticker: sticker.id.clone(),
                        },
                    );
                    ui.close();
                }
            }
        });
    }

    /// Вкладка пака или нового пака (когда не «Мои»).
    pub(crate) fn view(&mut self, ui: &mut egui::Ui, bridge: &Bridge) -> PackOutcome {
        let mut out = PackOutcome::default();
        match self.tab.clone() {
            Tab::Mine => {}
            Tab::New => self.new_pack(ui, bridge),
            Tab::Pack(id) => {
                let pack = self
                    .list
                    .as_ref()
                    .and_then(|l| l.own.iter().chain(&l.added).find(|p| p.id == id))
                    .cloned();
                match pack {
                    None => theme::empty_state(ui, tr("Загрузка…")),
                    Some(pack) => out = self.pack(ui, bridge, &pack),
                }
            }
        }
        out
    }

    fn new_pack(&mut self, ui: &mut egui::Ui, bridge: &Bridge) {
        ui.label(theme::section_title(tr("Новый стикерпак")));
        ui.add_space(6.0);
        let mut create = false;
        ui.horizontal(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.new_name)
                    .hint_text(tr("Название"))
                    .char_limit(64)
                    .desired_width(ui.available_width() - 90.0),
            );
            create |= field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            create |= ui
                .add_enabled(
                    !self.new_name.trim().is_empty(),
                    theme::primary_button(tr("Создать")),
                )
                .clicked();
        });
        ui.add_space(8.0);
        ui.label(theme::muted(tr(
            "Соберите стикеры в набор: добавляйте свои картинки или кладите в пак \
             личные стикеры через их меню. Открытый пак могут добавить себе и другие — \
             из меню стикера в чате.",
        )));
        if create && !self.new_name.trim().is_empty() {
            Self::op(bridge, PackOp::Create(self.new_name.trim().to_owned()));
            self.new_name.clear();
            self.tab = Tab::Mine;
        }
    }

    fn pack(&mut self, ui: &mut egui::Ui, bridge: &Bridge, pack: &StickerPack) -> PackOutcome {
        let mut out = PackOutcome::default();
        self.pack_header(ui, bridge, pack, &mut out);
        ui.add_space(4.0);
        if pack.stickers.is_empty() {
            theme::empty_state(
                ui,
                if pack.mine {
                    tr("Пак пуст. Добавьте картинку или положите сюда личные стикеры.")
                } else {
                    tr("Пак пуст.")
                },
            );
            return out;
        }
        let last = pack.stickers.len() - 1;
        let mut moved: Option<(usize, usize)> = None;
        let mut remove = None;
        grid(
            ui,
            "sticker_pack_scroll",
            &pack.stickers,
            |response, sticker, index| {
                if response.clicked() {
                    out.chosen = Some(sticker.token.clone());
                }
                if pack.mine {
                    response.context_menu(|ui| {
                        if index > 0 && ui.button(tr("Сдвинуть влево")).clicked() {
                            moved = Some((index, index - 1));
                            ui.close();
                        }
                        if index < last && ui.button(tr("Сдвинуть вправо")).clicked()
                        {
                            moved = Some((index, index + 1));
                            ui.close();
                        }
                        if ui.button(tr("Убрать из пака")).clicked() {
                            remove = Some(sticker.id.clone());
                            ui.close();
                        }
                    });
                }
            },
        );
        if let Some((from, to)) = moved {
            let mut order: Vec<String> = pack.stickers.iter().map(|s| s.id.clone()).collect();
            order.swap(from, to);
            Self::op(
                bridge,
                PackOp::Reorder {
                    pack: pack.id.clone(),
                    order,
                },
            );
        }
        if let Some(sticker) = remove {
            Self::op(
                bridge,
                PackOp::Remove {
                    pack: pack.id.clone(),
                    sticker,
                },
            );
        }
        out
    }

    fn pack_header(
        &mut self,
        ui: &mut egui::Ui,
        bridge: &Bridge,
        pack: &StickerPack,
        out: &mut PackOutcome,
    ) {
        if let Some(draft) = &mut self.rename {
            let mut done = None;
            ui.horizontal(|ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(draft)
                        .char_limit(64)
                        .desired_width(ui.available_width() - 120.0),
                );
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = Some(true);
                }
                if ui.add(theme::primary_button(tr("Сохранить"))).clicked() {
                    done = Some(true);
                }
                if ui.button("✕").clicked() {
                    done = Some(false);
                }
            });
            if let Some(save) = done {
                let name = self.rename.take().unwrap_or_default();
                if save && !name.trim().is_empty() && name.trim() != pack.name {
                    Self::op(
                        bridge,
                        PackOp::Rename {
                            pack: pack.id.clone(),
                            name: name.trim().to_owned(),
                        },
                    );
                }
            }
            return;
        }
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.add(egui::Label::new(pack_title(&pack.name)).truncate());
                let note = if pack.mine {
                    if pack.shared {
                        tr("Открыт: другие могут добавить").to_owned()
                    } else {
                        tr("Только для вас").to_owned()
                    }
                } else {
                    crate::tf!(
                        "от {owner}",
                        owner = pack.owner_name.as_deref().unwrap_or(&pack.owner_id)
                    )
                };
                ui.label(theme::muted(note).size(11.5));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if pack.mine {
                    ui.menu_button("⋯", |ui| {
                        if ui.button(tr("Переименовать")).clicked() {
                            self.rename = Some(pack.name.clone());
                            ui.close();
                        }
                        let mut shared = pack.shared;
                        if ui
                            .checkbox(&mut shared, tr("Другие могут добавить"))
                            .changed()
                        {
                            Self::op(
                                bridge,
                                PackOp::Share {
                                    pack: pack.id.clone(),
                                    shared,
                                },
                            );
                        }
                        ui.separator();
                        if ui
                            .button(RichText::new(tr("Удалить стикерпак")).color(theme::rose()))
                            .clicked()
                        {
                            Self::op(bridge, PackOp::Delete(pack.id.clone()));
                            ui.close();
                        }
                    });
                    if ui.add(theme::primary_button(tr("Добавить"))).clicked() {
                        out.upload_into = Some(pack.id.clone());
                    }
                } else if ui.button(tr("Убрать у себя")).clicked() {
                    Self::op(
                        bridge,
                        PackOp::Added {
                            pack: pack.id.clone(),
                            added: false,
                        },
                    );
                }
            });
        });
    }

    /// Окно найденных паков (из меню стикера в чате).
    pub(crate) fn found_window(&mut self, ctx: &egui::Context, bridge: &Bridge) {
        let Some(found) = &self.found else {
            return;
        };
        let mut open = true;
        let mut add = None;
        egui::Window::new(tr("Стикерпаки"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(theme::floating(theme::bg_sidebar(), 14).inner_margin(egui::Margin::same(14)))
            .show(ctx, |ui| {
                if found.loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(theme::muted(tr("Ищем паки…")));
                    });
                    return;
                }
                if let Some(error) = &found.error {
                    ui.label(RichText::new(error).color(theme::rose()));
                    return;
                }
                if found.packs.is_empty() {
                    ui.label(theme::muted(tr(
                        "Этот стикер не входит в открытые стикерпаки.",
                    )));
                    return;
                }
                for pack in &found.packs {
                    crate::screens::dialogs::card(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.add(egui::Label::new(pack_title(&pack.name)).truncate());
                                let owner = pack.owner_name.as_deref().unwrap_or(&pack.owner_id);
                                ui.label(
                                    theme::muted(crate::tf!(
                                        "от {owner} · стикеров: {count}",
                                        owner = owner,
                                        count = pack.stickers.len()
                                    ))
                                    .size(11.5),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if pack.mine {
                                        ui.label(theme::muted(tr("Ваш пак")));
                                    } else if pack.added {
                                        ui.label(theme::muted(tr("Добавлен")));
                                    } else if ui
                                        .add(theme::primary_button(tr("Добавить")))
                                        .clicked()
                                    {
                                        add = Some(pack.id.clone());
                                    }
                                },
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for sticker in pack.stickers.iter().take(6) {
                                let (rect, _) =
                                    ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
                                images::paint_value(
                                    ui.painter(),
                                    rect,
                                    &sticker.token,
                                    CornerRadius::ZERO,
                                    true,
                                    Color32::WHITE,
                                );
                            }
                        });
                    });
                    ui.add_space(6.0);
                }
            });
        if let Some(pack) = add {
            Self::op(bridge, PackOp::Added { pack, added: true });
        }
        if !open {
            self.found = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sticker_id;

    #[test]
    fn sticker_id_comes_from_token() {
        let id = "a".repeat(64);
        assert_eq!(
            sticker_id(&format!("sticker:https://h/api/stickers/assets/{id}.webp")),
            Some(id.as_str())
        );
        assert_eq!(
            sticker_id("sticker:https://h/api/stickers/assets/x.webp"),
            None
        );
        assert_eq!(sticker_id("https://h/a.webp"), None);
    }
}
