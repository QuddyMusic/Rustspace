//! Страница «Голос и видео»: устройства, громкость, обработка, камера.

#![allow(
    clippy::wildcard_imports,
    reason = "части одного модуля, разнесённые по файлам"
)]

use super::*;

/// «Голос и видео»: устройства и обработка звука.
pub(super) fn voice_settings(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    bridge: &Bridge,
    voice: &mut crate::voice::VoiceState,
) {
    use crate::voice_media::{DeviceKind, MediaEngine};
    page_title(ui, tr("Голос и видео"));
    if !MediaEngine::available() {
        card(ui, |ui| {
            ui.label(tr(
                "Эта сборка клиента без LiveKit: присутствие в голосовых каналах работает, \
                 а звук — нет. Соберите с флагом `--features voice`.",
            ));
        });
        ui.add_space(10.0);
    }
    if state.voice_devices.is_none() {
        let prefs = voice.prefs.clone();
        let media = voice.media(bridge);
        let inputs = media.devices(DeviceKind::Input, &prefs);
        let outputs = media.devices(DeviceKind::Output, &prefs);
        state.voice_devices = Some((inputs, outputs));
    }
    let mut changed = false;
    let mut refresh = false;
    if let Some((inputs, outputs)) = state.voice_devices.as_ref() {
        card(ui, |ui| {
            changed |= device_picker(
                ui,
                tr("Микрофон"),
                "voice_input",
                inputs,
                &mut voice.prefs.input,
            );
            changed |= volume_slider(ui, tr("Громкость микрофона"), &mut voice.prefs.input_volume);
            ui.add_space(8.0);
            changed |= device_picker(
                ui,
                tr("Устройство воспроизведения"),
                "voice_output",
                outputs,
                &mut voice.prefs.output,
            );
            changed |= volume_slider(ui, tr("Громкость звука"), &mut voice.prefs.output_volume);
            ui.add_space(8.0);
            changed |= camera_picker(ui, voice);
            ui.add_space(8.0);
            if ui.button(tr("Обновить список устройств")).clicked() {
                refresh = true;
                voice.stage.cameras = None;
            }
        });
    }
    if refresh {
        state.voice_devices = None;
    }
    ui.add_space(10.0);
    card(ui, |ui| {
        let prefs = &mut voice.prefs;
        for (title, hint, flag) in [
            (
                tr("Эхоподавление"),
                tr("Убирает эхо ваших колонок из микрофона."),
                &mut prefs.echo_cancellation,
            ),
            (
                tr("Шумоподавление"),
                tr("Приглушает фоновый шум."),
                &mut prefs.noise_suppression,
            ),
            (
                tr("Автоусиление микрофона"),
                tr("Выравнивает громкость вашего голоса."),
                &mut prefs.auto_gain_control,
            ),
        ] {
            let before = *flag;
            crate::screens::settings_extra::setting_row(ui, title, hint, flag);
            changed |= before != *flag;
        }
    });
    ui.add_space(10.0);
    changed |= attenuation_card(ui, &mut voice.prefs);
    if changed {
        let prefs = voice.prefs.clone();
        voice.media(bridge).apply_prefs(&prefs);
        crate::prefs::update(|saved| saved.voice = prefs);
    }
    ui.add_space(10.0);
    mic_test_card(ui, state, bridge, voice);
    ui.add_space(10.0);
    crate::screens::settings_app::sounds_card(ui);
}

/// Выбор камеры; список ищется один раз (и по «Обновить»).
pub(super) fn camera_picker(ui: &mut egui::Ui, voice: &mut crate::voice::VoiceState) -> bool {
    let cameras = voice
        .stage
        .cameras
        .get_or_insert_with(crate::video_media::VideoControl::cameras);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(tr("Камера"));
        match cameras {
            Err(err) => {
                ui.label(theme::muted(err.as_str()));
            }
            Ok(list) if list.is_empty() => {
                ui.label(theme::muted(tr("камер не найдено")));
            }
            Ok(list) => {
                let current = voice.prefs.camera.unwrap_or(0);
                let name = list
                    .iter()
                    .find(|c| c.index == current)
                    .map_or_else(|| tr("Первая").to_owned(), |c| c.name.clone());
                egui::ComboBox::from_id_salt("camera_device")
                    .selected_text(name)
                    .show_ui(ui, |ui| {
                        for camera in list.iter() {
                            if ui
                                .selectable_label(current == camera.index, &camera.name)
                                .clicked()
                            {
                                voice.prefs.camera = Some(camera.index);
                                changed = true;
                            }
                        }
                    });
            }
        }
    });
    changed
}

/// Приглушение трансляций, пока кто-то говорит.
pub(super) fn attenuation_card(
    ui: &mut egui::Ui,
    prefs: &mut crate::voice_media::VoicePrefs,
) -> bool {
    let mut changed = false;
    card(ui, |ui| {
        let before = prefs.stream_attenuation;
        crate::screens::settings_extra::setting_row(
            ui,
            tr("Приглушать трансляции"),
            tr("Пока кто-то говорит, звук демонстраций экрана становится тише."),
            &mut prefs.stream_attenuation,
        );
        changed |= before != prefs.stream_attenuation;
        if prefs.stream_attenuation {
            ui.horizontal(|ui| {
                ui.label(tr("Насколько"));
                changed |= ui
                    .add(egui::Slider::new(&mut prefs.attenuation_strength, 0..=100).suffix(" %"))
                    .changed();
            });
        }
    });
    changed
}

/// Ползунок громкости 0–200 %.
pub(super) fn volume_slider(ui: &mut egui::Ui, title: &str, value: &mut u16) -> bool {
    ui.horizontal(|ui| {
        ui.label(title);
        ui.add(egui::Slider::new(value, 0..=200).suffix(" %"))
            .changed()
    })
    .inner
}

/// Сколько длится проверка микрофона.
pub(super) const MIC_TEST_SECONDS: f64 = 15.0;

/// «Проверка микрофона»: индикатор уровня сигнала без звонка.
pub(super) fn mic_test_card(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    bridge: &Bridge,
    voice: &mut crate::voice::VoiceState,
) {
    let now = ui.input(|i| i.time);
    card(ui, |ui| {
        widgets::section_label(ui, tr("Проверка микрофона"));
        ui.add_space(6.0);
        let running = state.mic_test_until.is_some_and(|until| now < until);
        if !running && state.mic_test_until.take().is_some() {
            voice.media(bridge).stop_mic_test();
        }
        let in_call = voice.call.is_some() || voice.dm.is_some();
        ui.horizontal(|ui| {
            let label = if running {
                tr("Остановить")
            } else {
                tr("Проверить микрофон")
            };
            if ui
                .add_enabled(!in_call, egui::Button::new(label))
                .on_disabled_hover_text(tr("Во время звонка проверка недоступна"))
                .clicked()
            {
                if running {
                    state.mic_test_until = None;
                    voice.media(bridge).stop_mic_test();
                } else {
                    let prefs = voice.prefs.clone();
                    match voice.media(bridge).start_mic_test(&prefs) {
                        Ok(()) => {
                            state.mic_test_until = Some(now + MIC_TEST_SECONDS);
                            state.mic_test_error = None;
                        }
                        Err(err) => state.mic_test_error = Some(err),
                    }
                }
            }
            let level = voice.media(bridge).mic_level().unwrap_or(0.0);
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width().min(320.0), 10.0),
                egui::Sense::hover(),
            );
            ui.painter()
                .rect_filled(rect, crate::theme::rounded(5), theme::bg_input());
            let mut filled = rect;
            filled.set_width(rect.width() * level);
            let color = if level > 0.9 {
                theme::rose()
            } else if level > 0.6 {
                theme::amber()
            } else {
                theme::green()
            };
            ui.painter()
                .rect_filled(filled, crate::theme::rounded(5), color);
            if running {
                ui.ctx().request_repaint();
            }
        });
        ui.add_space(4.0);
        let hint = state.mic_test_error.as_deref().map_or_else(
            || {
                theme::muted(tr(
                    "Скажите что-нибудь: полоса покажет уровень сигнала выбранного микрофона.",
                ))
                .size(12.5)
            },
            |err| {
                RichText::new(crate::tf!("Микрофон не открылся: {err}", err = err))
                    .size(12.5)
                    .color(theme::rose())
            },
        );
        ui.label(hint);
    });
}

/// Выпадающий список устройств; `None` — системное по умолчанию.
pub(super) fn device_picker(
    ui: &mut egui::Ui,
    title: &str,
    id: &str,
    devices: &[crate::voice_media::Device],
    selected: &mut Option<String>,
) -> bool {
    let before = selected.clone();
    widgets::field_label(ui, title);
    let current = selected
        .as_ref()
        .and_then(|id| devices.iter().find(|d| &d.id == id))
        .map_or_else(|| tr("Системное по умолчанию"), |d| d.name.as_str());
    egui::ComboBox::from_id_salt(id)
        .selected_text(theme::truncate_chars(current, 40))
        .width(320.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(selected, None, tr("Системное по умолчанию"));
            for device in devices {
                ui.selectable_value(selected, Some(device.id.clone()), &device.name);
            }
        });
    *selected != before
}
