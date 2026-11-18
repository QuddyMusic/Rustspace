//! Кнопка «Загрузки» в шапке чата и список скачанного.

use crate::i18n::tr;
use crate::icons::Icon;
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
        .any(|(_, _, state)| matches!(state, crate::images::Download::Running { .. }));
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
    egui::Popup::menu(&button).width(320.0).show(|ui| {
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
            .max_height(320.0)
            .show(ui, |ui| {
                for (label, dest, state) in &list {
                    download_row(ui, label, dest, state);
                    ui.add_space(6.0);
                }
            });
    });
}

fn download_row(
    ui: &mut egui::Ui,
    label: &str,
    dest: &std::path::Path,
    state: &crate::images::Download,
) {
    ui.label(
        egui::RichText::new(theme::truncate_chars(label, 40))
            .color(theme::text())
            .size(13.0),
    )
    .on_hover_text(dest.display().to_string());
    match state {
        crate::images::Download::Running { got, total } => {
            let fraction = crate::images::Download::percent(*got, *total)
                .and_then(|p| u8::try_from(p).ok())
                .map_or(0.0, |p| f32::from(p) / 100.0);
            ui.add(
                egui::ProgressBar::new(fraction)
                    .desired_height(6.0)
                    .fill(theme::accent()),
            );
        }
        crate::images::Download::Done => {
            ui.horizontal(|ui| {
                ui.label(theme::muted(tr("Сохранено")).size(12.0));
                if let Some(folder) = dest.parent()
                    && ui.small_button(tr("Открыть папку")).clicked()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(format!(
                        "file://{}",
                        folder.display()
                    )));
                }
            });
        }
        crate::images::Download::Failed(message) => {
            ui.label(
                egui::RichText::new(theme::truncate_chars(message, 60))
                    .color(theme::rose())
                    .size(12.0),
            );
        }
    }
}
