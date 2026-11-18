//! Перевод без языковой модели: модели Firefox Translations (проект
//! Bergamot, Mozilla) и движок fxtranslate — их переписанный на Rust
//! вычислитель (MPL-2.0). Модели маленькие (десятки мегабайт на направление),
//! работают на процессоре и на слабых компьютерах: это не LLM, а обычный
//! машинный переводчик, как встроенный в Firefox.
//!
//! Каждая модель переводит с английского или на английский; остальные пары
//! идут через английский. Модель направления скачивается при первом переводе
//! (или кнопкой в настройках) и дальше работает без сети. Язык сообщения
//! определяется по письменности и whatlang (MIT).

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use fxtranslate::cache::Cache;
use fxtranslate::engine::Translation;
use fxtranslate::fetch::NetworkFetch;

use crate::i18n::tr;

/// Сколько направлений держать в памяти (каждое — около 150 МиБ).
const LOADED_MAX: usize = 2;

/// Где лежат модели.
fn root() -> Option<PathBuf> {
    crate::saved_session::config_dir().map(|d| d.join("translate-models"))
}

fn cache() -> Result<Cache, String> {
    root()
        .map(Cache::with_root)
        .ok_or_else(|| tr("не найден каталог настроек").to_owned())
}

/// Задание потоку переводчика.
struct Job {
    src: String,
    trg: String,
    text: String,
    reply: tokio::sync::oneshot::Sender<Result<String, String>>,
}

/// Модели держит один поток: движок нельзя передавать между потоками.
fn worker() -> &'static Mutex<mpsc::Sender<Option<Job>>> {
    static WORKER: OnceLock<Mutex<mpsc::Sender<Option<Job>>>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Option<Job>>();
        let spawned = std::thread::Builder::new()
            .name("offline-translate".to_owned())
            .spawn(move || {
                let mut loaded: Vec<((String, String), Translation)> = Vec::new();
                for job in rx {
                    // `None` — выгрузить модели (их удалили с диска).
                    let Some(job) = job else {
                        loaded.clear();
                        continue;
                    };
                    let key = (job.src.clone(), job.trg.clone());
                    let index = if let Some(i) = loaded.iter().position(|(k, _)| *k == key) {
                        Ok(i)
                    } else {
                        load(&job.src, &job.trg).map(|made| {
                            if loaded.len() >= LOADED_MAX {
                                loaded.remove(0);
                            }
                            loaded.push((key, made));
                            loaded.len() - 1
                        })
                    };
                    let result = index.map(|i| translate_marked(&loaded[i].1, &job.text));
                    let _ = job.reply.send(result);
                }
            });
        if let Err(err) = spawned {
            tracing::warn!(%err, "поток переводчика не запустился");
        }
        Mutex::new(tx)
    })
}

/// Модель направления с диска или из сети.
fn load(src: &str, trg: &str) -> Result<Translation, String> {
    let cache = cache()?;
    *downloading() = Some(format!("{src} → {trg}"));
    let result = fxtranslate::loader::load_translation(
        &NetworkFetch::new(),
        &cache,
        model_code(src),
        model_code(trg),
    );
    *downloading() = None;
    result.map_err(|err| {
        crate::tf!(
            "модель перевода {a0} → {a1} недоступна: {a2}",
            a0 = src,
            a1 = trg,
            a2 = err
        )
    })
}

/// Что сейчас скачивается (для подписи в настройках).
fn downloading() -> MutexGuard<'static, Option<String>> {
    static NOW: Mutex<Option<String>> = Mutex::new(None);
    NOW.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Подпись «скачиваю модель en → ru», если идёт загрузка.
pub(crate) fn status() -> Option<String> {
    downloading().clone()
}

/// Код языка у Firefox Translations.
fn model_code(code: &str) -> &str {
    match code {
        "zh" => "zh-Hans",
        other => other,
    }
}

/// Язык текста: двухбуквенный код или `None`, если не понять.
#[must_use]
pub(crate) fn detect(text: &str) -> Option<String> {
    let Some(info) = whatlang::detect(text) else {
        return by_script(text);
    };
    let code = match info.lang() {
        whatlang::Lang::Eng => "en",
        whatlang::Lang::Rus => "ru",
        whatlang::Lang::Ukr => "uk",
        whatlang::Lang::Bel => "be",
        whatlang::Lang::Bul => "bg",
        whatlang::Lang::Deu => "de",
        whatlang::Lang::Fra => "fr",
        whatlang::Lang::Spa => "es",
        whatlang::Lang::Por => "pt",
        whatlang::Lang::Ita => "it",
        whatlang::Lang::Pol => "pl",
        whatlang::Lang::Ces => "cs",
        whatlang::Lang::Nld => "nl",
        whatlang::Lang::Swe => "sv",
        whatlang::Lang::Dan => "da",
        whatlang::Lang::Fin => "fi",
        whatlang::Lang::Est => "et",
        whatlang::Lang::Lav => "lv",
        whatlang::Lang::Lit => "lt",
        whatlang::Lang::Hun => "hu",
        whatlang::Lang::Ron => "ro",
        whatlang::Lang::Tur => "tr",
        whatlang::Lang::Ell => "el",
        whatlang::Lang::Heb => "he",
        whatlang::Lang::Ara => "ar",
        whatlang::Lang::Pes => "fa",
        whatlang::Lang::Hin => "hi",
        whatlang::Lang::Cmn => "zh",
        whatlang::Lang::Jpn => "ja",
        whatlang::Lang::Kor => "ko",
        whatlang::Lang::Kat => "ka",
        whatlang::Lang::Hye => "hy",
        whatlang::Lang::Ind => "id",
        whatlang::Lang::Vie => "vi",
        whatlang::Lang::Tha => "th",
        _ => return by_script(text),
    };
    // Короткие латинские фразы определяются плохо: тогда считаем, что это
    // английский, — самый частый язык таких сообщений.
    if !info.is_reliable() && info.script() == whatlang::Script::Latin {
        return Some("en".to_owned());
    }
    Some(code.to_owned())
}

/// По письменности, когда whatlang не уверен: латиница — английский,
/// кириллица — русский.
fn by_script(text: &str) -> Option<String> {
    let letter = text.chars().find(|c| c.is_alphabetic())?;
    match letter {
        'a'..='z' | 'A'..='Z' => Some("en".to_owned()),
        '\u{400}'..='\u{4ff}' => Some("ru".to_owned()),
        _ => None,
    }
}

/// Переводит текст с метками `⟦n⟧`: метки и пустые строки остаются на месте,
/// переводятся только куски текста между ними.
fn translate_marked(engine: &Translation, text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let mut rest = line;
        while let Some(start) = rest.find('⟦') {
            let end = rest[start..]
                .find('⟧')
                .map_or(rest.len(), |e| start + e + '⟧'.len_utf8());
            out.push_str(&piece(engine, &rest[..start]));
            out.push_str(&rest[start..end]);
            rest = &rest[end..];
        }
        out.push_str(&piece(engine, rest));
    }
    out
}

/// Кусок без меток: пробелы по краям сохраняются, текст без букв не трогается.
fn piece(engine: &Translation, text: &str) -> String {
    if !text.chars().any(char::is_alphabetic) {
        return text.to_owned();
    }
    let lead = &text[..text.len() - text.trim_start().len()];
    let tail = &text[text.trim_end().len()..];
    format!("{lead}{}{tail}", engine.translate_long(text.trim()))
}

/// Перевод на `target` (текст уже с метками `⟦n⟧`).
///
/// # Errors
/// Язык не определён, пары нет или модель не скачалась.
pub(crate) async fn translate(target: &str, text: &str) -> Result<String, String> {
    let plain: String = text
        .split('⟦')
        .map(|part| part.split_once('⟧').map_or(part, |(_, after)| after))
        .collect::<Vec<_>>()
        .join(" ");
    let source = detect(&plain).ok_or_else(|| tr("не удалось определить язык сообщения"))?;
    if source == target {
        return Ok(text.to_owned());
    }
    let (reply, answer) = tokio::sync::oneshot::channel();
    let job = Job {
        src: source,
        trg: target.to_owned(),
        text: text.to_owned(),
        reply,
    };
    worker()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .send(Some(job))
        .map_err(|_| tr("переводчик остановился").to_owned())?;
    answer
        .await
        .map_err(|_| tr("переводчик остановился").to_owned())?
}

/// Скачать заранее модели между языком перевода и английским (в обе стороны).
///
/// # Errors
/// Нет сети или такой пары у Firefox Translations.
pub(crate) async fn prefetch(target: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let cache = cache()?;
        let pairs: Vec<(&str, &str)> = if target == "en" {
            vec![("ru", "en"), ("en", "ru")]
        } else {
            vec![("en", target.as_str()), (target.as_str(), "en")]
        };
        for (src, trg) in pairs {
            *downloading() = Some(format!("{src} → {trg}"));
            let result = fxtranslate::loader::ensure_route_files(
                &NetworkFetch::new(),
                &cache,
                model_code(src),
                model_code(trg),
            );
            *downloading() = None;
            result.map_err(|err| {
                crate::tf!(
                    "модель перевода {a0} → {a1} недоступна: {a2}",
                    a0 = src,
                    a1 = trg,
                    a2 = err
                )
            })?;
        }
        Ok(())
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Скачанные направления и их размер в байтах.
#[must_use]
pub(crate) fn installed() -> Vec<(String, u64)> {
    cache()
        .and_then(|c| c.list_cached())
        .map(|pairs| pairs.into_iter().map(|p| (p.name, p.bytes)).collect())
        .unwrap_or_default()
}

/// Удалить все скачанные модели.
///
/// # Errors
/// Файлы не удалились.
pub(crate) fn remove_all() -> Result<(), String> {
    let _ = worker()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .send(None);
    let cache = cache()?;
    for (name, _) in installed() {
        cache.remove_pair(&name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::detect;

    #[test]
    fn languages_are_recognised() {
        assert_eq!(
            detect("Привет, как дела? Сегодня отличная погода для прогулки").as_deref(),
            Some("ru")
        );
        assert_eq!(
            detect("The weather is nice today and I want to go for a walk").as_deref(),
            Some("en")
        );
        assert_eq!(
            detect("Das Wetter ist heute schön und ich möchte spazieren gehen").as_deref(),
            Some("de")
        );
        assert_eq!(detect("ok").as_deref(), Some("en"));
    }
}
