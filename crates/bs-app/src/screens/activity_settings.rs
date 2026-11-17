//! Настройки → «Активность»: игры, музыка, Rich Presence и выбор игрока.
//! По мотивам `ActivityGameSection.tsx` из PR #482 оригинального Backspace
//! (QuddyMusic, cirzear), в оформлении Rustspace.

use egui::{Align, Layout, RichText};

use super::dialogs::{card, page_title};
use super::settings_extra::setting_row;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::presence::{self, ActivityPrefs, key};
use crate::{theme, widgets};

/// Состояние страницы.
#[derive(Debug, Default)]
pub(crate) struct ActivityPage {
    /// Новая игра: имя и программа.
    new_name: String,
    new_program: Option<String>,
    /// Переименование: ключ строки и текст.
    renaming: Option<(String, String)>,
}

/// Строка списка игр.
struct GameRow {
    /// Найденное (или своё) имя — ключ выборов.
    name: String,
    /// Что показано другим.
    shown: String,
    /// Откуда: «Steam», «словарь», «своя», «запомнена».
    source: String,
    running: bool,
    manual: bool,
    hidden: bool,
}

fn source_label(source: &str) -> String {
    match source {
        "steam" => "Steam".into(),
        "flatpak" => "Flatpak".into(),
        "gog" => "GOG".into(),
        "heroic" => "Heroic".into(),
        "bottles" => "Bottles".into(),
        "lutris" => "Lutris".into(),
        "manual" => tr("своя").into(),
        _ => tr("из словаря").into(),
    }
}

fn rows(prefs: &ActivityPrefs, overview: &presence::Overview) -> Vec<GameRow> {
    let mut rows: Vec<GameRow> = Vec::new();
    let mut push = |name: &str, source: String, running: bool, manual: bool| {
        if rows.iter().any(|r| key(&r.name) == key(name)) {
            return;
        }
        rows.push(GameRow {
            name: name.to_owned(),
            shown: prefs
                .renames
                .get(&key(name))
                .cloned()
                .unwrap_or_else(|| name.to_owned()),
            source,
            running,
            manual,
            hidden: prefs.hidden.contains(&key(name)),
        });
    };
    for (name, source) in &overview.games {
        let manual = prefs.manual.iter().any(|g| key(&g.name) == key(name));
        push(name, source_label(source), true, manual);
    }
    for game in &prefs.manual {
        let label = game.process.as_deref().map_or_else(
            || tr("своя, видна всегда").to_owned(),
            |p| crate::tf!("своя · {p}", p = p),
        );
        push(&game.name, label, false, true);
    }
    for remembered in prefs
        .renames
        .keys()
        .chain(&prefs.hidden)
        .chain(&prefs.order)
    {
        push(remembered, tr("запомнена").into(), false, false);
    }
    let rank = |r: &GameRow| {
        prefs
            .order
            .iter()
            .position(|k| *k == key(&r.name))
            .unwrap_or(usize::MAX)
    };
    rows.sort_by_key(|r| (rank(r), !r.running));
    rows
}

/// Страница «Активность».
pub(crate) fn page(ui: &mut egui::Ui, state: &mut ActivityPage) {
    page_title(ui, tr("Активность"));
    let saved = crate::prefs::get().activity;
    let mut prefs = saved.clone();
    let overview = presence::overview();
    switches(ui, &mut prefs, &overview);
    ui.add_space(10.0);
    now_card(ui, &overview);
    ui.add_space(10.0);
    games_card(ui, state, &mut prefs, &overview);
    ui.add_space(10.0);
    add_card(ui, state, &mut prefs, &overview);
    ui.add_space(8.0);
    ui.label(
        theme::muted(tr(
            "Распознавание игр, Rich Presence и музыка перенесены из PR #482 Backspace — \
             авторы QuddyMusic и cirzear. Словарь игр — общий с Backspace.",
        ))
        .size(11.5),
    );
    if prefs != saved {
        let rescan = prefs.manual != saved.manual || prefs.games != saved.games;
        crate::prefs::update(|p| p.activity = prefs);
        if rescan {
            presence::rescan();
        }
    }
    // Список найденного обновляется сам.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs(2));
}

fn switches(ui: &mut egui::Ui, prefs: &mut ActivityPrefs, overview: &presence::Overview) {
    card(ui, |ui| {
        widgets::section_label(ui, tr("Что показывать"));
        ui.add_space(4.0);
        setting_row(
            ui,
            tr("Игры"),
            tr(
                "Запущенные игры: словарь Backspace, Steam, Flatpak, GOG, Heroic, Bottles, Lutris, игры под Wine и Proton.",
            ),
            &mut prefs.games,
        );
        let media_hint = if cfg!(all(target_os = "linux", feature = "presence")) {
            tr("Что играет в плеере или браузере (MPRIS): трек, исполнитель, обложка и прогресс.")
        } else {
            tr("Что играет в плеере. Пока только на Linux (MPRIS).")
        };
        setting_row(ui, tr("Музыка и видео"), media_hint, &mut prefs.media);
        let ipc_hint = match (prefs.rich_presence, overview.ipc_sockets) {
            (false, _) => tr(
                "Игры с Rich Presence (osu!lazer и другие) сообщают карту, режим, время и обложку.",
            ),
            (true, 0) => tr(
                "Все места заняты другой программой (например, клиентом Discord): игры говорят с ней.",
            ),
            (true, _) => tr(
                "Слушаем локальный канал Rich Presence: игры сами сообщают карту, режим, время и обложку.",
            ),
        };
        setting_row(
            ui,
            tr("Rich Presence от игр"),
            ipc_hint,
            &mut prefs.rich_presence,
        );
        setting_row(
            ui,
            tr("Иконки игр"),
            tr(
                "Иконка берётся с этого компьютера и загружается на ваш домашний инстанс (нужен rustspace-server).",
            ),
            &mut prefs.icons,
        );
        ui.label(
            theme::muted(tr(
                "Скрыть активность от всех можно в «Приватности» — это настройка аккаунта.",
            ))
            .size(12.0),
        );
    });
}

fn now_card(ui: &mut egui::Ui, overview: &presence::Overview) {
    card(ui, |ui| {
        widgets::section_label(ui, tr("Сейчас видят другие"));
        ui.add_space(2.0);
        if overview.shown.is_empty() {
            ui.label(theme::muted(tr("Ничего: ни игры, ни музыки не найдено.")));
        } else {
            ui.add_space(4.0);
            super::activity_view::cards(ui, "me", &overview.shown);
        }
        if let Some(game) = &overview.ipc_game {
            ui.label(
                theme::muted(crate::tf!(
                    "Rich Presence получен от игры «{game}».",
                    game = game
                ))
                .size(12.0),
            );
        }
    });
}

fn games_card(
    ui: &mut egui::Ui,
    state: &mut ActivityPage,
    prefs: &mut ActivityPrefs,
    overview: &presence::Overview,
) {
    let list = rows(prefs, overview);
    card(ui, |ui| {
        widgets::section_label(ui, tr("Игры"));
        ui.add_space(2.0);
        if list.is_empty() {
            ui.label(theme::muted(tr(
                "Запустите игру — она появится здесь. Порядок, имя и видимость запоминаются.",
            )));
            return;
        }
        let keys: Vec<String> = list.iter().map(|r| key(&r.name)).collect();
        for (index, row) in list.iter().enumerate() {
            game_row(ui, state, prefs, row, index, &keys);
        }
    });
}

fn game_row(
    ui: &mut egui::Ui,
    state: &mut ActivityPage,
    prefs: &mut ActivityPrefs,
    row: &GameRow,
    index: usize,
    keys: &[String],
) {
    let k = key(&row.name);
    ui.horizontal(|ui| {
        let color = if row.running {
            theme::mint()
        } else {
            theme::text_faint()
        };
        ui.add(widgets::flat_icon(Icon::Gamepad, 18.0, color));
        if state.renaming.as_ref().is_some_and(|(e, _)| *e == k) {
            rename_editor(ui, state, prefs, row);
            return;
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let name = RichText::new(&row.shown).color(if row.hidden {
                theme::text_faint()
            } else {
                theme::text_bright()
            });
            ui.label(if row.hidden {
                name.strikethrough()
            } else {
                name
            });
            let mut sub = row.source.clone();
            if row.shown != row.name {
                sub = crate::tf!(
                    "{sub} · найдена как «{name}»",
                    sub = sub,
                    name = row.name.clone()
                );
            }
            if row.running {
                sub = crate::tf!("{sub} · запущена", sub = sub);
            }
            ui.label(theme::muted(sub).size(11.5));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if row.manual
                && ui
                    .add(widgets::flat_icon(Icon::Close, 16.0, theme::text_faint()))
                    .on_hover_text(tr("Удалить"))
                    .clicked()
            {
                prefs.manual.retain(|g| key(&g.name) != k);
            }
            let eye = if row.hidden {
                theme::text_faint()
            } else {
                theme::text_muted()
            };
            if ui
                .add(widgets::flat_icon(Icon::Eye, 18.0, eye))
                .on_hover_text(if row.hidden {
                    tr("Показывать")
                } else {
                    tr("Скрыть от других")
                })
                .clicked()
            {
                prefs.set_hidden(&row.name, !row.hidden);
            }
            if ui
                .small_button(tr("Имя"))
                .on_hover_text(tr("Показывать под другим именем"))
                .clicked()
            {
                state.renaming = Some((k.clone(), row.shown.clone()));
            }
            let reorder = |prefs: &mut ActivityPrefs, to: usize| {
                let mut order = keys.to_vec();
                order.swap(index, to);
                prefs.order = order;
            };
            if index + 1 < keys.len() && ui.small_button("↓").on_hover_text(tr("Ниже")).clicked()
            {
                reorder(prefs, index + 1);
            }
            if index > 0
                && ui
                    .small_button("↑")
                    .on_hover_text(tr("Выше: важнее в списке"))
                    .clicked()
            {
                reorder(prefs, index - 1);
            }
        });
    });
    ui.add_space(4.0);
}

fn add_card(
    ui: &mut egui::Ui,
    state: &mut ActivityPage,
    prefs: &mut ActivityPrefs,
    overview: &presence::Overview,
) {
    card(ui, |ui| {
        widgets::section_label(ui, tr("Своя игра"));
        ui.label(
            theme::muted(tr(
                "Игры нет в списке? Назовите её и укажите программу — статус появится, пока она \
                 запущена. Без программы игра видна всегда.",
            ))
            .size(12.5),
        );
        ui.add_space(6.0);
        widgets::text_field(ui, &mut state.new_name, tr("Название игры"), 128);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let selected = state
                .new_program
                .clone()
                .unwrap_or_else(|| tr("Без программы").to_owned());
            egui::ComboBox::from_id_salt("activity_program")
                .selected_text(selected)
                .width(260.0)
                .height(320.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut state.new_program, None, tr("Без программы"));
                    for program in &overview.programs {
                        ui.selectable_value(&mut state.new_program, Some(program.clone()), program);
                    }
                });
            let ready = presence::clean_name(&state.new_name).is_some();
            if ui
                .add_enabled(ready, egui::Button::new(tr("Добавить")))
                .clicked()
            {
                prefs.add_manual(&state.new_name, state.new_program.as_deref());
                state.new_name.clear();
                state.new_program = None;
            }
        });
        if !overview.unknown_wine.is_empty() {
            ui.add_space(8.0);
            ui.label(
                theme::muted(tr("Запущены программы Windows, которых нет в словаре:")).size(12.5),
            );
            for program in &overview.unknown_wine {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(program).color(theme::text()));
                    if ui.small_button(tr("Это игра")).clicked() {
                        let name = program.trim_end_matches(".exe").trim_end_matches(".EXE");
                        prefs.add_manual(name, Some(program));
                    }
                });
            }
        }
    });
}

/// Поле нового имени игры.
fn rename_editor(
    ui: &mut egui::Ui,
    state: &mut ActivityPage,
    prefs: &mut ActivityPrefs,
    row: &GameRow,
) {
    let Some((_, text)) = state.renaming.as_mut() else {
        return;
    };
    let response = ui.add(
        egui::TextEdit::singleline(text)
            .desired_width(200.0)
            .char_limit(128),
    );
    let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if enter || ui.button(tr("Готово")).clicked() {
        let chosen = text.clone();
        let k = key(&row.name);
        if row.manual {
            if let Some(name) = presence::clean_name(&chosen)
                && let Some(game) = prefs.manual.iter_mut().find(|g| key(&g.name) == k)
            {
                game.name = name;
            }
        } else {
            prefs.set_rename(&row.name, Some(&chosen));
        }
        state.renaming = None;
    } else if ui.button(tr("Отмена")).clicked() {
        state.renaming = None;
    }
}
