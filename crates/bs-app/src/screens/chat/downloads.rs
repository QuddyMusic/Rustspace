//! Кнопка «Загрузки» в шапке чата и список скачанного.

use crate::i18n::tr;
use crate::icons::Icon;
use crate::images::{Download, DownloadEntry};
use crate::{theme, widgets};

/// Кнопка «Загрузки» в шапке: видна, когда в этом запуске что-то скачивали;
/// по щелчку — список скачанного с ходом загрузки.
pub(super) fn button(ui: &mut egui::Ui) {
    let Some(hub) = crate::images::ImageHub::of(ui.ctx()) else {
        return;
    };
    let list = hub.downloads();
    if list.is_empty() {
        return;
    }
    let running = list
        .iter()
        .any(|entry| matches!(entry.state, Download::Running { .. }));
    let color = if running {
        theme::accent()
    } else {
        theme::text_faint()
    };
    let button = ui
        .add(widgets::flat_icon(Icon::Download, 22.0, color))
        .on_hover_text(tr("Загрузки"));
    if running {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
    egui::Popup::menu(&button).width(340.0).show(|ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(tr("Загрузки"))
                    .family(theme::bold_family())
                    .color(theme::text_bright()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button(tr("Очистить")).clicked() {
                    hub.clear_finished_downloads();
                }
            });
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                for entry in &list {
                    download_row(ui, entry);
                    ui.add_space(6.0);
                }
            });
    });
}

/// Открыть файл программой по умолчанию или показать его в папке.
fn open(path: &std::path::Path, reveal: bool) {
    let result = if reveal {
        opener::reveal(path)
    } else {
        opener::open(path)
    };
    if let Err(err) = result {
        tracing::warn!(%err, "файл не открылся");
    }
}

/// Строка списка: миниатюра картинки, имя, ход или «Открыть» и «Показать в
/// папке»; щелчок по готовой строке открывает файл.
fn download_row(ui: &mut egui::Ui, entry: &DownloadEntry) {
    let done = matches!(entry.state, Download::Done);
    let row = ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(40.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, theme::rounded(6), theme::bg_hover());
        let is_picture = crate::media::mime_for_name(&entry.label).starts_with("image/");
        if is_picture {
            crate::images::paint_value(
                ui.painter(),
                rect,
                &entry.value,
                theme::rounded(6),
                false,
                egui::Color32::WHITE,
            );
        } else {
            crate::icons::paint(ui.painter(), rect.center(), 20.0, Icon::File, theme::sky());
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(
                egui::RichText::new(theme::truncate_chars(&entry.label, 36))
                    .color(theme::text())
                    .size(13.0),
            )
            .on_hover_text(entry.dest.display().to_string());
            match &entry.state {
                Download::Running { got, total } => {
                    let fraction = Download::percent(*got, *total)
                        .and_then(|p| u8::try_from(p).ok())
                        .map_or(0.0, |p| f32::from(p) / 100.0);
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .desired_height(6.0)
                            .fill(theme::accent()),
                    );
                }
                Download::Done => {
                    ui.horizontal(|ui| {
                        if ui.small_button(tr("Открыть")).clicked() {
                            open(&entry.dest, false);
                        }
                        if ui.small_button(tr("Показать в папке")).clicked() {
                            open(&entry.dest, true);
                        }
                    });
                }
                Download::Failed(message) => {
                    ui.label(
                        egui::RichText::new(theme::truncate_chars(message, 60))
                            .color(theme::rose())
                            .size(12.0),
                    );
                }
            }
        });
    });
    if done {
        let response = ui
            .interact(
                row.response.rect,
                ui.scope_id().with(("download", &entry.value)),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.double_clicked() {
            open(&entry.dest, false);
        }
    }
}
