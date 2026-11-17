//! Админские разделы «Стриминг», «Телеметрия» и «Обновления».

use bs_api::{
    ApprovalRequest, InstanceSettingsPatch, Peer, StreamingLimits, StreamingPatch, TelemetryStatus,
    UpdateStatus,
};
use egui::RichText;

use super::settings_extra::{Note, fail, number_row, setting_row, show_note};
use crate::bridge::{AdminEvent, AdminRequest, BridgeRequest, PeerOp};
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::screens::dialogs::{card, page_title};
use crate::theme;
use crate::widgets;

/// Разрешения, которые знает сервер.
const RESOLUTIONS: [u32; 5] = [540, 720, 1080, 1440, 2160];
/// Частоты кадров, которые знает сервер.
const FRAMERATES: [u32; 6] = [30, 45, 60, 75, 90, 120];

/// Состояние этих разделов.
#[allow(
    clippy::struct_excessive_bools,
    reason = "флаги «уже запрашивали» по разделам"
)]
#[derive(Debug, Default)]
pub(crate) struct MoreUi {
    streaming: Option<StreamingLimits>,
    edit: Option<StreamingLimits>,
    streaming_asked: bool,
    streaming_note: Option<Note>,
    voice: Option<bs_api::InstanceVoiceBitrate>,
    voice_edit: u32,
    voice_asked: bool,
    voice_note: Option<Note>,
    telemetry: Option<TelemetryStatus>,
    telemetry_asked: bool,
    preview: Option<String>,
    telemetry_note: Option<Note>,
    updates: Option<UpdateStatus>,
    updates_asked: bool,
    updates_note: Option<Note>,
    peers: Option<Vec<Peer>>,
    approvals: Vec<ApprovalRequest>,
    peers_asked: bool,
    peers_note: Option<Note>,
    initiate: String,
    confirm: Option<PeerOp>,
    show_revoked: bool,
    intervals: std::collections::HashMap<String, u32>,
}

impl MoreUi {
    /// Забирает «свои» события; остальные возвращает вызывающему.
    pub(crate) fn accept(&mut self, event: AdminEvent) -> Option<AdminEvent> {
        match event {
            AdminEvent::Peers(result) => match result {
                Ok((peers, approvals)) => {
                    self.peers = Some(peers);
                    self.approvals = approvals;
                }
                Err(message) => self.peers_note = Some(fail(message)),
            },
            AdminEvent::PeerDone(result) => {
                self.peers_note = Some(match result {
                    Ok(text) => super::settings_extra::ok(text),
                    Err(message) => fail(message),
                });
                self.peers_asked = false;
            }
            AdminEvent::Streaming(result) => match result {
                Ok(limits) => {
                    self.edit = Some(limits.clone());
                    self.streaming = Some(limits);
                    self.streaming_note = None;
                }
                Err(message) => self.streaming_note = Some(fail(message)),
            },
            AdminEvent::VoiceBitrate(result) => match result {
                Ok(voice) => {
                    self.voice_edit = voice.kbps;
                    self.voice = Some(voice);
                    self.voice_note = None;
                }
                Err(message) => self.voice_note = Some(fail(message)),
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

/// Допустимые разрешения и частоты следуют из предельных: всё, что не выше
/// предела; «исходное» — только когда предел не ниже 4K (иначе оно бы его
/// обходило). Отдельные галочки не нужны: клиент всё равно упирается в предел.
pub(crate) fn derive_allowed(edit: &mut StreamingLimits) {
    let mut resolutions: Vec<serde_json::Value> = RESOLUTIONS
        .iter()
        .filter(|res| **res <= edit.max_resolution)
        .map(|res| serde_json::json!(res))
        .collect();
    if edit.max_resolution >= RESOLUTIONS[RESOLUTIONS.len() - 1] {
        resolutions.push(serde_json::json!("native"));
    }
    edit.allowed_resolutions = resolutions;
    edit.allowed_framerates = FRAMERATES
        .iter()
        .copied()
        .filter(|fps| *fps <= edit.max_framerate)
        .collect();
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
pub(crate) fn streaming_panel(ui: &mut egui::Ui, more: &mut MoreUi, bridge: &Bridge) {
    page_title(ui, tr("Стриминг"));
    voice_card(ui, more, bridge);
    ui.add_space(10.0);
    if !more.streaming_asked {
        more.streaming_asked = request(bridge, AdminRequest::LoadStreaming);
    }
    let (Some(saved), Some(edit)) = (more.streaming.clone(), more.edit.as_mut()) else {
        show_note(ui, more.streaming_note.as_ref());
        ui.label(theme::muted(tr("Загрузка…")));
        return;
    };
    streaming_limits(ui, edit);
    if (edit.max_resolution, edit.max_framerate) == (saved.max_resolution, saved.max_framerate) {
        edit.allowed_resolutions
            .clone_from(&saved.allowed_resolutions);
        edit.allowed_framerates
            .clone_from(&saved.allowed_framerates);
    } else {
        derive_allowed(edit);
    }
    let patch = streaming_patch(&saved, edit);
    let changed = patch != StreamingPatch::default();
    ui.add_space(14.0);
    show_note(ui, more.streaming_note.as_ref());
    ui.horizontal(|ui| {
        if ui
            .add_enabled(changed, theme::primary_button(tr("Сохранить")))
            .clicked()
        {
            request(bridge, AdminRequest::SaveStreaming(patch));
        }
        if ui
            .add_enabled(changed, egui::Button::new(tr("Сбросить")))
            .clicked()
        {
            more.edit.clone_from(&more.streaming);
        }
    });
}

/// Битрейт голоса в звонках (сервер Rustspace).
fn voice_card(ui: &mut egui::Ui, more: &mut MoreUi, bridge: &Bridge) {
    if !more.voice_asked {
        more.voice_asked = request(bridge, AdminRequest::LoadVoiceBitrate);
    }
    card(ui, |ui| {
        widgets::section_label(ui, tr("Голос в звонках"));
        ui.add_space(4.0);
        let Some(voice) = more.voice.clone() else {
            if more.voice_note.is_some() {
                ui.label(theme::muted(tr(
                    "Сервер не поддерживает настройку битрейта голоса (нужен rustspace-server).",
                )));
            } else {
                ui.label(theme::muted(tr("Загрузка…")));
            }
            return;
        };
        ui.label(theme::muted(tr(
            "С каким битрейтом клиенты отправляют микрофон. Чем выше — тем чище голос, но \
             тем больше трафика. Управляющие голосовым каналом и участники личной переписки \
             могут понизить его у себя, если связь не тянет.",
        )));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().slider_width = 260.0;
            ui.add(
                egui::Slider::new(&mut more.voice_edit, voice.min.max(16)..=voice.max.max(32))
                    .step_by(8.0)
                    .suffix(tr(" кбит/с")),
            );
            if ui
                .add_enabled(
                    more.voice_edit != voice.kbps,
                    theme::primary_button(tr("Сохранить")),
                )
                .clicked()
            {
                request(bridge, AdminRequest::SaveVoiceBitrate(more.voice_edit));
            }
        });
        ui.label(
            theme::muted(tr(
                "Ориентиры: 32 — разборчиво, 64 — как в Discord, 96 — чисто, 128 и выше — музыка.",
            ))
            .size(12.0),
        );
    });
}

/// Битрейт, свой битрейт и предельные разрешение и частота.
fn streaming_limits(ui: &mut egui::Ui, edit: &mut StreamingLimits) {
    card(ui, |ui| {
        number_row(
            ui,
            tr("Максимальный битрейт, кбит/с"),
            tr("Выше этого значения поток не поднимается."),
            &mut edit.max_bitrate_kbps,
            (500, 1_000_000),
        );
        number_row(
            ui,
            tr("Минимальный битрейт, кбит/с"),
            tr("Ниже этого значения поток не опускается."),
            &mut edit.min_bitrate_kbps,
            (100, 1_000_000),
        );
        number_row(
            ui,
            tr("Шаг битрейта, кбит/с"),
            tr("Шаг ползунка качества."),
            &mut edit.bitrate_step_kbps,
            (50, 5_000),
        );
        setting_row(
            ui,
            tr("Свой битрейт"),
            tr("Разрешить пользователям задавать битрейт вручную."),
            &mut edit.allow_custom_bitrate,
        );
        ui.horizontal(|ui| {
            ui.label(tr("Предельное разрешение"));
            combo(ui, "max_res", &mut edit.max_resolution, &RESOLUTIONS, "p");
            ui.add_space(16.0);
            ui.label(tr("Предельная частота кадров"));
            combo(ui, "max_fps", &mut edit.max_framerate, &FRAMERATES, " fps");
        });
    });
}

/// «Телеметрия»: согласие и предпросмотр отправляемого.
pub(crate) fn telemetry_panel(ui: &mut egui::Ui, more: &mut MoreUi, bridge: &Bridge) {
    page_title(ui, tr("Телеметрия"));
    if !more.telemetry_asked {
        more.telemetry_asked = request(bridge, AdminRequest::LoadTelemetry);
    }
    show_note(ui, more.telemetry_note.as_ref());
    let Some(status) = more.telemetry.clone() else {
        ui.label(theme::muted(tr("Загрузка…")));
        return;
    };
    card(ui, |ui| {
        ui.label(tr(
            "Раз в сутки инстанс может отправлять разработчикам обезличенную сводку: \
             версию и обобщённые счётчики. Никаких сообщений и имён.",
        ));
        ui.add_space(8.0);
        let mut on = status.enabled == Some(true);
        let before = on;
        setting_row(
            ui,
            tr("Отправлять телеметрию"),
            match status.enabled {
                None => tr("Выбор ещё не сделан."),
                Some(true) => tr("Включено."),
                Some(false) => tr("Выключено."),
            },
            &mut on,
        );
        if on != before {
            request(bridge, AdminRequest::SetTelemetry(on));
        }
        if let Some(id) = &status.id {
            let masked: String = id.chars().take(8).collect();
            ui.label(
                theme::muted(crate::tf!("Идентификатор: {masked}…", masked = masked)).size(12.5),
            );
        }
        if let Some(day) = &status.last_day {
            ui.label(theme::muted(crate::tf!("Последняя отправка: {day}", day = day)).size(12.5));
        }
    });
    ui.add_space(10.0);
    if ui.button(tr("Показать, что будет отправлено")).clicked() {
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
        "up-to-date" => tr("Установлена последняя версия").to_owned(),
        "update-available" => tr("Доступно обновление").to_owned(),
        _ => match status.reason.as_deref() {
            Some("disabled") => tr("Проверка обновлений отключена на сервере").to_owned(),
            Some("unreachable") => tr("Не удалось связаться с сервером релизов").to_owned(),
            Some("rate-limited") => {
                tr("Сервер релизов ограничил запросы, попробуйте позже").to_owned()
            }
            Some("unparseable") => tr("Не удалось разобрать ответ сервера релизов").to_owned(),
            _ => tr("Состояние неизвестно").to_owned(),
        },
    }
}

/// «Обновления»: версия, последний релиз и способ установки.
pub(crate) fn updates_panel(ui: &mut egui::Ui, more: &mut MoreUi, bridge: &Bridge) {
    page_title(ui, tr("Обновления"));
    if !more.updates_asked {
        more.updates_asked = request(bridge, AdminRequest::LoadUpdates(false));
    }
    show_note(ui, more.updates_note.as_ref());
    let Some(status) = more.updates.clone() else {
        ui.label(theme::muted(tr("Загрузка…")));
        return;
    };
    card(ui, |ui| {
        let commit = status
            .current
            .commit
            .as_deref()
            .map(|c| format!(" ({c})"))
            .unwrap_or_default();
        ui.label(crate::tf!(
            "Установлено: {a0}{commit}",
            a0 = status.current.version,
            commit = commit
        ));
        if let Some(latest) = &status.latest {
            ui.label(crate::tf!("Последний релиз: {a0}", a0 = latest.version));
            if !latest.published_at.is_empty() {
                ui.label(
                    theme::muted(crate::tf!("Опубликован: {a0}", a0 = latest.published_at))
                        .size(12.5),
                );
            }
            if !latest.url.is_empty() {
                ui.hyperlink_to(tr("Страница релиза"), &latest.url);
            }
        }
        ui.add_space(6.0);
        ui.label(RichText::new(state_label(&status)).color(theme::text_bright()));
    });
    ui.add_space(10.0);
    if ui
        .add_enabled(
            status.check_enabled,
            egui::Button::new(tr("Проверить снова")),
        )
        .clicked()
    {
        request(bridge, AdminRequest::LoadUpdates(true));
    }
    ui.add_space(10.0);
    card(ui, |ui| {
        ui.label(RichText::new(tr("Как обновить вручную")).color(theme::text_bright()));
        let hint = match status.channel.as_str() {
            "prebuilt" => tr("Установка из готового образа: на сервере выполните ./update.sh."),
            "source" => {
                tr("Установка из исходников: на сервере выполните git pull и затем ./update.sh.")
            }
            _ => tr("Способ установки неизвестен. Готовый образ: ./update.sh. \
                 Из исходников: git pull, затем ./update.sh."),
        };
        ui.label(theme::muted(hint).size(12.5));
    });
}

fn status_text(peer: &Peer) -> (&'static str, egui::Color32) {
    match peer.status.as_str() {
        "active" => (tr("Активен"), theme::green()),
        "pending" => (tr("Ожидает ответа"), theme::amber()),
        "awaiting_approval" => (tr("Ждёт одобрения"), theme::amber()),
        "unreachable" => (tr("Недоступен"), theme::rose()),
        "needs_attention" => (tr("Требует внимания"), theme::rose()),
        "rejected" => (tr("Отклонён"), theme::rose()),
        "revoked" => (tr("Отозван"), theme::text_faint()),
        _ => (tr("Неизвестно"), theme::text_faint()),
    }
}

fn confirm_text(op: &PeerOp) -> &'static str {
    match op {
        PeerOp::Rotate(_) => tr(
            "Сменить общий секрет с этим инстансом? Старый секрет будет действовать ещё некоторое время.",
        ),
        PeerOp::Revoke(_) => tr("Отозвать пира? Федерация с этим инстансом прекратится."),
        PeerOp::Reset(_) => tr("Сбросить пира? Связь будет установлена заново."),
        PeerOp::Delete(_) => tr("Удалить пира навсегда? Запись исчезнет безвозвратно."),
        _ => tr("Подтвердите действие."),
    }
}

fn peer_card(ui: &mut egui::Ui, more: &mut MoreUi, peer: &Peer, bridge: &Bridge) {
    let revoked = peer.status == "revoked";
    card(ui, |ui| {
        peer_info(ui, peer);
        ui.add_space(6.0);
        if peer_confirm(ui, more, peer, bridge) {
            return;
        }
        peer_actions(ui, more, peer, bridge);
        if !revoked {
            rotation_interval(ui, more, peer, bridge);
        }
    });
    ui.add_space(8.0);
}

/// Имя, адрес, связь и причина статуса пира.
fn peer_info(ui: &mut egui::Ui, peer: &Peer) {
    let (label, color) = status_text(peer);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(peer.instance_name.as_deref().unwrap_or(&peer.origin))
                .color(theme::text_bright()),
        );
        ui.label(RichText::new(label).color(color).size(12.5));
    });
    ui.label(theme::muted(&peer.origin).size(12.5));
    if let Some(seen) = peer.last_seen_at {
        ui.label(
            theme::muted(crate::tf!(
                "Был на связи: {a0}",
                a0 = theme::stamp(seen, chrono::Utc::now().timestamp_millis())
            ))
            .size(12.5),
        );
    }
    if peer.consecutive_failures > 0 || peer.consecutive_auth_failures > 0 {
        ui.label(
            theme::muted(crate::tf!(
                "Подряд неудач: связь — {a0}, аутентификация — {a1}",
                a0 = peer.consecutive_failures,
                a1 = peer.consecutive_auth_failures
            ))
            .size(12.5),
        );
    }
    if let Some(reason) = &peer.status_reason {
        ui.label(theme::muted(crate::tf!("Причина: {reason}", reason = reason)).size(12.5));
    }
}

/// Подтверждение опасного действия над этим пиром; `true` — показано.
fn peer_confirm(ui: &mut egui::Ui, more: &mut MoreUi, peer: &Peer, bridge: &Bridge) -> bool {
    if more.confirm.as_ref().is_some_and(|op| match op {
        PeerOp::Rotate(id) | PeerOp::Revoke(id) | PeerOp::Reset(id) | PeerOp::Delete(id) => {
            *id == peer.id
        }
        _ => false,
    }) {
        let op = more.confirm.clone();
        if let Some(op) = op {
            ui.label(RichText::new(confirm_text(&op)).color(theme::amber()));
            ui.horizontal(|ui| {
                if ui.button(tr("Подтвердить")).clicked() {
                    request(bridge, AdminRequest::Peer(op));
                    more.confirm = None;
                }
                if ui.button(tr("Отмена")).clicked() {
                    more.confirm = None;
                }
            });
        }
        return true;
    }
    false
}

/// Кнопки пира: переподключить, перепроверить, сменить секрет, сбросить, отозвать.
fn peer_actions(ui: &mut egui::Ui, more: &mut MoreUi, peer: &Peer, bridge: &Bridge) {
    let revoked = peer.status == "revoked";
    ui.horizontal_wrapped(|ui| {
        if revoked {
            if ui.button(tr("Подключить снова")).clicked() {
                request(
                    bridge,
                    AdminRequest::Peer(PeerOp::Initiate(peer.origin.clone())),
                );
            }
            if ui.button(tr("Удалить навсегда")).clicked() {
                more.confirm = Some(PeerOp::Delete(peer.id.clone()));
            }
            return;
        }
        if matches!(peer.status.as_str(), "unreachable" | "needs_attention")
            && ui.button(tr("Перепроверить")).clicked()
        {
            request(bridge, AdminRequest::Peer(PeerOp::Recheck(peer.id.clone())));
        }
        if peer.status == "active"
            && !peer.rotation_in_progress
            && ui.button(tr("Сменить секрет")).clicked()
        {
            more.confirm = Some(PeerOp::Rotate(peer.id.clone()));
        }
        if ui.button(tr("Сбросить")).clicked() {
            more.confirm = Some(PeerOp::Reset(peer.id.clone()));
        }
        if ui.button(tr("Отозвать")).clicked() {
            more.confirm = Some(PeerOp::Revoke(peer.id.clone()));
        }
    });
}

/// Как часто менять секрет этого пира.
fn rotation_interval(ui: &mut egui::Ui, more: &mut MoreUi, peer: &Peer, bridge: &Bridge) {
    ui.horizontal(|ui| {
        ui.label(theme::muted(tr("Ротация секрета каждые, дней")).size(12.5));
        let days = more
            .intervals
            .entry(peer.id.clone())
            .or_insert(peer.auto_rotate_interval_days);
        ui.add(egui::DragValue::new(days).range(1..=365));
        if *days != peer.auto_rotate_interval_days && ui.button(tr("Применить")).clicked()
        {
            request(
                bridge,
                AdminRequest::Peer(PeerOp::Interval(peer.id.clone(), *days)),
            );
        }
    });
}

/// Глобальные параметры федерации (живут в общих настройках инстанса).
fn federation_globals(
    ui: &mut egui::Ui,
    admin: &mut super::settings_extra::AdminUi,
    bridge: &Bridge,
) {
    let Some(edit) = admin.edit_mut() else {
        return;
    };
    widgets::section_label(ui, tr("Глобальные параметры"));
    card(ui, |ui| {
        setting_row(
            ui,
            tr("Автоматически принимать пиринг"),
            tr("Запросы от других инстансов принимаются без ручного одобрения."),
            &mut edit.auto_accept_peering,
        );
        setting_row(
            ui,
            tr("Ретрансляция сообщений"),
            tr("Хранить сообщения для недоступных пиров и доставить позже."),
            &mut edit.federation_relay_enabled,
        );
        number_row(
            ui,
            tr("Хранение ретрансляции, дней"),
            tr("Сколько дней ждать недоступного пира."),
            &mut edit.federation_relay_ttl_days,
            (1, 365),
        );
        number_row(
            ui,
            tr("Ротация секрета по умолчанию, дней"),
            tr("Для новых пиров."),
            &mut edit.default_auto_rotate_interval_days,
            (1, 365),
        );
    });
    let patch = admin.settings_patch_only();
    let changed = patch != InstanceSettingsPatch::default();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(changed, theme::primary_button(tr("Сохранить")))
            .clicked()
        {
            bridge.request(BridgeRequest::Admin(AdminRequest::SaveSettings(patch)));
        }
        if ui
            .add_enabled(changed, egui::Button::new(tr("Сбросить")))
            .clicked()
        {
            admin.reset_edit();
        }
    });
    ui.add_space(12.0);
}

/// Запросы на пиринг, ждущие решения.
fn approvals(ui: &mut egui::Ui, more: &MoreUi, bridge: &Bridge) {
    if more.approvals.is_empty() {
        return;
    }
    widgets::section_label(ui, tr("Запросы на одобрение"));
    for req in more.approvals.clone() {
        card(ui, |ui| {
            let dir = if req.direction == "inbound" {
                tr("Входящий запрос")
            } else {
                tr("Исходящий запрос")
            };
            ui.label(
                RichText::new(req.instance_name.as_deref().unwrap_or(&req.origin))
                    .color(theme::text_bright()),
            );
            ui.label(theme::muted(format!("{dir} · {}", req.origin)).size(12.5));
            ui.horizontal(|ui| {
                if ui.button(tr("Одобрить")).clicked() {
                    request(
                        bridge,
                        AdminRequest::Peer(PeerOp::Answer(req.id.clone(), true)),
                    );
                }
                if ui.button(tr("Отклонить")).clicked() {
                    request(
                        bridge,
                        AdminRequest::Peer(PeerOp::Answer(req.id.clone(), false)),
                    );
                }
            });
        });
        ui.add_space(8.0);
    }
}

/// «Федерация»: пиры, запросы на одобрение и глобальные параметры.
pub(crate) fn federation_panel(
    ui: &mut egui::Ui,
    extra: &mut super::settings_extra::ExtraUi,
    bridge: &Bridge,
) {
    let admin = &mut extra.admin;
    page_title(ui, tr("Федерация"));
    if !admin.more.peers_asked {
        admin.more.peers_asked = request(bridge, AdminRequest::LoadPeers);
    }
    // Глобальные параметры живут в общих настройках инстанса.
    super::settings_extra::ensure_settings(admin, bridge);
    federation_globals(ui, admin, bridge);
    let more = &mut admin.more;
    widgets::section_label(ui, tr("Подключить инстанс"));
    card(ui, |ui| {
        ui.horizontal(|ui| {
            widgets::text_field(ui, &mut more.initiate, "https://example.org", 200);
            let origin = more.initiate.trim().to_owned();
            if ui
                .add_enabled(
                    !origin.is_empty(),
                    theme::primary_button(tr("Запросить пиринг")),
                )
                .clicked()
            {
                request(bridge, AdminRequest::Peer(PeerOp::Initiate(origin)));
                more.initiate.clear();
            }
        });
    });
    ui.add_space(8.0);
    show_note(ui, more.peers_note.as_ref());
    approvals(ui, more, bridge);
    let Some(peers) = more.peers.clone() else {
        ui.label(theme::muted(tr("Загрузка…")));
        return;
    };
    ui.horizontal(|ui| {
        ui.selectable_value(&mut more.show_revoked, false, tr("Активные"));
        ui.selectable_value(&mut more.show_revoked, true, tr("Отозванные"));
    });
    ui.add_space(6.0);
    let mut shown: Vec<&Peer> = peers
        .iter()
        .filter(|p| (p.status == "revoked") == more.show_revoked)
        .collect();
    shown.sort_by_key(|a| a.origin.to_lowercase());
    if shown.is_empty() {
        theme::empty_state(
            ui,
            if more.show_revoked {
                tr("Отозванных пиров нет")
            } else {
                tr("Пока нет ни одного пира")
            },
        );
    }
    for peer in shown {
        peer_card(ui, more, peer, bridge);
    }
}

#[cfg(test)]
mod tests {
    use super::{derive_allowed, streaming_patch};
    use bs_api::StreamingLimits;

    #[test]
    fn allowed_follow_the_limits() {
        let mut edit = StreamingLimits {
            max_resolution: 1080,
            max_framerate: 60,
            ..StreamingLimits::default()
        };
        derive_allowed(&mut edit);
        assert_eq!(
            edit.allowed_resolutions,
            vec![
                serde_json::json!(540),
                serde_json::json!(720),
                serde_json::json!(1080)
            ]
        );
        assert_eq!(edit.allowed_framerates, vec![30, 45, 60]);
        edit.max_resolution = 2160;
        derive_allowed(&mut edit);
        assert_eq!(
            edit.allowed_resolutions.last(),
            Some(&serde_json::json!("native"))
        );
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
