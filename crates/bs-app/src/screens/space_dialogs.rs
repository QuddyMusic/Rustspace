//! Окна над пространством: вход по коду, создание канала и категории, выход.
//!
//! Тексты и поля — как в `CreateChannel`, `CreateCategory`, `JoinSpace` и
//! подтверждении выхода оригинального веб-клиента.

use egui::{Align, CornerRadius, Layout, Margin, RichText};

use crate::bridge::{BridgeRequest, SpaceEvent};
use crate::fleet::Bridge;
use crate::{theme, widgets};

const CHANNEL_NAME_MAX: usize = 100;
const CHANNEL_TOPIC_MAX: usize = 1024;
const CATEGORY_NAME_MAX: usize = 100;

/// Открытое окно.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SpaceDialog {
    /// Вступить по коду или ссылке.
    Join(JoinForm),
    /// Создать канал.
    Channel(ChannelForm),
    /// Создать категорию.
    Category(CategoryForm),
    /// Подтвердить выход.
    Leave(LeaveForm),
}

/// Поля окна входа.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct JoinForm {
    input: String,
    error: Option<String>,
    busy: bool,
}

/// Поля окна создания канала.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChannelForm {
    origin: usize,
    space_id: String,
    name: String,
    topic: String,
    voice: bool,
    category: Option<String>,
    categories: Vec<(String, String)>,
    error: Option<String>,
    busy: bool,
}

/// Поля окна создания категории.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CategoryForm {
    origin: usize,
    space_id: String,
    name: String,
    error: Option<String>,
    busy: bool,
}

/// Окно подтверждения выхода.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LeaveForm {
    origin: usize,
    space_id: String,
    space_name: String,
    error: Option<String>,
    busy: bool,
}

impl SpaceDialog {
    /// Окно входа по коду.
    pub(crate) fn join() -> Self {
        Self::Join(JoinForm::default())
    }

    /// Окно нового канала; `category` — уже выбранная категория.
    pub(crate) fn channel(
        origin: usize,
        space_id: &str,
        categories: Vec<(String, String)>,
        category: Option<String>,
    ) -> Self {
        Self::Channel(ChannelForm {
            origin,
            space_id: space_id.to_owned(),
            name: String::new(),
            topic: String::new(),
            voice: false,
            category,
            categories,
            error: None,
            busy: false,
        })
    }

    /// Окно новой категории.
    pub(crate) fn category(origin: usize, space_id: &str) -> Self {
        Self::Category(CategoryForm {
            origin,
            space_id: space_id.to_owned(),
            name: String::new(),
            error: None,
            busy: false,
        })
    }

    /// Окно подтверждения выхода.
    pub(crate) fn leave(origin: usize, space_id: &str, space_name: &str) -> Self {
        Self::Leave(LeaveForm {
            origin,
            space_id: space_id.to_owned(),
            space_name: space_name.to_owned(),
            error: None,
            busy: false,
        })
    }

    /// Учитывает итог действия. Возвращает `false`, если окно надо закрыть.
    pub(crate) fn apply(&mut self, event: &SpaceEvent) -> bool {
        match (self, event) {
            (Self::Channel(form), SpaceEvent::Created(result)) => {
                form.busy = false;
                match result {
                    Ok(()) => return false,
                    Err(err) => form.error = Some(err.clone()),
                }
            }
            (Self::Category(form), SpaceEvent::Created(result)) => {
                form.busy = false;
                match result {
                    Ok(()) => return false,
                    Err(err) => form.error = Some(err.clone()),
                }
            }
            (Self::Join(form), SpaceEvent::JoinFailed(err)) => {
                form.busy = false;
                form.error = Some(err.clone());
            }
            (Self::Leave(form), SpaceEvent::Left { space_id, result })
                if *space_id == form.space_id =>
            {
                form.busy = false;
                match result {
                    Ok(()) => return false,
                    Err(err) => form.error = Some(err.clone()),
                }
            }
            _ => {}
        }
        true
    }
}

/// Рамка модального окна с заголовком и крестиком; `body` рисует содержимое.
fn modal(
    ctx: &egui::Context,
    id: &'static str,
    title: &str,
    body: impl FnOnce(&mut egui::Ui),
) -> bool {
    let mut keep_open = true;
    let response = egui::Modal::new(egui::Id::unique(id))
        .frame(
            egui::Frame::new()
                .fill(theme::BG_BUBBLE)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(16))
                .inner_margin(Margin::same(22)),
        )
        .show(ctx, |ui| {
            ui.set_width(400.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(title)
                        .size(19.0)
                        .family(theme::bold_family())
                        .color(theme::TEXT_BRIGHT),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, crate::icons::Icon::Close, 30.0).clicked() {
                        keep_open = false;
                    }
                });
            });
            body(ui);
        });
    keep_open && !response.should_close()
}

fn error_line(ui: &mut egui::Ui, error: Option<&String>) {
    if let Some(err) = error {
        ui.add_space(6.0);
        ui.label(RichText::new(err).color(theme::ROSE));
    }
}

/// Показывает окно. Возвращает `false`, когда его надо закрыть.
pub(crate) fn show(ctx: &egui::Context, dialog: &mut SpaceDialog, bridge: &mut Bridge) -> bool {
    match dialog {
        SpaceDialog::Join(form) => show_join(ctx, form, bridge),
        SpaceDialog::Channel(form) => show_channel(ctx, form, bridge),
        SpaceDialog::Category(form) => show_category(ctx, form, bridge),
        SpaceDialog::Leave(form) => show_leave(ctx, form, bridge),
    }
}

/// Куда отправить вход: инстанс, чей адрес указан в ссылке, иначе текущий.
fn join_target(bridge: &Bridge, origin: Option<&str>) -> Result<usize, String> {
    let Some(origin) = origin else {
        return Ok(bridge.focus_index());
    };
    bridge
        .live_origins()
        .into_iter()
        .find(|&i| {
            bridge
                .base_at(i)
                .is_some_and(|base| base.trim_end_matches('/') == origin)
        })
        .ok_or_else(|| {
            format!("Сервер {origin} не подключён: добавьте его в Настройки → Подключения")
        })
}

fn show_join(ctx: &egui::Context, form: &mut JoinForm, bridge: &mut Bridge) -> bool {
    modal(
        ctx,
        "join_space",
        "Присоединиться к пространству",
        |ui| {
            ui.add_space(6.0);
            ui.label(
                RichText::new("Есть код приглашения?")
                    .size(13.0)
                    .color(theme::TEXT_MUTED),
            );
            widgets::field_label(ui, "Код или ссылка");
            let field = widgets::text_field(
                ui,
                &mut form.input,
                "abc123 или https://сервер/join/abc123",
                200,
            );
            error_line(ui, form.error.as_ref());
            ui.add_space(14.0);
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let go = ui.add_enabled(
                    !form.busy && !form.input.trim().is_empty(),
                    theme::primary_button(if form.busy {
                        "Присоединение…"
                    } else {
                        "Присоединиться"
                    }),
                );
                if (go.clicked() || enter) && !form.busy && !form.input.trim().is_empty() {
                    match bs_api::parse_invite_input(&form.input).and_then(|(code, origin)| {
                        join_target(bridge, origin.as_deref()).map(|index| (code, index))
                    }) {
                        Ok((code, index)) => {
                            form.error = None;
                            form.busy = true;
                            bridge.focus_on(index);
                            bridge.request(BridgeRequest::JoinByCode(code));
                        }
                        Err(err) => form.error = Some(err),
                    }
                }
            });
        },
    )
}

fn show_channel(ctx: &egui::Context, form: &mut ChannelForm, bridge: &mut Bridge) -> bool {
    modal(ctx, "create_channel", "Создать канал", |ui| {
        widgets::field_label(ui, "Тип канала");
        if widgets::radio_card(
            ui,
            "Текстовый",
            "Отправляйте сообщения, изображения и файлы",
            !form.voice,
        )
        .clicked()
        {
            form.voice = false;
        }
        if widgets::radio_card(ui, "Голосовой", "Общайтесь голосом и по видео", form.voice)
            .clicked()
        {
            form.voice = true;
        }
        widgets::field_label(ui, "Название канала");
        let field = widgets::text_field(ui, &mut form.name, "новый-канал", CHANNEL_NAME_MAX);
        if !form.voice {
            widgets::field_label(ui, "Тема (необязательно)");
            widgets::text_field(ui, &mut form.topic, "О чём этот канал?", CHANNEL_TOPIC_MAX);
        }
        if !form.categories.is_empty() {
            widgets::field_label(ui, "Категория");
            let current = form
                .categories
                .iter()
                .find(|(id, _)| Some(id) == form.category.as_ref())
                .map_or("Без категории", |(_, name)| name.as_str())
                .to_owned();
            egui::ComboBox::from_id_salt("channel_category")
                .selected_text(current)
                .width(ui.available_width() - 8.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut form.category, None, "Без категории");
                    for (id, name) in &form.categories {
                        ui.selectable_value(&mut form.category, Some(id.clone()), name);
                    }
                });
        }
        error_line(ui, form.error.as_ref());
        ui.add_space(14.0);
        let ready = !form.name.trim().is_empty() && !form.busy;
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let go = ui.add_enabled(
                ready,
                theme::primary_button(if form.busy {
                    "Создание…"
                } else {
                    "Создать канал"
                }),
            );
            if (go.clicked() || enter) && ready {
                let topic = form.topic.trim();
                form.busy = true;
                form.error = None;
                bridge.focus_on(form.origin);
                bridge.request(BridgeRequest::CreateChannel {
                    space_id: form.space_id.clone(),
                    channel: bs_api::NewChannel {
                        name: form.name.trim().to_owned(),
                        kind: if form.voice { "voice" } else { "text" }.to_owned(),
                        topic: (!form.voice && !topic.is_empty()).then(|| topic.to_owned()),
                        category_id: form.category.clone(),
                    },
                });
            }
        });
    })
}

fn show_category(ctx: &egui::Context, form: &mut CategoryForm, bridge: &mut Bridge) -> bool {
    modal(
        ctx,
        "create_category",
        "Создать категорию",
        |ui| {
            widgets::field_label(ui, "Название категории");
            let field =
                widgets::text_field(ui, &mut form.name, "новая-категория", CATEGORY_NAME_MAX);
            error_line(ui, form.error.as_ref());
            ui.add_space(14.0);
            let ready = !form.name.trim().is_empty() && !form.busy;
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let go = ui.add_enabled(
                    ready,
                    theme::primary_button(if form.busy {
                        "Создание…"
                    } else {
                        "Создать категорию"
                    }),
                );
                if (go.clicked() || enter) && ready {
                    form.busy = true;
                    form.error = None;
                    bridge.focus_on(form.origin);
                    bridge.request(BridgeRequest::CreateCategory {
                        space_id: form.space_id.clone(),
                        name: form.name.trim().to_owned(),
                    });
                }
            });
        },
    )
}

fn show_leave(ctx: &egui::Context, form: &mut LeaveForm, bridge: &mut Bridge) -> bool {
    let title = format!("Покинуть {}", form.space_name);
    let mut cancel = false;
    let open = modal(ctx, "leave_space", &title, |ui| {
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Вы уверены, что хотите покинуть это пространство? Чтобы снова \
                 присоединиться, вам понадобится новое приглашение.",
            )
            .color(theme::TEXT_MUTED),
        );
        error_line(ui, form.error.as_ref());
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let leave = ui.add_enabled(
                !form.busy,
                egui::Button::new(
                    RichText::new(if form.busy {
                        "Выходим…"
                    } else {
                        "Покинуть пространство"
                    })
                    .color(theme::BG_RAIL),
                )
                .fill(theme::ROSE)
                .corner_radius(CornerRadius::same(8)),
            );
            if leave.clicked() {
                form.busy = true;
                form.error = None;
                bridge.focus_on(form.origin);
                let user_id = bridge
                    .cache_at(form.origin)
                    .map(|cache| cache.me().id.clone())
                    .unwrap_or_default();
                bridge.request(BridgeRequest::LeaveSpace {
                    space_id: form.space_id.clone(),
                    user_id,
                });
            }
            if ui.button("Отмена").clicked() {
                cancel = true;
            }
        });
    });
    open && !cancel
}

#[cfg(test)]
mod tests {
    use super::SpaceDialog;
    use crate::bridge::SpaceEvent;

    #[test]
    fn created_ok_closes_and_error_stays_open() {
        let mut dialog = SpaceDialog::category(0, "s1");
        assert!(!dialog.apply(&SpaceEvent::Created(Ok(()))));
        let mut dialog = SpaceDialog::category(0, "s1");
        assert!(dialog.apply(&SpaceEvent::Created(Err("нет права".to_owned()))));
    }

    #[test]
    fn leave_ignores_other_spaces() {
        let mut dialog = SpaceDialog::leave(0, "s1", "Игры");
        let other = SpaceEvent::Left {
            space_id: "s2".to_owned(),
            result: Ok(()),
        };
        assert!(dialog.apply(&other));
        let mine = SpaceEvent::Left {
            space_id: "s1".to_owned(),
            result: Ok(()),
        };
        assert!(!dialog.apply(&mine));
    }

    #[test]
    fn join_failure_keeps_form_with_error() {
        let mut dialog = SpaceDialog::join();
        assert!(dialog.apply(&SpaceEvent::JoinFailed("плохой код".to_owned())));
        let SpaceDialog::Join(form) = &dialog else {
            unreachable!("окно входа");
        };
        assert_eq!(form.error.as_deref(), Some("плохой код"));
        assert!(!form.busy);
    }
}
