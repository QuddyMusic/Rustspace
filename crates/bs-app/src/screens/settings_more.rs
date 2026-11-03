//! Админские разделы «Стриминг», «Телеметрия» и «Обновления».

use bs_api::{StreamingLimits, StreamingPatch, TelemetryStatus, UpdateStatus};
use egui::RichText;

use super::settings_extra::{Note, fail, number_row, setting_row, show_note};
use crate::bridge::{AdminEvent, AdminRequest, BridgeRequest};
use crate::fleet::Bridge;
use crate::screens::dialogs::{card, page_title};
use crate::theme;
use crate::widgets;

/// Разрешения, которые знает сервер.
const RESOLUTIONS: [u32; 5] = [540, 720, 1080, 1440, 2160];
/// Частоты кадров, которые знает сервер.
const FRAMERATES: [u32; 6] = [30, 45, 60, 75, 90, 120];

/// Состояние этих разделов.
#[derive(Debug, Default)]
pub(crate) struct MoreUi {
    streaming: Option<StreamingLimits>,
    edit: Option<StreamingLimits>,
    streaming_asked: bool,
    streaming_note: Option<Note>,
    telemetry: Option<TelemetryStatus>,
    telemetry_asked: bool,
    preview: Option<String>,
    telemetry_note: Option<Note>,
    updates: Option<UpdateStatus>,
    updates_asked: bool,
    updates_note: Option<Note>,
}

impl MoreUi {
    /// Забирает «свои» события; остальные возвращает вызывающему.
    pub(crate) fn accept(&mut self, event: AdminEvent) -> Option<AdminEvent> {
        match event {
            AdminEvent::Streaming(result) => match result {
                Ok(limits) => {
                    self.edit = Some(limits.clone());
                    self.streaming = Some(limits);
                    self.streaming_note = None;
                }
                Err(message) => self.streaming_note = Some(fail(message)),
            },
            AdminEvent::Telemetry(result) => match result {
                Ok(status) => {
                    self.telemetry = Some(status);
                    self.telemetry_note = None;
                    self.preview = None;
                }
                Err(message) => self.telemetry_note = Some(fail(message)),
            },
            AdminEvent::TelemetryPreview(result) => match result {
                Ok(text) => self.preview = Some(text),
                Err(message) => self.telemetry_note = Some(fail(message)),
            },
            AdminEvent::Updates(result) => match result {
                Ok(status) => {
                    self.updates = Some(status);
                    self.updates_note = None;
                }
                Err(message) => self.updates_note = Some(fail(message)),
            },
            other => return Some(other),
        }
        None
    }
}

fn request(bridge: &Bridge, request: AdminRequest) -> bool {
    bridge.request(BridgeRequest::Admin(request))
}

/// Что изменилось в ограничениях.
pub(crate) fn streaming_patch(saved: &StreamingLimits, edit: &StreamingLimits) -> StreamingPatch {
    let mut patch = StreamingPatch::default();
    if edit.max_bitrate_kbps != saved.max_bitrate_kbps {
        patch.max_bitrate_kbps = Some(edit.max_bitrate_kbps);
    }
    if edit.min_bitrate_kbps != saved.min_bitrate_kbps {
        patch.min_bitrate_kbps = Some(edit.min_bitrate_kbps);
    }
    if edit.bitrate_step_kbps != saved.bitrate_step_kbps {
        patch.bitrate_step_kbps = Some(edit.bitrate_step_kbps);
    }
    if edit.allowed_resolutions != saved.allowed_resolutions {
        patch.allowed_resolutions = Some(edit.allowed_resolutions.clone());
    }
    if edit.allowed_framerates != saved.allowed_framerates {
        patch.allowed_framerates = Some(edit.allowed_framerates.clone());
    }
    if edit.max_resolution != saved.max_resolution {
        patch.max_resolution = Some(edit.max_resolution);
    }
    if edit.max_framerate != saved.max_framerate {
        patch.max_framerate = Some(edit.max_framerate);
    }
    if edit.allow_custom_bitrate != saved.allow_custom_bitrate {
        patch.allow_custom_bitrate = Some(edit.allow_custom_bitrate);
    }
    patch
}

fn is_native(value: &serde_json::Value) -> bool {
    value.as_str() == Some("native")
}

fn has_resolution(list: &[serde_json::Value], res: Option<u32>) -> bool {
    list.iter()
        .any(|v| res.map_or_else(|| is_native(v), |r| v.as_u64() == Some(u64::from(r))))
}

fn toggle_resolution(list: &mut Vec<serde_json::Value>, res: Option<u32>, on: bool) {
    list.retain(|v| res.map_or_else(|| !is_native(v), |r| v.as_u64() != Some(u64::from(r))));
    if on {
        list.push(res.map_or_else(|| serde_json::json!("native"), |r| serde_json::json!(r)));
    }
    // Числа по возрастанию, «native» в конце — как хранит сервер.
    list.sort_by_key(|v| v.as_u64().unwrap_or(u64::MAX));
}

fn combo(ui: &mut egui::Ui, id: &str, value: &mut u32, options: &[u32], suffix: &str) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(format!("{value}{suffix}"))
        .show_ui(ui, |ui| {
            for option in options {
                ui.selectable_value(value, *option, format!("{option}{suffix}"));
            }
        });
}

/// «Стриминг»: пределы качества демонстрации экрана и камеры.
#[allow(clippy::too_many_lines, reason = "форма настроек одним блоком")]
pub(crate) fn streaming_panel(ui: &mut egui::Ui, more: &mut MoreUi, bridge: &Bridge) {
    page_title(ui, "Стриминг");
    if !more.streaming_asked {
        more.streaming_asked = request(bridge, AdminRequest::LoadStreaming);
    }
    let (Some(saved), Some(edit)) = (more.streaming.clone(), more.edit.as_mut()) else {
        show_note(ui, more.streaming_note.as_ref());
        ui.label(theme::muted("Загрузка…"));
        return;
    };
    card(ui, |ui| {
        number_row(
            ui,
            "Максимальный битрейт, кбит/с",
            "Выше этого значения поток не поднимается.",
            &mut edit.max_bitrate_kbps,
            (500, 1_000_000),
        );
        number_row(
            ui,
            "Минимальный битрейт, кбит/с",
            "Ниже этого значения поток не опускается.",
            &mut edit.min_bitrate_kbps,
            (100, 1_000_000),
        );
        number_row(
            ui,
            "Шаг битрейта, кбит/с",
            "Шаг ползунка качества.",
            &mut edit.bitrate_step_kbps,
            (50, 5_000),
        );
        setting_row(
            ui,
            "Свой битрейт",
            "Разрешить пользователям задавать битрейт вручную.",
            &mut edit.allow_custom_bitrate,
        );
        ui.horizontal(|ui| {
            ui.label("Предельное разрешение");
            combo(ui, "max_res", &mut edit.max_resolution, &RESOLUTIONS, "p");
            ui.add_space(16.0);
            ui.label("Предельная частота кадров");
            combo(ui, "max_fps", &mut edit.max_framerate, &FRAMERATES, " fps");
        });
    });
    ui.add_space(10.0);
    widgets::section_label(ui, "Допустимые разрешения");
    card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for res in RESOLUTIONS {
                let mut on = has_resolution(&edit.allowed_resolutions, Some(res));
                if ui.checkbox(&mut on, format!("{res}p")).changed() {
                    toggle_resolution(&mut edit.allowed_resolutions, Some(res), on);
                }
            }
            let mut native = has_resolution(&edit.allowed_resolutions, None);
            if ui.checkbox(&mut native, "Исходное").changed() {
                toggle_resolution(&mut edit.allowed_resolutions, None, native);
            }
        });
    });
    ui.add_space(10.0);
    widgets::section_label(ui, "Допустимая частота кадров");
    card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for fps in FRAMERATES {
                let mut on = edit.allowed_framerates.contains(&fps);
                if ui.checkbox(&mut on, format!("{fps} fps")).changed() {
                    edit.allowed_framerates.retain(|f| *f != fps);
                    if on {
                        edit.allowed_framerates.push(fps);
                    }
                    edit.allowed_framerates.sort_unstable();
                }
            }
        });
    });
    let patch = streaming_patch(&saved, edit);
    let changed = patch != StreamingPatch::default();
    ui.add_space(14.0);
    show_note(ui, more.streaming_note.as_ref());
    ui.horizontal(|ui| {
        if ui
            .add_enabled(changed, theme::primary_button("Сохранить"))
            .clicked()
        {
            request(bridge, AdminRequest::SaveStreaming(patch));
        }
        if ui
            .add_enabled(changed, egui::Button::new("Сбросить"))
            .clicked()
        {
            more.edit.clone_from(&more.streaming);
        }
    });
}

/// «Телеметрия»: согласие и предпросмотр отправляемого.
pub(crate) fn telemetry_panel(ui: &mut egui::Ui, more: &mut MoreUi, bridge: &Bridge) {
    page_title(ui, "Телеметрия");
    if !more.telemetry_asked {
        more.telemetry_asked = request(bridge, AdminRequest::LoadTelemetry);
    }
    show_note(ui, more.telemetry_note.as_ref());
    let Some(status) = more.telemetry.clone() else {
        ui.label(theme::muted("Загрузка…"));
        return;
    };
    card(ui, |ui| {
        ui.label(
            "Раз в сутки инстанс может отправлять разработчикам обезличенную сводку: \
             версию и обобщённые счётчики. Никаких сообщений и имён.",
        );
        ui.add_space(8.0);
        let mut on = status.enabled == Some(true);
        let before = on;
        setting_row(
            ui,
            "Отправлять телеметрию",
            match status.enabled {
                None => "Выбор ещё не сделан.",
                Some(true) => "Включено.",
                Some(false) => "Выключено.",
            },
            &mut on,
        );
        if on != before {
            request(bridge, AdminRequest::SetTelemetry(on));
        }
        if let Some(id) = &status.id {
            let masked: String = id.chars().take(8).collect();
            ui.label(theme::muted(format!("Идентификатор: {masked}…")).size(12.5));
        }
        if let Some(day) = &status.last_day {
            ui.label(theme::muted(format!("Последняя отправка: {day}")).size(12.5));
        }
    });
    ui.add_space(10.0);
    if ui.button("Показать, что будет отправлено").clicked() {
        request(bridge, AdminRequest::LoadTelemetryPreview);
    }
    if let Some(preview) = &more.preview {
        ui.add_space(6.0);
        card(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(280.0)
                .show(ui, |ui| {
                    ui.label(RichText::new(preview).monospace().size(12.5));
                });
        });
    }
}

fn state_label(status: &UpdateStatus) -> String {
    match status.state.as_str() {
        "up-to-date" => "Установлена последняя версия".to_owned(),
        "update-available" => "Доступно обновление".to_owned(),
        _ => match status.reason.as_deref() {
            Some("disabled") => "Проверка обновлений отключена на сервере".to_owned(),
            Some("unreachable") => "Не удалось связаться с сервером релизов".to_owned(),
            Some("rate-limited") => "Сервер релизов ограничил запросы, попробуйте позже".to_owned(),
            Some("unparseable") => "Не удалось разобрать ответ сервера релизов".to_owned(),
            _ => "Состояние неизвестно".to_owned(),
        },
    }
}

/// «Обновления»: версия, последний релиз и способ установки.
pub(crate) fn updates_panel(ui: &mut egui::Ui, more: &mut MoreUi, bridge: &Bridge) {
    page_title(ui, "Обновления");
    if !more.updates_asked {
        more.updates_asked = request(bridge, AdminRequest::LoadUpdates(false));
    }
    show_note(ui, more.updates_note.as_ref());
    let Some(status) = more.updates.clone() else {
        ui.label(theme::muted("Загрузка…"));
        return;
    };
    card(ui, |ui| {
        let commit = status
            .current
            .commit
            .as_deref()
            .map(|c| format!(" ({c})"))
            .unwrap_or_default();
        ui.label(format!("Установлено: {}{commit}", status.current.version));
        if let Some(latest) = &status.latest {
            ui.label(format!("Последний релиз: {}", latest.version));
            if !latest.published_at.is_empty() {
                ui.label(theme::muted(format!("Опубликован: {}", latest.published_at)).size(12.5));
            }
            if !latest.url.is_empty() {
                ui.hyperlink_to("Страница релиза", &latest.url);
            }
        }
        ui.add_space(6.0);
        ui.label(RichText::new(state_label(&status)).color(theme::TEXT_BRIGHT));
    });
    ui.add_space(10.0);
    if ui
        .add_enabled(status.check_enabled, egui::Button::new("Проверить снова"))
        .clicked()
    {
        request(bridge, AdminRequest::LoadUpdates(true));
    }
    ui.add_space(10.0);
    card(ui, |ui| {
        ui.label(RichText::new("Как обновить вручную").color(theme::TEXT_BRIGHT));
        let hint = match status.channel.as_str() {
            "prebuilt" => "Установка из готового образа: на сервере выполните ./update.sh.",
            "source" => {
                "Установка из исходников: на сервере выполните git pull и затем ./update.sh."
            }
            _ => {
                "Способ установки неизвестен. Готовый образ: ./update.sh. \
                 Из исходников: git pull, затем ./update.sh."
            }
        };
        ui.label(theme::muted(hint).size(12.5));
    });
}

#[cfg(test)]
mod tests {
    use super::{has_resolution, streaming_patch, toggle_resolution};
    use bs_api::StreamingLimits;

    #[test]
    fn native_stays_last_and_toggles() {
        let mut list = vec![serde_json::json!(720), serde_json::json!("native")];
        toggle_resolution(&mut list, Some(540), true);
        assert_eq!(
            list,
            vec![
                serde_json::json!(540),
                serde_json::json!(720),
                serde_json::json!("native")
            ]
        );
        toggle_resolution(&mut list, None, false);
        assert!(!has_resolution(&list, None));
    }

    #[test]
    fn patch_has_only_changes() {
        let saved = StreamingLimits {
            max_framerate: 60,
            ..StreamingLimits::default()
        };
        let mut edit = saved.clone();
        assert_eq!(
            streaming_patch(&saved, &edit),
            bs_api::StreamingPatch::default()
        );
        edit.max_framerate = 30;
        assert_eq!(streaming_patch(&saved, &edit).max_framerate, Some(30));
    }
}
