//! «Оформление» (значок, баннер) и вкладка «Обнаружение» настроек пространства.
//!
//! Тексты — из `spaces.json` (`settings.overview.*`, `settings.discovery.*`).

use std::sync::mpsc::{Receiver, channel};

use bs_types::SpaceVisibility;
use egui::{CornerRadius, Margin, RichText, Vec2};

use super::{DESCRIPTION_MAX, Rights, SpaceSettings, heading, status_lines};
use crate::bridge::SpaceOp;
use crate::fleet::Bridge;
use crate::{images, theme, widgets};

const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;

/// Результат выбора файла: (баннер?, имя, содержимое либо ошибка).
pub(super) type Picked = (bool, String, Result<Vec<u8>, String>);

/// Окно выбора файла в отдельном потоке: UI не замирает.
#[derive(Debug)]
pub(super) struct Picking(Receiver<Picked>);

impl Picking {
    /// Забирает результат, если выбор завершён.
    pub(super) fn poll(&self) -> Option<Picked> {
        self.0.try_recv().ok()
    }
}

fn pick(ctx: &egui::Context, banner: bool) -> Picking {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let chosen = rfd::FileDialog::new()
            .add_filter("Изображения", &["png", "jpg", "jpeg", "gif", "webp"])
            .pick_file();
        if let Some(path) = chosen {
            let name = path
                .file_name()
                .map_or_else(|| "image".to_owned(), |n| n.to_string_lossy().into_owned());
            let bytes = match std::fs::metadata(&path) {
                Ok(meta) if meta.len() > MAX_IMAGE_BYTES => Err("Картинка больше 16 МБ".to_owned()),
                Ok(_) => std::fs::read(&path).map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send((banner, name, bytes));
        }
        ctx.request_repaint();
    });
    Picking(rx)
}

/// Забирает выбранный файл и отправляет его на сервер.
pub(super) fn drain_pick(state: &mut SpaceSettings, bridge: &mut Bridge) {
    let Some(picked) = state.picking.as_ref().and_then(Picking::poll) else {
        return;
    };
    state.picking = None;
    match picked {
        (banner, name, Ok(bytes)) => {
            state.uploading = Some(banner);
            state.request(bridge, SpaceOp::UploadImage(banner, name, bytes));
        }
        (_, _, Err(err)) => state.error = Some(err),
    }
}

/// Блок «Оформление пространства»: значок и баннер.
pub(super) fn appearance(
    ui: &mut egui::Ui,
    state: &mut SpaceSettings,
    rights: Rights,
    bridge: &Bridge,
) {
    widgets::section_label(ui, "Оформление пространства");
    ui.add_space(6.0);
    let base = bridge.base_at(state.origin).unwrap_or_default();
    image_row(ui, state, rights, base, false);
    ui.add_space(8.0);
    image_row(ui, state, rights, base, true);
    ui.add_space(8.0);
}

fn image_row(
    ui: &mut egui::Ui,
    state: &mut SpaceSettings,
    rights: Rights,
    base: &str,
    banner: bool,
) {
    let (label, upload, remove, size) = if banner {
        (
            "Баннер пространства",
            "Загрузить баннер (16:9)",
            "Удалить баннер",
            Vec2::new(160.0, 90.0),
        )
    } else {
        (
            "Значок пространства",
            "Загрузить значок",
            "Удалить значок",
            Vec2::splat(64.0),
        )
    };
    widgets::field_label(ui, label);
    let value = if banner {
        state.fields.banner.clone()
    } else {
        state.fields.icon.clone()
    };
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let radius = CornerRadius::same(if banner { 10 } else { 16 });
        ui.painter().rect_filled(rect, radius, theme::bg_rail());
        let shown = !value.is_empty()
            && images::paint_value(
                ui.painter(),
                rect,
                &images::absolute(base, &value),
                radius,
                true,
                egui::Color32::WHITE,
            );
        if !shown && !value.is_empty() {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "…",
                egui::FontId::proportional(18.0),
                theme::text_muted(),
            );
        }
        ui.vertical(|ui| {
            ui.add_enabled_ui(rights.manage_space, |ui| {
                let busy = state.picking.is_some() || state.uploading == Some(banner);
                let text = if state.uploading == Some(banner) {
                    "Загрузка…"
                } else {
                    upload
                };
                if ui.add_enabled(!busy, egui::Button::new(text)).clicked() {
                    state.picking = Some(pick(ui.ctx(), banner));
                }
                if !value.is_empty() && ui.button(remove).clicked() {
                    if banner {
                        state.fields.banner.clear();
                    } else {
                        state.fields.icon.clear();
                    }
                }
            });
        });
    });
}

/// Панель «Сохранить / Отменить изменения» для общих полей.
pub(super) fn save_bar(
    ui: &mut egui::Ui,
    state: &mut SpaceSettings,
    rights: Rights,
    bridge: &mut Bridge,
) {
    if !(rights.manage_space && state.fields != state.saved) {
        return;
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let valid = !state.fields.name.trim().is_empty() && state.uploading.is_none();
        let label = if state.busy {
            "Сохранение…"
        } else {
            "Сохранить"
        };
        if ui
            .add_enabled(valid && !state.busy, theme::primary_button(label))
            .clicked()
        {
            let op = SpaceOp::Update(state.update_patch());
            state.request(bridge, op);
        }
        if ui.button("Сбросить").clicked() {
            state.fields = state.saved.clone();
        }
    });
}

/// Вкладка «Обнаружение».
pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut SpaceSettings,
    rights: Rights,
    bridge: &mut Bridge,
) {
    heading(ui, "Обнаружение");
    ui.add_enabled_ui(rights.manage_space, |ui| {
        widgets::field_label(ui, "Видимость");
        ui.label(
            RichText::new(
                "Управляйте тем, кто может находить это пространство и присоединяться к нему.",
            )
            .color(theme::text_muted()),
        );
        ui.add_space(4.0);
        for (value, title, hint) in [
            (
                SpaceVisibility::Private,
                "Закрытое",
                "Присоединиться могут только те, у кого есть ссылка для приглашения",
            ),
            (
                SpaceVisibility::Request,
                "По запросу",
                "Видно в разделе «Обзор»; можно запросить вступление",
            ),
            (
                SpaceVisibility::Public,
                "Открытое",
                "Видно в разделе «Обзор»; присоединиться может любой",
            ),
        ] {
            if widgets::radio_card(ui, title, hint, state.fields.visibility == value).clicked() {
                state.fields.visibility = value;
            }
        }
        ui.add_space(10.0);
        directory(ui, state);
        ui.add_space(10.0);
        widgets::field_label(ui, "Описание");
        ui.label(
            RichText::new("Краткое описание, отображаемое на странице «Обзор».")
                .color(theme::text_muted()),
        );
        widgets::text_area(
            ui,
            &mut state.fields.description,
            "Краткое описание для страницы «Обзор»…",
            DESCRIPTION_MAX,
            3,
        );
    });
    status_lines(ui, state);
    save_bar(ui, state, rights, bridge);
    if state.fields.visibility == SpaceVisibility::Request && rights.manage_space {
        join_requests(ui, state, bridge);
    }
}

fn directory(ui: &mut egui::Ui, state: &mut SpaceSettings) {
    widgets::section_label(ui, "Внешний каталог Backspace");
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(theme::bg_rail())
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let locked = state.fields.visibility == SpaceVisibility::Private;
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Добавить во внешний каталог Backspace")
                            .color(theme::text_bright()),
                    );
                    ui.label(
                        RichText::new(
                            "Показывать это пространство во внешнем пространстве на каждом сервере Backspace.",
                        )
                        .size(12.0)
                        .color(theme::text_muted()),
                    );
                });
                ui.add_enabled_ui(!locked, |ui| {
                    widgets::toggle(ui, &mut state.fields.directory_listed);
                });
            });
            if locked {
                ui.label(
                    RichText::new("Сначала сделайте пространство открытым или доступным по запросу.")
                        .size(12.0)
                        .color(theme::text_muted()),
                );
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "После добавления становятся общедоступными: название, описание, значок, баннер и число участников пространства, а также адрес этого сервера. Те, кто просматривает внешний каталог, загружают значок и баннер с этого сервера.",
                )
                .size(12.0)
                .color(theme::text_muted()),
            );
        });
}

fn join_requests(ui: &mut egui::Ui, state: &mut SpaceSettings, bridge: &mut Bridge) {
    ui.add_space(18.0);
    widgets::section_label(ui, "Ожидающие запросы на вступление");
    ui.add_space(6.0);
    let Some(list) = state.requests.clone() else {
        ui.label(RichText::new("Загрузка…").color(theme::text_muted()));
        return;
    };
    if list.is_empty() {
        ui.label(RichText::new("Нет ожидающих запросов на вступление").color(theme::text_muted()));
        return;
    }
    for request in &list {
        let name = request
            .user
            .as_ref()
            .map_or_else(|| "Неизвестно".to_owned(), super::display_name);
        egui::Frame::new()
            .fill(theme::bg_rail())
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::avatar(ui, &name, 32.0, None, None);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&name).color(theme::text_bright()));
                        if let Some(message) = request.message.as_deref().filter(|m| !m.is_empty())
                        {
                            ui.label(RichText::new(message).size(12.0).color(theme::text_muted()));
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!state.busy, egui::Button::new("Отклонить"))
                            .clicked()
                        {
                            state
                                .request(bridge, SpaceOp::DecideRequest(request.id.clone(), false));
                        }
                        if ui
                            .add_enabled(!state.busy, egui::Button::new("Принять"))
                            .clicked()
                        {
                            state.request(bridge, SpaceOp::DecideRequest(request.id.clone(), true));
                        }
                    });
                });
            });
        ui.add_space(4.0);
    }
}
