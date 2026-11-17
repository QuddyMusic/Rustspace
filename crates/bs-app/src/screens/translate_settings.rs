//! Настройки → «Перевод»: сервис, язык, автоперевод и проверка.

use egui::RichText;

use super::dialogs::{card, page_title};
use super::settings_extra::setting_row;
use crate::i18n::tr;
use crate::theme;
use crate::translate::{Engine, LANGUAGES, TranslatePrefs};
use crate::widgets;

/// Итог проверки сервиса: текст или ошибка; `None` — ждём ответа.
type Check = std::sync::Arc<std::sync::Mutex<Option<Result<String, String>>>>;

/// Состояние страницы.
#[derive(Debug, Default)]
pub(crate) struct TranslatePage {
    check: Option<Check>,
}

/// Страница «Перевод».
pub(crate) fn page(ui: &mut egui::Ui, state: &mut TranslatePage, runtime: &tokio::runtime::Handle) {
    page_title(ui, tr("Перевод"));
    let saved = crate::prefs::get().translate;
    let mut prefs = saved.clone();
    card(ui, |ui| {
        widgets::section_label(ui, tr("Чем переводить"));
        ui.add_space(4.0);
        for engine in Engine::ALL {
            ui.radio_value(&mut prefs.engine, engine, engine.title());
        }
        ui.add_space(6.0);
        engine_fields(ui, &mut prefs, runtime);
    });
    ui.add_space(10.0);
    card(ui, |ui| {
        widgets::section_label(ui, tr("Язык перевода"));
        ui.add_space(4.0);
        let current = prefs.target_language();
        let label = LANGUAGES
            .iter()
            .find(|(code, _)| *code == current)
            .map_or_else(|| current.clone(), |(_, name)| (*name).to_owned());
        egui::ComboBox::from_id_salt("translate_target")
            .selected_text(if prefs.target.is_empty() {
                crate::tf!("Как интерфейс ({label})", label = label)
            } else {
                label
            })
            .width(240.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut prefs.target, String::new(), tr("Как интерфейс"));
                for (code, name) in LANGUAGES {
                    ui.selectable_value(&mut prefs.target, code.to_owned(), name);
                }
            });
        ui.add_space(8.0);
        setting_row(
            ui,
            tr("Переводить сразу"),
            tr(
                "Чужие сообщения не на вашем языке переводятся сами, как только видны. \
                Текст уходит выбранному сервису.",
            ),
            &mut prefs.auto,
        );
        setting_row(
            ui,
            tr("Оставлять оригинал"),
            tr("Перевод под сообщением. Без этого — вместо текста, оригинал в меню."),
            &mut prefs.keep_original,
        );
    });
    ui.add_space(10.0);
    check_card(ui, state, &prefs, runtime);
    if prefs != saved {
        crate::prefs::update(|p| p.translate = prefs);
    }
}

/// Проверка: перевести пример текущими (ещё не сохранёнными) настройками.
fn check_card(
    ui: &mut egui::Ui,
    state: &mut TranslatePage,
    prefs: &TranslatePrefs,
    runtime: &tokio::runtime::Handle,
) {
    card(ui, |ui| {
        widgets::section_label(ui, tr("Проверка"));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(tr("Перевести пример")).clicked() {
                let check: Check = std::sync::Arc::default();
                state.check = Some(check.clone());
                let prefs = prefs.clone();
                let ctx = ui.ctx().clone();
                runtime.spawn(async move {
                    let target = prefs.target_language();
                    let sample = if target == "en" {
                        "Привет! Сегодня созвон в 20:00, ссылка: https://example.org"
                    } else {
                        "Hi! The call is at 8 pm today, link: https://example.org"
                    };
                    let result = crate::translate::translate(&prefs, &target, sample).await;
                    *check
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
                    ctx.request_repaint();
                });
            }
            let result = state.check.as_ref().map(|c| {
                c.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
            });
            match result {
                Some(None) => {
                    ui.spinner();
                }
                Some(Some(Ok(text))) => {
                    ui.label(RichText::new(text).color(theme::mint()));
                }
                Some(Some(Err(err))) => {
                    ui.label(RichText::new(err).color(theme::rose()));
                }
                None => {}
            }
        });
        ui.add_space(4.0);
        ui.label(
            theme::muted(tr(
                "Перевод идёт из клиента прямо в выбранный сервис, без сервера Rustspace. \
                 Код, ссылки и упоминания не отправляются на перевод — они остаются как есть.",
            ))
            .size(12.0),
        );
    });
}

/// Настройки выбранного варианта.
fn engine_fields(ui: &mut egui::Ui, prefs: &mut TranslatePrefs, runtime: &tokio::runtime::Handle) {
    let field = |ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str, secret: bool| {
        widgets::field_label(ui, label);
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .password(secret)
                .desired_width(f32::INFINITY)
                .margin(egui::vec2(10.0, 6.0)),
        );
    };
    match prefs.engine {
        Engine::Local => local_fields(ui, &mut prefs.local, runtime),
        Engine::Libre => {
            field(
                ui,
                tr("Адрес LibreTranslate"),
                &mut prefs.libre_url,
                "http://127.0.0.1:5000",
                false,
            );
            field(
                ui,
                tr("Ключ API (если сервер его просит)"),
                &mut prefs.libre_key,
                "",
                true,
            );
            ui.label(
                theme::muted(tr(
                    "LibreTranslate — открытый переводчик на моделях Argos. Свой сервер: \
                     pip install libretranslate && libretranslate — адрес http://127.0.0.1:5000.",
                ))
                .size(12.0),
            );
        }
        Engine::Llm => {
            field(
                ui,
                tr("Адрес API (OpenAI-совместимый)"),
                &mut prefs.llm_url,
                "http://127.0.0.1:8080/v1",
                false,
            );
            field(
                ui,
                tr("Модель (если сервер держит несколько)"),
                &mut prefs.llm_model,
                "",
                false,
            );
            field(
                ui,
                tr("Ключ (если сервер его просит)"),
                &mut prefs.llm_key,
                "",
                true,
            );
            ui.label(
                theme::muted(tr(
                    "Например, llama-server из llama.cpp на другом компьютере: \
                     llama-server -m модель.gguf --host 0.0.0.0 --port 8080.",
                ))
                .size(12.0),
            );
        }
    }
}

/// llama.cpp на этом компьютере: модель, устройство, установка.
fn local_fields(
    ui: &mut egui::Ui,
    local: &mut crate::local_llm::LocalPrefs,
    runtime: &tokio::runtime::Handle,
) {
    use crate::local_llm::{self, MODELS, Progress};
    ui.label(
        theme::muted(tr(
            "Перевод без ключей и без интернета: llama.cpp (MIT) и открытая модель Qwen \
             (Apache-2.0) скачиваются один раз и работают на этом компьютере.",
        ))
        .size(12.5),
    );
    widgets::field_label(ui, tr("Модель"));
    for model in MODELS {
        ui.radio_value(
            &mut local.model,
            model.id.to_owned(),
            crate::tf!(
                "{title} · ≈{size} ГБ",
                title = model.title(),
                size = model.size
            ),
        );
    }
    widgets::field_label(ui, tr("Где считать"));
    ui.horizontal(|ui| {
        ui.radio_value(&mut local.gpu, true, tr("Видеокарта"));
        ui.radio_value(&mut local.gpu, false, tr("Процессор"));
    });
    if local.gpu && !local_llm::gpu_available() {
        ui.label(
            theme::muted(tr("Vulkan не найден — на видеокарте может не запуститься.")).size(12.0),
        );
    }
    ui.add_space(8.0);
    let have = local_llm::installed(&local.model);
    let ready = have.engine.is_some() && have.model.is_some() && have.gpu == local.gpu;
    let status = local_llm::status();
    let busy = matches!(
        status,
        Progress::Looking(_) | Progress::Downloading(..) | Progress::Unpacking
    );
    ui.horizontal(|ui| {
        if ready {
            ui.label(
                RichText::new(crate::tf!(
                    "Установлено: llama.cpp {tag}",
                    tag = have.engine.clone().unwrap_or_default()
                ))
                .color(theme::mint()),
            );
        } else if ui
            .add_enabled(!busy, egui::Button::new(tr("Установить")))
            .clicked()
        {
            local_llm::install(runtime, ui.ctx(), local);
        }
        if (have.engine.is_some() || have.model.is_some())
            && !busy
            && ui
                .small_button(tr("Удалить всё"))
                .on_hover_text(tr("llama.cpp и скачанные модели"))
                .clicked()
        {
            local_llm::remove();
        }
    });
    match status {
        Progress::Looking(what) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::muted(crate::tf!("Ищу {what}…", what = what)));
            });
        }
        Progress::Downloading(what, got, total) => {
            #[allow(clippy::cast_precision_loss, reason = "доля для полосы")]
            let fraction = total.map_or(0.0, |t| got as f32 / t.max(1) as f32);
            let mb = |b: u64| b / (1024 * 1024);
            let text = total.map_or_else(
                || crate::tf!("{what}: {got} МБ", what = what, got = mb(got)),
                |t| {
                    crate::tf!(
                        "{what}: {got} из {total} МБ",
                        what = what,
                        got = mb(got),
                        total = mb(t)
                    )
                },
            );
            ui.add(egui::ProgressBar::new(fraction).text(text));
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }
        Progress::Unpacking => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::muted(tr("Распаковываю…")));
            });
        }
        Progress::Failed(error) => {
            ui.label(RichText::new(error).color(theme::rose()));
        }
        Progress::Done | Progress::Idle => {}
    }
}
