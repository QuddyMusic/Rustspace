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
        engine_fields(ui, &mut prefs);
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

/// Адреса и ключи выбранного сервиса.
fn engine_fields(ui: &mut egui::Ui, prefs: &mut TranslatePrefs) {
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
        Engine::Google => {
            ui.label(
                theme::muted(tr(
                    "Без ключа и настройки. Это неофициальный адрес веб-переводчика: при частых \
                 запросах Google может временно отказывать — тогда выберите другой сервис.",
                ))
                .size(12.5),
            );
        }
        Engine::Lingva => {
            field(
                ui,
                tr("Адрес Lingva"),
                &mut prefs.lingva_url,
                "https://lingva.ml",
                false,
            );
        }
        Engine::Libre => {
            field(
                ui,
                tr("Адрес LibreTranslate"),
                &mut prefs.libre_url,
                "https://libretranslate.com",
                false,
            );
            field(
                ui,
                tr("Ключ API (если нужен)"),
                &mut prefs.libre_key,
                "",
                true,
            );
            ui.label(theme::muted(tr(
                "Свой сервер без ключа: docker run -p 5000:5000 libretranslate/libretranslate — \
                 адрес http://127.0.0.1:5000.",
            )).size(12.0));
        }
        Engine::Deepl => {
            field(ui, tr("Ключ DeepL API"), &mut prefs.deepl_key, "…:fx", true);
            ui.label(
                theme::muted(tr(
                    "Бесплатный ключ (до 500 000 знаков в месяц) — на deepl.com/pro-api.",
                ))
                .size(12.0),
            );
        }
        Engine::Llm => {
            field(
                ui,
                tr("Адрес API (OpenAI-совместимый)"),
                &mut prefs.llm_url,
                "http://127.0.0.1:11434/v1",
                false,
            );
            field(ui, tr("Модель"), &mut prefs.llm_model, "qwen2.5:3b", false);
            field(
                ui,
                tr("Ключ (для локальной не нужен)"),
                &mut prefs.llm_key,
                "",
                true,
            );
            ui.add_space(4.0);
            ui.label(
                theme::muted(tr(
                    "Локально через Ollama: установите ollama.com, затем ollama pull qwen2.5:3b — \
                 адрес по умолчанию уже подходит. Маленькие модели (1.5–3B) быстры на \
                 процессоре; 7B переводят заметно лучше. Подходят LM Studio \
                 (http://127.0.0.1:1234/v1) и llama.cpp (llama-server, /v1), а также облачные \
                 сервисы с OpenAI-совместимым API и ключом.",
                ))
                .size(12.0),
            );
        }
    }
}
