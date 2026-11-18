//! Настройки → «Боты»: свои боты, их токены и пространства.

use bs_api::Bot;
use egui::RichText;

use super::dialogs::{card, page_title};
use super::settings_extra::{Note, fail, ok, show_note};
use crate::bridge::{AdminRequest, BotEvent, BotOp, BridgeRequest};
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::{theme, widgets};

/// Форма имени и описания: новый бот (`id: None`) или правка.
#[derive(Debug, Default)]
struct Form {
    id: Option<String>,
    username: String,
    display_name: String,
    description: String,
}

/// Что ждёт подтверждения.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Confirm {
    Delete(String),
    Regenerate(String),
}

/// Состояние раздела.
#[derive(Debug, Default)]
pub(crate) struct BotsUi {
    asked: bool,
    loaded: Option<Result<Vec<Bot>, Option<String>>>,
    busy: bool,
    note: Option<Note>,
    form: Option<Form>,
    /// Только что выданный токен: (бот, токен). Показывается один раз.
    token: Option<(String, String)>,
    confirm: Option<Confirm>,
    /// Бот, у которого открыт выбор пространства.
    picking: Option<String>,
}

fn send(bridge: &Bridge, op: BotOp) {
    bridge.request(BridgeRequest::Admin(AdminRequest::Bots(op)));
}

impl BotsUi {
    /// Ответ моста.
    pub(crate) fn apply(&mut self, event: BotEvent) {
        self.busy = false;
        match event {
            BotEvent::Loaded(result) => self.loaded = Some(result),
            BotEvent::Done { bots, token } => {
                if let Some(bots) = bots {
                    self.loaded = Some(Ok(bots));
                }
                self.form = None;
                self.picking = None;
                self.note = Some(ok(if token.is_some() {
                    tr("Готово — скопируйте токен")
                } else {
                    tr("Сохранено")
                }));
                if token.is_some() {
                    self.token = token;
                }
            }
            BotEvent::Failed(message) => self.note = Some(fail(message)),
        }
    }

    fn act(&mut self, bridge: &Bridge, op: BotOp) {
        self.busy = true;
        self.note = None;
        send(bridge, op);
    }
}

/// Раздел «Боты».
pub(crate) fn page(ui: &mut egui::Ui, state: &mut BotsUi, bridge: &Bridge) {
    page_title(ui, tr("Боты"));
    ui.label(theme::muted(tr(
        "Бот — отдельный участник, за которого пишет ваша программа. Создайте бота, добавьте его \
         в пространство и запустите программу с его токеном.",
    )));
    ui.add_space(10.0);
    if !state.asked {
        state.asked = true;
        send(bridge, BotOp::Load);
    }
    let bots = match &state.loaded {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::muted(tr("Загрузка…")));
            });
            return;
        }
        Some(Err(None)) => {
            card(ui, |ui| {
                ui.label(theme::muted(tr(
                    "Этот сервер не поддерживает ботов: нужен сервер Rustspace свежей версии.",
                )));
            });
            return;
        }
        Some(Err(Some(message))) => {
            ui.label(RichText::new(message).color(theme::rose()));
            if ui.button(tr("Повторить")).clicked() {
                state.asked = false;
            }
            return;
        }
        Some(Ok(bots)) => bots.clone(),
    };
    show_note(ui, state.note.as_ref());
    token_card(ui, state, &bots);
    for bot in &bots {
        bot_card(ui, state, bridge, bot);
        ui.add_space(8.0);
    }
    if bots.is_empty() && state.form.is_none() {
        card(ui, |ui| {
            ui.label(theme::muted(tr("Ботов пока нет.")));
        });
        ui.add_space(8.0);
    }
    if state.form.as_ref().is_some_and(|f| f.id.is_none()) {
        form(ui, state, bridge);
    } else if ui
        .add_enabled(
            state.form.is_none(),
            theme::primary_button(tr("Создать бота")),
        )
        .clicked()
    {
        state.form = Some(Form::default());
        state.note = None;
    }
    ui.add_space(12.0);
    ui.label(
        theme::muted(tr(
            "Бот входит токеном и видит то же, что участник: HTTP API и события по WebSocket. \
             Для Rust есть библиотека bs-bot; на JavaScript, TypeScript и других языках ботов \
             пишут поверх того же API — см. docs/bots.ru.md.",
        ))
        .size(12.0),
    );
}

/// Только что выданный токен.
fn token_card(ui: &mut egui::Ui, state: &mut BotsUi, bots: &[Bot]) {
    let Some((bot_id, token)) = state.token.clone() else {
        return;
    };
    let name = bots
        .iter()
        .find(|b| b.id == bot_id)
        .map_or(bot_id.as_str(), |b| b.username.as_str());
    card(ui, |ui| {
        widgets::section_label(ui, &crate::tf!("Токен бота {a0}", a0 = name));
        ui.label(
            theme::muted(tr(
                "Сохраните его сейчас: больше он не покажется. Кто знает токен, пишет от имени бота.",
            ))
            .size(12.5),
        );
        ui.add_space(6.0);
        let mut shown = token.clone();
        ui.add(
            egui::TextEdit::singleline(&mut shown)
                .font(egui::TextStyle::Monospace)
                .desired_width(ui.available_width()),
        );
        ui.horizontal(|ui| {
            if ui.add(theme::primary_button(tr("Скопировать"))).clicked() {
                ui.ctx().copy_text(token);
            }
            if ui.button(tr("Готово")).clicked() {
                state.token = None;
            }
        });
    });
    ui.add_space(8.0);
}

fn bot_card(ui: &mut egui::Ui, state: &mut BotsUi, bridge: &Bridge, bot: &Bot) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(bot.display_name.as_deref().unwrap_or(&bot.username))
                            .family(theme::bold_family())
                            .color(theme::text_bright()),
                    );
                    theme::bot_badge(ui);
                    ui.label(theme::muted(format!("@{}", bot.username)).size(12.5));
                });
                if let Some(description) = &bot.description {
                    ui.label(RichText::new(description).color(theme::text()).size(13.0));
                }
                let spaces = if bot.spaces.is_empty() {
                    tr("Пока ни в одном пространстве").to_owned()
                } else {
                    crate::tf!(
                        "В пространствах: {a0}",
                        a0 = bot
                            .spaces
                            .iter()
                            .map(|s| s.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                ui.label(theme::muted(spaces).size(12.5));
            });
        });
        ui.add_space(6.0);
        if confirm_row(ui, state, bridge, bot) {
            return;
        }
        if state.form.as_ref().and_then(|f| f.id.as_deref()) == Some(bot.id.as_str()) {
            form(ui, state, bridge);
            return;
        }
        ui.horizontal_wrapped(|ui| {
            let idle = !state.busy;
            let picking = state.picking.as_deref() == Some(bot.id.as_str());
            if ui
                .add_enabled(idle, egui::Button::new(tr("Добавить в пространство")))
                .clicked()
            {
                state.picking = (!picking).then(|| bot.id.clone());
            }
            if ui
                .add_enabled(idle, egui::Button::new(tr("Изменить")))
                .clicked()
            {
                state.form = Some(Form {
                    id: Some(bot.id.clone()),
                    username: bot.username.clone(),
                    display_name: bot.display_name.clone().unwrap_or_default(),
                    description: bot.description.clone().unwrap_or_default(),
                });
                state.note = None;
            }
            if ui
                .add_enabled(idle, egui::Button::new(tr("Новый токен")))
                .clicked()
            {
                state.confirm = Some(Confirm::Regenerate(bot.id.clone()));
            }
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(RichText::new(tr("Удалить")).color(theme::rose())),
                )
                .clicked()
            {
                state.confirm = Some(Confirm::Delete(bot.id.clone()));
            }
        });
        if state.picking.as_deref() == Some(bot.id.as_str()) {
            ui.add_space(6.0);
            space_picker(ui, state, bridge, bot);
        }
    });
}

/// Подтверждение удаления или нового токена; `true`, если показано.
fn confirm_row(ui: &mut egui::Ui, state: &mut BotsUi, bridge: &Bridge, bot: &Bot) -> bool {
    match state.confirm.clone() {
        Some(Confirm::Delete(id)) if id == bot.id => {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(tr("Удалить бота? Он выйдет из всех пространств."))
                        .color(theme::rose()),
                );
                if ui
                    .button(RichText::new(tr("Да, удалить")).color(theme::rose()))
                    .clicked()
                {
                    state.confirm = None;
                    state.act(bridge, BotOp::Delete(id.clone()));
                }
                if ui.button(tr("Отмена")).clicked() {
                    state.confirm = None;
                }
            });
            true
        }
        Some(Confirm::Regenerate(id)) if id == bot.id => {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(tr("Старый токен перестанет работать.")).color(theme::amber()),
                );
                if ui.button(tr("Выдать новый")).clicked() {
                    state.confirm = None;
                    state.act(bridge, BotOp::Regenerate(id.clone()));
                }
                if ui.button(tr("Отмена")).clicked() {
                    state.confirm = None;
                }
            });
            true
        }
        _ => false,
    }
}

/// Пространства, где можно управлять и где бота ещё нет.
fn space_picker(ui: &mut egui::Ui, state: &mut BotsUi, bridge: &Bridge, bot: &Bot) {
    widgets::section_label(ui, tr("Куда добавить"));
    let origin = bridge.focus_index();
    let mut spaces: Vec<(String, String)> = bridge
        .cache_at(origin)
        .map(|cache| {
            cache
                .spaces()
                .filter(|s| !bot.spaces.iter().any(|b| b.id == s.space.id))
                .filter(|s| {
                    super::space_settings::rights_for(bridge, origin, &s.space.id)
                        .is_some_and(|r| r.manage_space)
                })
                .map(|s| (s.space.id.clone(), s.space.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    spaces.sort_by_key(|(_, name)| name.to_lowercase());
    if spaces.is_empty() {
        ui.label(theme::muted(tr(
            "Нет пространств, где вы можете управлять и где этого бота ещё нет.",
        )));
        return;
    }
    let mut chosen = None;
    for (id, name) in &spaces {
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).color(theme::text()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!state.busy, egui::Button::new(tr("Добавить")))
                    .clicked()
                {
                    chosen = Some(id.clone());
                }
            });
        });
    }
    if let Some(space) = chosen {
        state.act(
            bridge,
            BotOp::AddToSpace {
                bot: bot.id.clone(),
                space,
            },
        );
    }
}

fn form(ui: &mut egui::Ui, state: &mut BotsUi, bridge: &Bridge) {
    let Some(form) = &mut state.form else {
        return;
    };
    let mut submit = None;
    let mut cancel = false;
    card(ui, |ui| {
        let editing = form.id.is_some();
        widgets::section_label(
            ui,
            if editing {
                tr("Изменить бота")
            } else {
                tr("Новый бот")
            },
        );
        if !editing {
            widgets::field_label(ui, tr("Имя для входа и упоминаний"));
            widgets::text_field(ui, &mut form.username, "weather_bot", 32);
        }
        widgets::field_label(ui, tr("Отображаемое имя"));
        widgets::text_field(
            ui,
            &mut form.display_name,
            tr("Как его видят остальные"),
            32,
        );
        widgets::field_label(ui, tr("Описание"));
        widgets::text_field(ui, &mut form.description, tr("Что умеет бот"), 400);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let ready = editing || form.username.trim().chars().count() >= 2;
            if ui
                .add_enabled(
                    ready && !state.busy,
                    theme::primary_button(if editing {
                        tr("Сохранить")
                    } else {
                        tr("Создать")
                    }),
                )
                .clicked()
            {
                submit = Some(match &form.id {
                    Some(id) => BotOp::Update {
                        id: id.clone(),
                        display_name: form.display_name.clone(),
                        description: form.description.clone(),
                    },
                    None => BotOp::Create {
                        username: form.username.clone(),
                        display_name: form.display_name.clone(),
                        description: form.description.clone(),
                    },
                });
            }
            if ui.button(tr("Отмена")).clicked() {
                cancel = true;
            }
        });
    });
    if cancel {
        state.form = None;
    }
    if let Some(op) = submit {
        state.act(bridge, op);
    }
}
