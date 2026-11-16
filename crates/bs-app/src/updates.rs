//! Проверка обновлений самого клиента по релизам GitHub.
//!
//! Десктоп оригинала проверяет свой репозиторий; этот клиент — свой:
//! `owner/repo` задаётся в настройках (по умолчанию — из переменной
//! сборки `BS_UPDATE_REPO`). Раз в шесть часов и по кнопке клиент
//! спрашивает `GET /repos/{repo}/releases/latest` (или список релизов,
//! если нужны предварительные), сравнивает версии по semver и, если есть
//! новее, показывает полосу с кнопками «Скачать» (файл под эту систему,
//! если он приложен к релизу) и «Пропустить версию».

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;

/// Версия этой сборки.
pub(crate) const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Как часто проверять.
const PERIOD: Duration = Duration::from_secs(6 * 60 * 60);

/// Настройки проверки обновлений.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct UpdatePrefs {
    /// Проверять автоматически.
    pub(crate) enabled: bool,
    /// Репозиторий клиента: `owner/repo`.
    pub(crate) repo: String,
    /// Предлагать предварительные версии.
    pub(crate) prereleases: bool,
    /// Версия, которую пользователь решил пропустить.
    pub(crate) skipped: Option<String>,
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            repo: option_env!("BS_UPDATE_REPO")
                .unwrap_or(crate::brand::UPDATE_REPO)
                .to_owned(),
            prereleases: false,
            skipped: None,
        }
    }
}

/// Найденный релиз.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Release {
    /// Версия без `v`.
    pub(crate) version: String,
    /// Страница релиза.
    pub(crate) page: String,
    /// Файл под эту систему, если приложен.
    pub(crate) download: Option<String>,
    /// Заметки к выпуску (первые строки).
    pub(crate) notes: String,
}

/// Состояние проверки.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum UpdateState {
    /// Ещё не проверяли.
    #[default]
    Idle,
    /// Запрос в пути.
    Checking,
    /// Установлена последняя версия.
    UpToDate,
    /// Есть новее.
    Available(Release),
    /// Не удалось проверить.
    Failed(String),
}

#[derive(Debug)]
struct Checker {
    state: UpdateState,
    last: Option<Instant>,
    /// Для какого репозитория получен ответ.
    repo: String,
}

static CHECKER: Mutex<Checker> = Mutex::new(Checker {
    state: UpdateState::Idle,
    last: None,
    repo: String::new(),
});

fn checker() -> MutexGuard<'static, Checker> {
    CHECKER.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Текущее состояние проверки.
pub(crate) fn state() -> UpdateState {
    checker().state.clone()
}

/// Корректно ли записан репозиторий (`owner/repo`).
#[must_use]
pub(crate) fn valid_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let ok = |part: Option<&str>| {
        part.is_some_and(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
    };
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

/// Вызывается каждый кадр: проверяет по расписанию.
pub(crate) fn tick(runtime: &tokio::runtime::Handle, ctx: &egui::Context, prefs: &UpdatePrefs) {
    let due = {
        let checker = checker();
        prefs.enabled
            && valid_repo(&prefs.repo)
            && checker.state != UpdateState::Checking
            && (checker.repo != prefs.repo || checker.last.is_none_or(|t| t.elapsed() >= PERIOD))
    };
    if due {
        check_now(runtime, ctx, prefs);
    }
}

/// Проверяет сейчас (кнопка в настройках).
pub(crate) fn check_now(
    runtime: &tokio::runtime::Handle,
    ctx: &egui::Context,
    prefs: &UpdatePrefs,
) {
    {
        let mut checker = checker();
        if checker.state == UpdateState::Checking {
            return;
        }
        if !valid_repo(&prefs.repo) {
            checker.state = UpdateState::Failed(
                crate::i18n::tr("укажите репозиторий в виде owner/repo").to_owned(),
            );
            return;
        }
        checker.state = UpdateState::Checking;
        checker.last = Some(Instant::now());
        prefs.repo.clone_into(&mut checker.repo);
    }
    let repo = prefs.repo.clone();
    let prereleases = prefs.prereleases;
    let ctx = ctx.clone();
    runtime.spawn(async move {
        let state = match fetch(&repo, prereleases).await {
            Ok(Some(release)) if newer(&release.version, CURRENT) => {
                UpdateState::Available(release)
            }
            Ok(_) => UpdateState::UpToDate,
            Err(err) => UpdateState::Failed(err),
        };
        let mut checker = checker();
        if checker.repo == repo {
            checker.state = state;
        }
        drop(checker);
        ctx.request_repaint();
    });
}

/// Есть что предложить (новее и не пропущено).
pub(crate) fn offer(prefs: &UpdatePrefs) -> Option<Release> {
    match state() {
        UpdateState::Available(release)
            if prefs.skipped.as_deref() != Some(release.version.as_str()) =>
        {
            Some(release)
        }
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

async fn fetch(repo: &str, prereleases: bool) -> Result<Option<Release>, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("backspace-rs/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|err| err.to_string())?;
    let url = if prereleases {
        format!("https://api.github.com/repos/{repo}/releases?per_page=20")
    } else {
        format!("https://api.github.com/repos/{repo}/releases/latest")
    };
    let response = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // Нет ни одного релиза (или репозиторий закрыт) — обновлять нечем.
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("GitHub: {}", response.status()));
    }
    let releases: Vec<GhRelease> = if prereleases {
        response.json().await.map_err(|err| err.to_string())?
    } else {
        vec![response.json().await.map_err(|err| err.to_string())?]
    };
    Ok(pick(releases, prereleases))
}

/// Самый новый подходящий релиз.
fn pick(releases: Vec<GhRelease>, prereleases: bool) -> Option<Release> {
    releases
        .into_iter()
        .filter(|r| !r.draft && (prereleases || !r.prerelease))
        .filter_map(|r| {
            let version = r.tag_name.trim_start_matches('v').to_owned();
            let parsed = semver::Version::parse(&version).ok()?;
            Some((parsed, version, r))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, version, r)| Release {
            download: asset_for_this_system(&r.assets),
            notes: r
                .body
                .unwrap_or_default()
                .lines()
                .take(12)
                .collect::<Vec<_>>()
                .join("\n"),
            version,
            page: r.html_url,
        })
}

/// Новее ли `candidate`, чем `current` (semver; непарсящееся — нет).
#[must_use]
pub(crate) fn newer(candidate: &str, current: &str) -> bool {
    match (
        semver::Version::parse(candidate.trim_start_matches('v')),
        semver::Version::parse(current.trim_start_matches('v')),
    ) {
        (Ok(candidate), Ok(current)) => candidate > current,
        _ => false,
    }
}

/// Файл релиза под эту систему и архитектуру.
fn asset_for_this_system(assets: &[GhAsset]) -> Option<String> {
    let os_words: &[&str] = if cfg!(target_os = "windows") {
        &["windows", "win64", "win", ".exe", ".msi"]
    } else if cfg!(target_os = "macos") {
        &["macos", "darwin", "mac", ".dmg"]
    } else {
        &["linux", ".appimage", ".deb", ".rpm", ".flatpak"]
    };
    let arch_words: &[&str] = if cfg!(target_arch = "aarch64") {
        &["aarch64", "arm64"]
    } else {
        &["x86_64", "amd64", "x64"]
    };
    let foreign_arch: &[&str] = if cfg!(target_arch = "aarch64") {
        &["x86_64", "amd64", "x64"]
    } else {
        &["aarch64", "arm64"]
    };
    assets
        .iter()
        .filter(|asset| {
            let name = asset.name.to_lowercase();
            let extension = std::path::Path::new(&name)
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or_default();
            !matches!(extension, "sha256" | "sig" | "asc")
                && os_words.iter().any(|w| name.contains(w))
                && !foreign_arch.iter().any(|w| name.contains(w))
        })
        .max_by_key(|asset| {
            let name = asset.name.to_lowercase();
            arch_words.iter().any(|w| name.contains(w))
        })
        .map(|asset| asset.browser_download_url.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool, assets: &[&str]) -> GhRelease {
        GhRelease {
            tag_name: tag.to_owned(),
            html_url: format!("https://example.com/{tag}"),
            body: Some("notes".to_owned()),
            draft: false,
            prerelease,
            assets: assets
                .iter()
                .map(|name| GhAsset {
                    name: (*name).to_owned(),
                    browser_download_url: format!("https://dl/{name}"),
                })
                .collect(),
        }
    }

    #[test]
    fn versions_compare_by_semver() {
        assert!(newer("v0.10.0", "0.9.9"));
        assert!(newer("1.0.0", "1.0.0-rc.1"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("garbage", "0.1.0"));
    }

    #[test]
    fn repo_must_be_owner_slash_name() {
        assert!(valid_repo("quddy/backspace-rs"));
        assert!(!valid_repo("quddy"));
        assert!(!valid_repo("a/b/c"));
        assert!(!valid_repo("https://github.com/a/b"));
        assert!(!valid_repo(""));
    }

    #[test]
    fn newest_stable_release_wins() {
        let picked = pick(
            vec![
                release("v0.2.0", false, &[]),
                release("v0.3.0-beta.1", true, &[]),
                release("v0.2.5", false, &[]),
            ],
            false,
        )
        .expect("релиз");
        assert_eq!(picked.version, "0.2.5");
        let pre = pick(
            vec![
                release("v0.2.5", false, &[]),
                release("v0.3.0-beta.1", true, &[]),
            ],
            true,
        )
        .expect("релиз");
        assert_eq!(pre.version, "0.3.0-beta.1");
    }

    #[test]
    fn asset_matches_this_system() {
        let assets = release(
            "v1.0.0",
            false,
            &[
                "backspace-rs-1.0.0-windows-x86_64.zip",
                "backspace-rs-1.0.0-macos-aarch64.dmg",
                "backspace-rs-1.0.0-linux-x86_64.tar.gz",
                "backspace-rs-1.0.0-linux-aarch64.tar.gz",
                "backspace-rs-1.0.0-linux-x86_64.tar.gz.sha256",
            ],
        )
        .assets;
        let picked = asset_for_this_system(&assets).expect("файл");
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            assert!(picked.ends_with("linux-x86_64.tar.gz"));
        }
    }
}
