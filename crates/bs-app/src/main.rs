//! Точка входа GUI-приложения Rustspace.
//!
//! Этап 14: egui/eframe поверх `bs-core`.
//! Архитектура: tokio-runtime живёт в фоне, GUI-поток владеет кэшем
//! состояния и применяет события, пришедшие через канал (см. [`bridge`]).

#![forbid(unsafe_code)]
// Windows: без окна консоли.
#![cfg_attr(windows, windows_subsystem = "windows")]
// Бинарь: `pub` не нужен (unreachable_pub), а `pub(crate)` внутри приватных
// модулей clippy считает избыточным. Оставляем `pub(crate)` — он честнее.
#![allow(clippy::redundant_pub_crate)]
// В браузере задачи живут в одном потоке, а настольные службы (трей,
// игры, локальные модели, самообновление) не собираются в работу.
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        clippy::future_not_send,
        dead_code,
        unused_imports,
        unused_mut,
        clippy::unused_async,
        clippy::unused_self,
        clippy::needless_pass_by_value,
        reason = "веб-версия без настольных служб"
    )
)]
// Шаблоны перевода `tf!("… {name} …")` подставляются во время работы
// (см. `i18n`), а не `format!`: подстановки в них — не ошибка.
#![allow(clippy::literal_string_with_formatting_args)]
// В тестах паника — законный способ упасть быстро и с понятным текстом.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod accounts;
mod app;
mod brand;
mod bridge;
mod clipboard;
mod columns;
mod desktop;
#[cfg(target_os = "linux")]
mod desktop_entry;
mod dmcall;
mod download;
mod e2ee_files;
mod emoji_data;
#[cfg(test)]
mod fake_instance;
mod files;
mod fleet;
mod gif_favorites;
mod highlight;
mod hotkeys;
mod i18n;
mod icons;
mod images;
mod local_llm;
mod logic;
mod markdown;
mod media;
mod notify;
mod offline_mt;
mod player;
#[cfg(all(feature = "global-hotkeys", target_os = "linux"))]
mod portal_keys;
mod prefs;
mod presence;
mod registry;
mod rt;
mod saved_session;
mod screens;
mod self_update;
mod share_quality;
mod social;
mod sounds;
mod stage;
mod stream_preview;
mod sys;
mod theme;
mod translate;
mod updates;
mod video_media;
mod voice;
mod voice_media;
mod widgets;
#[cfg(windows)]
mod windows_app;
#[cfg(target_os = "linux")]
mod wm;

/// Идентификатор приложения: app_id окна Wayland, имя .desktop-файла и имя
/// для порталов (без точек портал его не примет).
pub(crate) const APP_ID: &str = "io.github.quddymusic.Rustspace";

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--version" || arg == "-V")
    {
        #[allow(clippy::print_stdout, reason = "ответ на --version")]
        {
            println!("rustspace {}", env!("CARGO_PKG_VERSION"));
        }
        return Ok(());
    }
    let logging = tracing_subscriber::fmt().with_env_filter(
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            tracing_subscriber::EnvFilter::new(
                // Штатное закрытие соединения с PulseAudio крейт пишет как
                // ERROR, а e2ee LiveKit — служебные INFO на каждый трек.
                "info,pulseaudio::client::reactor=off,livekit::room::e2ee=warn",
            )
        }),
    );
    // Windows: консоли нет — журнал в файл (если не открылся — в stderr).
    #[cfg(windows)]
    {
        let (writer, ansi) = windows_app::log_file().map_or_else(
            || {
                (
                    tracing_subscriber::fmt::writer::BoxMakeWriter::new(std::io::stderr),
                    true,
                )
            },
            |file| {
                (
                    tracing_subscriber::fmt::writer::BoxMakeWriter::new(std::sync::Mutex::new(
                        file,
                    )),
                    false,
                )
            },
        );
        logging.with_ansi(ansi).with_writer(writer).init();
    }
    #[cfg(not(windows))]
    logging.init();
    #[cfg(windows)]
    windows_app::register();
    saved_session::migrate_legacy_dir();
    #[cfg(target_os = "linux")]
    desktop_entry::ensure();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            tracing::error!(%err, "не удалось создать tokio runtime");
            return Ok(());
        }
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([720.0, 480.0])
        .with_title(crate::brand::NAME)
        // Совпадает с StartupWMClass в .desktop: окно получает иконку панели.
        .with_app_id(APP_ID);
    match eframe::icon_data::from_png_bytes(include_bytes!(
        "../../../packaging/linux/rustspace.png"
    )) {
        Ok(icon) => viewport = viewport.with_icon(icon),
        Err(err) => tracing::warn!(%err, "иконка окна не загружена"),
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let handle = runtime.handle().clone();
    let result = eframe::run_native(
        crate::brand::NAME,
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(app::BackspaceApp::new(
                &cc.egui_ctx,
                rt::Handle::new(handle),
            )))
        }),
    );

    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}

/// Веб-версия: приложение рисуется на `<canvas id="rustspace">` страницы.
#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast as _;
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&info.to_string().into());
    }));
    let runner = eframe::WebRunner::new();
    wasm_bindgen_futures::spawn_local(async move {
        let Some(canvas) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id("rustspace"))
            .and_then(|element| element.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        else {
            return;
        };
        let started = runner
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| {
                    theme::install(&cc.egui_ctx);
                    Ok(Box::new(app::BackspaceApp::new(
                        &cc.egui_ctx,
                        rt::Handle::new(),
                    )))
                }),
            )
            .await;
        if let Err(err) = started {
            tracing::error!(?err, "приложение не запустилось");
        }
    });
}
