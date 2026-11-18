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
//!
//! Модели берутся так же, как их берёт Firefox: список — из Remote Settings
//! Mozilla, файлы — с её CDN, сжатые zstd (распаковывает ruzstd) и с SHA-256
//! распакованного файла, который сверяется.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use fxtranslate::engine::{Engine, Translation};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::i18n::tr;

/// Сколько направлений держать в памяти (каждое — около 150 МиБ).
const LOADED_MAX: usize = 2;
/// Список моделей Firefox Translations.
const RECORDS_URL: &str = "https://firefox.settings.services.mozilla.com/v1/buckets/main/collections/translations-models-v2/records";
/// Откуда скачиваются файлы моделей.
const CDN_ROOT: &str = "https://firefox-settings-attachments.cdn.mozilla.net";
/// Версия формата моделей, которую понимает fxtranslate 0.5.
const SUPPORTED_MAJOR: u64 = 3;
/// Больше сжатый файл модели не бывает.
const MAX_DOWNLOAD: usize = 128 * 1024 * 1024;
/// Больше распакованный файл не бывает (защита от «zip-бомбы»).
const MAX_UNPACKED: u64 = 256 * 1024 * 1024;

/// Где лежат модели: `<каталог>/<src>-<trg>/`.
fn root() -> Option<PathBuf> {
    crate::saved_session::config_dir().map(|d| d.join("translate-models"))
}

fn root_or_err() -> Result<PathBuf, String> {
    root().ok_or_else(|| tr("не найден каталог настроек").to_owned())
}

/// Файлы одного направления на диске.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pair {
    model: PathBuf,
    src_vocab: PathBuf,
    trg_vocab: PathBuf,
}

impl Pair {
    /// Направление, уже лежащее в каталоге.
    fn on_disk(dir: &Path) -> Option<Self> {
        let model = dir.join("model.bin");
        if !model.is_file() {
            return None;
        }
        let shared = dir.join("vocab.spm");
        if shared.is_file() {
            return Some(Self {
                model,
                src_vocab: shared.clone(),
                trg_vocab: shared,
            });
        }
        let (src, trg) = (dir.join("srcvocab.spm"), dir.join("trgvocab.spm"));
        (src.is_file() && trg.is_file()).then_some(Self {
            model,
            src_vocab: src,
            trg_vocab: trg,
        })
    }

    /// Движок направления (общий словарь — один и тот же путь).
    fn engine(&self) -> Result<Engine, String> {
        Engine::load(&self.model, &self.src_vocab, &self.trg_vocab)
    }
}

/// Запись Remote Settings — один файл модели.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    file_type: String,
    source_language: String,
    target_language: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    decompressed_hash: Option<String>,
    attachment: Attachment,
}

#[derive(Debug, Clone, Deserialize)]
struct Attachment {
    location: String,
}

#[derive(Deserialize)]
struct Records {
    data: Vec<serde_json::Value>,
}

fn version_key(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

/// Самая свежая запись нужного вида для пары (только понятной версии).
fn pick<'a>(records: &'a [Record], kind: &str, src: &str, trg: &str) -> Option<&'a Record> {
    records
        .iter()
        .filter(|r| r.file_type == kind && r.source_language == src && r.target_language == trg)
        .filter(|r| version_key(&r.version).first() == Some(&SUPPORTED_MAJOR))
        .max_by(|a, b| version_key(&a.version).cmp(&version_key(&b.version)))
}

fn http() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("rustspace/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(15 * 60))
        .https_only(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(CLIENT.get_or_init(|| client))
}

/// Список моделей (один раз за запуск).
async fn records() -> Result<Vec<Record>, String> {
    static CACHE: Mutex<Option<Vec<Record>>> = Mutex::new(None);
    let known = CACHE.lock().unwrap_or_else(PoisonError::into_inner).clone();
    if let Some(list) = known {
        return Ok(list);
    }
    let body: Records = http()?
        .get(RECORDS_URL)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| crate::tf!("список моделей недоступен: {a0}", a0 = e))?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    // Записи другого вида (без вложения и т. п.) пропускаются.
    let list: Vec<Record> = body
        .data
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect();
    *CACHE.lock().unwrap_or_else(PoisonError::into_inner) = Some(list.clone());
    Ok(list)
}

/// Скачивает, распаковывает и сверяет файл записи.
async fn fetch_file(record: &Record, dest: &Path) -> Result<(), String> {
    let url = format!(
        "{CDN_ROOT}/{}",
        record.attachment.location.trim_start_matches('/')
    );
    let mut response = http()?
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.to_string())?;
    let mut packed = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if packed.len() + chunk.len() > MAX_DOWNLOAD {
            return Err(tr("файл модели слишком большой").to_owned());
        }
        packed.extend_from_slice(&chunk);
    }
    let expected = record.decompressed_hash.clone();
    let dest = dest.to_owned();
    tokio::task::spawn_blocking(move || {
        let bytes = unpack(&packed)?;
        if let Some(expected) = expected {
            let got = Sha256::digest(&bytes)
                .iter()
                .fold(String::new(), |mut hex, b| {
                    use std::fmt::Write as _;
                    let _ = write!(hex, "{b:02x}");
                    hex
                });
            if !got.eq_ignore_ascii_case(expected.trim()) {
                return Err(tr("файл модели повреждён (не сошлась контрольная сумма)").to_owned());
            }
        }
        let tmp = dest.with_extension("part");
        std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Распаковка zstd с пределом размера.
fn unpack(packed: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let decoder =
        ruzstd::decoding::StreamingDecoder::new(packed).map_err(|e| format!("zstd: {e}"))?;
    let mut out = Vec::new();
    decoder
        .take(MAX_UNPACKED + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("zstd: {e}"))?;
    if out.len() as u64 > MAX_UNPACKED {
        return Err(tr("файл модели слишком большой").to_owned());
    }
    Ok(out)
}

/// Направление на диске; если его нет — скачивается.
async fn ensure_pair(records: &[Record], src: &str, trg: &str) -> Result<Pair, String> {
    let dir = root_or_err()?.join(format!("{src}-{trg}"));
    if let Some(pair) = Pair::on_disk(&dir) {
        return Ok(pair);
    }
    let model = pick(records, "model", src, trg)
        .ok_or_else(|| crate::tf!("нет модели {a0} → {a1}", a0 = src, a1 = trg))?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    *downloading() = Some(format!("{src} → {trg}"));
    let result = async {
        if let Some(vocab) = pick(records, "vocab", src, trg) {
            fetch_file(vocab, &dir.join("vocab.spm")).await?;
        } else {
            let no_vocab = || crate::tf!("нет словаря {a0} → {a1}", a0 = src, a1 = trg);
            let source = pick(records, "srcvocab", src, trg).ok_or_else(no_vocab)?;
            let target = pick(records, "trgvocab", src, trg).ok_or_else(no_vocab)?;
            fetch_file(source, &dir.join("srcvocab.spm")).await?;
            fetch_file(target, &dir.join("trgvocab.spm")).await?;
        }
        // Модель — последней: по ней видно, что направление скачано целиком.
        fetch_file(model, &dir.join("model.bin")).await
    }
    .await;
    *downloading() = None;
    result?;
    Pair::on_disk(&dir).ok_or_else(|| tr("модель не сохранилась").to_owned())
}

/// Направления для перевода `src` → `trg`: прямое или через английский.
/// Без сети годится то, что уже скачано.
async fn route(src: &str, trg: &str) -> Result<Vec<Pair>, String> {
    let root = root_or_err()?;
    let cached = |s: &str, t: &str| Pair::on_disk(&root.join(format!("{s}-{t}")));
    if let Some(pair) = cached(src, trg) {
        return Ok(vec![pair]);
    }
    let pivot = src != "en" && trg != "en";
    if pivot && let (Some(a), Some(b)) = (cached(src, "en"), cached("en", trg)) {
        return Ok(vec![a, b]);
    }
    let records = records().await?;
    if pick(&records, "model", src, trg).is_some() || !pivot {
        return Ok(vec![ensure_pair(&records, src, trg).await?]);
    }
    Ok(vec![
        ensure_pair(&records, src, "en").await?,
        ensure_pair(&records, "en", trg).await?,
    ])
}

/// Задание потоку переводчика.
struct Job {
    pairs: Vec<Pair>,
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
                let mut loaded: Vec<(Vec<Pair>, Translation)> = Vec::new();
                for job in rx {
                    // `None` — выгрузить модели (их удалили с диска).
                    let Some(job) = job else {
                        loaded.clear();
                        continue;
                    };
                    let index = if let Some(i) = loaded.iter().position(|(k, _)| *k == job.pairs) {
                        Ok(i)
                    } else {
                        build(&job.pairs).map(|made| {
                            if loaded.len() >= LOADED_MAX {
                                loaded.remove(0);
                            }
                            loaded.push((job.pairs.clone(), made));
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

/// Движок из одного направления или из двух через английский.
fn build(pairs: &[Pair]) -> Result<Translation, String> {
    match pairs {
        [one] => Ok(Translation::Direct(one.engine()?)),
        [first, second] => Ok(Translation::Pivot {
            pivot: "en".to_owned(),
            first: first.engine()?,
            second: second.engine()?,
        }),
        _ => Err(tr("модель перевода не найдена").to_owned()),
    }
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
    let pairs = route(model_code(&source), model_code(target)).await?;
    let (reply, answer) = tokio::sync::oneshot::channel();
    let job = Job {
        pairs,
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
    let other = if target == "en" {
        "ru"
    } else {
        model_code(&target)
    };
    route("en", other).await?;
    route(other, "en").await?;
    Ok(())
}

/// Скачанные направления и их размер в байтах.
#[must_use]
pub(crate) fn installed() -> Vec<(String, u64)> {
    let Some(root) = root() else {
        return Vec::new();
    };
    let Ok(dirs) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out: Vec<(String, u64)> = dirs
        .flatten()
        .filter(|entry| Pair::on_disk(&entry.path()).is_some())
        .map(|entry| {
            let size = std::fs::read_dir(entry.path()).map_or(0, |files| {
                files
                    .flatten()
                    .filter_map(|f| f.metadata().ok())
                    .map(|m| m.len())
                    .sum()
            });
            (entry.file_name().to_string_lossy().into_owned(), size)
        })
        .collect();
    out.sort();
    out
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
    match root() {
        Some(root) if root.exists() => std::fs::remove_dir_all(root).map_err(|e| e.to_string()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{Record, detect, pick, unpack};

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

    fn record(kind: &str, version: &str) -> Record {
        serde_json::from_value(serde_json::json!({
            "fileType": kind, "sourceLanguage": "en", "targetLanguage": "ru",
            "version": version, "attachment": {"location": "x"}
        }))
        .unwrap()
    }

    #[test]
    fn newest_supported_version_is_picked() {
        let list = vec![
            record("model", "3.0"),
            record("model", "3.1"),
            record("model", "4.0"),
        ];
        assert_eq!(
            pick(&list, "model", "en", "ru").map(|r| r.version.as_str()),
            Some("3.1")
        );
        assert!(pick(&list, "vocab", "en", "ru").is_none());
    }

    #[test]
    fn garbage_is_not_unpacked() {
        assert!(unpack(b"not zstd").is_err());
    }
}
