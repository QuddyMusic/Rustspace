//! Настройки клиента, которые не живут на сервере: горячие клавиши,
//! уведомления, звуки интерфейса.
//!
//! Всё сохраняется через [`crate::prefs`].

use egui::{Align, Layout, RichText};

use super::dialogs::{card, page_title};
use super::settings_extra::setting_row;
use crate::i18n::tr;
use crate::prefs::Action;
use crate::theme;
use crate::widgets;

/// Состояние страницы горячих клавиш.
#[derive(Debug, Default)]
pub(crate) struct HotkeysState {
    /// Действие, для которого сейчас записывается сочетание.
    recording: Option<Action>,
    /// Предупреждение о конфликте.
    conflict: Option<String>,
}

/// Постоянные сочетания чата.
fn chat_keys(ui: &mut egui::Ui) {
    ui.add_space(14.0);
    widgets::section_label(ui, tr("В чате"));
    ui.add_space(6.0);
    card(ui, |ui| {
        for (keys, what) in [
            (
                "↑",
                tr("Изменить последнее своё сообщение (в пустом поле ввода)"),
            ),
            ("Esc", tr("Отменить правку, закрыть окно")),
            ("Enter", tr("Отправить")),
            ("Shift+Enter", tr("Новая строка")),
            ("Ctrl+K", tr("Поиск канала или беседы")),
            ("Ctrl+V", tr("Вставить картинку из буфера обмена")),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(what).color(theme::text_bright()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(keys).monospace().color(theme::text_muted()));
                });
            });
            ui.add_space(4.0);
        }
    });
}

/// «Горячие клавиши».
pub(crate) fn hotkeys(ui: &mut egui::Ui, state: &mut HotkeysState) {
    page_title(ui, tr("Горячие клавиши"));
    if let Some(action) = state.recording
        && let Some(captured) = crate::hotkeys::capture(ui.ctx())
    {
        state.recording = None;
        state.conflict = None;
        if let crate::hotkeys::Captured::Bind(bind) = captured {
            let taken = crate::prefs::get()
                .keybinds
                .iter()
                .find(|(other, existing)| **other != action && **existing == bind)
                .map(|(other, _)| *other);
            if let Some(other) = taken {
                state.conflict = Some(crate::tf!(
                    "{a0} уже назначено на «{a1}»",
                    a0 = bind.label(),
                    a1 = other.title()
                ));
            } else {
                crate::prefs::update(|prefs| {
                    prefs.keybinds.insert(action, bind);
                });
            }
        }
    }
    card(ui, |ui| {
        if crate::desktop::global_keys_available() {
            let mut global = crate::prefs::get().desktop.global_hotkeys;
            setting_row(
                ui,
                tr("Работают вне окна"),
                tr("Сочетания срабатывают, даже когда Rustspace свёрнут или в трее."),
                &mut global,
            );
            if global != crate::prefs::get().desktop.global_hotkeys {
                crate::prefs::update(|prefs| prefs.desktop.global_hotkeys = global);
            }
        } else {
            ui.label(
                theme::muted(tr(
                    "Сочетания работают, пока окно Rustspace в фокусе. В поле ввода срабатывают \
                     только сочетания с Ctrl или Alt.",
                ))
                .size(12.5),
            );
        }
        portal_note(ui);
        ui.add_space(10.0);
        let binds = crate::prefs::get().keybinds;
        for action in Action::ALL {
            ui.horizontal(|ui| {
                ui.label(RichText::new(action.title()).color(theme::text_bright()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let bound = binds.get(&action).copied();
                    if bound.is_some() && ui.small_button("✕").on_hover_text(tr("Убрать")).clicked()
                    {
                        crate::prefs::update(|prefs| {
                            prefs.keybinds.remove(&action);
                        });
                    }
                    let recording = state.recording == Some(action);
                    let label = if recording {
                        tr("Нажмите сочетание… (Esc — отмена)").to_owned()
                    } else {
                        bound.map_or_else(
                            || tr("Не назначено").to_owned(),
                            super::super::prefs::Keybind::label,
                        )
                    };
                    if ui
                        .add(
                            egui::Button::new(label)
                                .selected(recording)
                                .min_size(egui::vec2(180.0, 26.0)),
                        )
                        .clicked()
                    {
                        state.recording = (!recording).then_some(action);
                        state.conflict = None;
                    }
                });
            });
            ui.add_space(6.0);
        }
        if let Some(conflict) = &state.conflict {
            ui.label(RichText::new(conflict).color(theme::rose()).size(12.5));
        }
    });
    chat_keys(ui);
}

/// «Язык»: язык интерфейса и подсказка, как добавить свой перевод.
pub(crate) fn language(ui: &mut egui::Ui) {
    page_title(ui, tr("Язык"));
    let current = crate::i18n::current();
    let mut chosen = None;
    card(ui, |ui| {
        for language in crate::i18n::available() {
            let selected = language.code == current;
            if ui
                .add(
                    egui::Button::new(&language.name)
                        .selected(selected)
                        .min_size(egui::vec2(220.0, 30.0)),
                )
                .clicked()
                && !selected
            {
                chosen = Some(language.code);
            }
            ui.add_space(4.0);
        }
        ui.add_space(6.0);
        let dir = crate::saved_session::config_dir()
            .map(|dir| dir.join("locales").display().to_string())
            .unwrap_or_default();
        ui.label(
            theme::muted(crate::tf!(
                "Свой перевод: положите <код>.json в {dir}. Ключи — русские строки интерфейса, \
                 значения — перевод; поле \"@language\" — название языка.",
                dir = dir
            ))
            .size(12.5),
        );
    });
    if let Some(code) = chosen {
        crate::i18n::set_language(&code);
        crate::prefs::update(|prefs| prefs.language = code);
    }
}

/// Можно ли обновиться изнутри и как.
fn self_update_note(ui: &mut egui::Ui) {
    match crate::self_update::method() {
        crate::self_update::Method::Manual(why) => {
            ui.label(
                theme::muted(crate::tf!(
                    "Обновление изнутри недоступно: {why}",
                    why = why
                ))
                .size(12.5),
            );
        }
        _ => {
            ui.label(
                theme::muted(tr(
                    "Кнопка «Обновить» в полосе сверху скачает версию, проверит \
                     контрольную сумму и поставит её; останется перезапустить.",
                ))
                .size(12.5),
            );
        }
    }
}

/// «О клиенте»: версия и проверка обновлений по релизам репозитория клиента.
pub(crate) fn about(ui: &mut egui::Ui, bridge: &crate::fleet::Bridge) {
    use crate::updates::{self, UpdateState};
    page_title(ui, tr("О клиенте"));
    let mut prefs = crate::prefs::get().updates;
    let before = prefs.clone();
    card(ui, |ui| {
        ui.label(
            RichText::new(crate::tf!(
                "Rustspace {version}",
                version = updates::CURRENT
            ))
            .size(16.0)
            .color(theme::text_bright()),
        );
        ui.label(theme::muted(tr(
            "Неофициальный клиент на Rust, совместимый с Backspace: egui, свой звук и видео через LiveKit.",
        )));
    });
    ui.add_space(12.0);
    card(ui, |ui| {
        setting_row(
            ui,
            tr("Проверять обновления"),
            tr("Раз в шесть часов клиент смотрит релизы своего репозитория на GitHub."),
            &mut prefs.enabled,
        );
        setting_row(
            ui,
            tr("Предварительные версии"),
            tr("Предлагать и бета-выпуски."),
            &mut prefs.prereleases,
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(tr("Репозиторий клиента"));
            ui.add(
                egui::TextEdit::singleline(&mut prefs.repo)
                    .hint_text("owner/repo")
                    .desired_width(240.0),
            );
        });
        let repo = prefs.repo.trim();
        if !repo.is_empty() && !updates::valid_repo(repo) {
            ui.label(
                theme::muted(tr("Нужен вид owner/repo, например owner/rustspace"))
                    .color(theme::rose()),
            );
        }
        ui.add_space(8.0);
        let state = updates::state();
        let busy = state == UpdateState::Checking;
        if ui
            .add_enabled(!busy, egui::Button::new(tr("Проверить сейчас")))
            .clicked()
        {
            updates::check_now(&bridge.runtime(), ui.ctx(), &prefs);
        }
        let text = match &state {
            UpdateState::Idle => String::new(),
            UpdateState::Checking => tr("Проверяем…").to_owned(),
            UpdateState::UpToDate => tr("Установлена последняя версия").to_owned(),
            UpdateState::Available(release) => {
                crate::tf!("Доступна версия {version}", version = release.version)
            }
            UpdateState::Failed(err) => crate::tf!("Не удалось проверить: {err}", err = err),
        };
        if !text.is_empty() {
            ui.add(egui::Label::new(theme::muted(text)).wrap());
        }
        if let UpdateState::Available(release) = updates::state() {
            ui.add_space(6.0);
            if !release.notes.is_empty() {
                ui.label(theme::muted(release.notes.clone()).size(12.5));
            }
            ui.horizontal(|ui| {
                ui.hyperlink_to(tr("Страница релиза"), &release.page);
                if let Some(download) = &release.download {
                    ui.hyperlink_to(tr("Скачать"), download);
                }
            });
            self_update_note(ui);
            if prefs.skipped.as_deref() == Some(release.version.as_str())
                && ui
                    .small_button(tr("Снова напоминать об этой версии"))
                    .clicked()
            {
                prefs.skipped = None;
            }
        }
    });
    let trimmed = prefs.repo.trim().to_owned();
    prefs.repo = trimmed;
    if prefs != before {
        crate::prefs::update(|saved| saved.updates = prefs);
    }
}

/// «Уведомления»: системные всплывающие уведомления.
pub(crate) fn notifications(ui: &mut egui::Ui) {
    page_title(ui, tr("Уведомления"));
    let mut desktop = crate::prefs::get().desktop;
    let before = desktop.clone();
    card(ui, |ui| {
        setting_row(
            ui,
            tr("Системные уведомления"),
            tr(
                "Всплывающее окно о новом сообщении по тем же правилам, что и звук: \
             ЛС, упоминания и уровень уведомлений канала.",
            ),
            &mut desktop.notifications,
        );
        ui.add_enabled_ui(desktop.notifications, |ui| {
            setting_row(
                ui,
                tr("Показывать текст сообщения"),
                tr("Без этого в уведомлении только автор и канал."),
                &mut desktop.previews,
            );
            setting_row(
                ui,
                tr("Только когда окно не в фокусе"),
                tr("Не беспокоить, пока вы и так смотрите на Rustspace."),
                &mut desktop.only_unfocused,
            );
        });
        if ui.button(tr("Проверить уведомление")).clicked() {
            crate::notify::show(crate::notify::Toast {
                title: crate::brand::NAME.to_owned(),
                body: tr("Так будут выглядеть уведомления о сообщениях.").to_owned(),
            });
        }
        if crate::desktop::tray_available() {
            ui.add_space(8.0);
            setting_row(
                ui,
                tr("Сворачивать в трей"),
                if crate::desktop::is_wayland() {
                    tr(
                        "Закрытие окна сворачивает Rustspace (спрятать окно Wayland не позволяет); \
                        выйти — через меню значка в трее.",
                    )
                } else {
                    tr("Закрытие окна прячет Rustspace в трей; выйти — через меню значка.")
                },
                &mut desktop.close_to_tray,
            );
        }
    });
    if desktop != before {
        crate::prefs::update(|prefs| prefs.desktop = desktop);
    }
}

/// Карточка «Звуки» для страницы «Голос и видео».
pub(crate) fn sounds_card(ui: &mut egui::Ui) {
    let mut sounds = crate::prefs::get().sounds;
    let before = sounds.clone();
    card(ui, |ui| {
        crate::widgets::section_label(ui, tr("Звуки интерфейса"));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(tr("Громкость"));
            ui.spacing_mut().slider_width = 240.0;
            ui.add(egui::Slider::new(&mut sounds.volume, 0..=200).suffix(" %"));
        });
        ui.add_space(8.0);
        setting_row(
            ui,
            tr("Звук на каждое сообщение"),
            tr("Играть звук на все сообщения в каналах, а не только на ЛС и упоминания."),
            &mut sounds.message_all_channels,
        );
    });
    if sounds != before {
        crate::prefs::update(|prefs| prefs.sounds = sounds);
    }
}

/// Под Wayland сочетания вне окна назначает система: показываем, какие.
fn portal_note(ui: &mut egui::Ui) {
    use crate::desktop::PortalView;
    let text = match crate::desktop::portal_state() {
        PortalView::Off => return,
        PortalView::Pending => tr("Ждём разрешения системы на глобальные сочетания…").to_owned(),
        PortalView::Failed(err) => crate::tf!(
            "Портал глобальных сочетаний недоступен: {err}. Работают сочетания в окне.",
            err = err
        ),
        PortalView::Ready(map) => {
            let list: Vec<String> = Action::ALL
                .into_iter()
                .map(|action| {
                    let trigger = map
                        .get(&action)
                        .filter(|t| !t.is_empty())
                        .map_or_else(|| tr("не назначено").to_owned(), Clone::clone);
                    format!("{} — {trigger}", action.title())
                })
                .collect();
            crate::tf!(
                "Вне окна сочетания назначаются в настройках системы (Wayland):\n{list}",
                list = list.join("\n")
            )
        }
    };
    ui.add_space(6.0);
    ui.label(theme::muted(text).size(12.5));
}

/// Раздел «Для разработчиков»: включает инструменты для тех, кто пишет
/// программы для Rustspace.
pub(crate) fn developer(ui: &mut egui::Ui) {
    page_title(ui, tr("Для разработчиков"));
    let before = crate::prefs::get().developer_mode;
    let mut on = before;
    card(ui, |ui| {
        setting_row(
            ui,
            tr("Режим разработчика"),
            tr(
                "Показывает раздел «Боты»: свои боты, их токены и пространства. Ботов пишут на Rust \
                 (библиотека bs-bot), JavaScript, TypeScript и других языках — см. docs/bots.ru.md.",
            ),
            &mut on,
        );
    });
    if on != before {
        crate::prefs::update(|p| p.developer_mode = on);
    }
}
