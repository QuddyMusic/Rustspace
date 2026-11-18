//! Точка входа GUI-приложения Rustspace.
//!
//! Этап 14: egui/eframe поверх `bs-core`.
//! Архитектура: tokio-runtime живёт в фоне, GUI-поток владеет кэшем
//! состояния и применяет события, пришедшие через канал (см. [`bridge`]).

#![forbid(unsafe_code)]
// Бинарь: `pub` не нужен (unreachable_pub), а `pub(crate)` внутри приватных
// модулей clippy считает избыточным. Оставляем `pub(crate)` — он честнее.
#![allow(clippy::redundant_pub_crate)]
// Шаблоны перевода `tf!("… {name} …")` подставляются во время работы
// (см. `i18n`), а не `format!`: подстановки в них — не ошибка.
#![allow(clippy::literal_string_with_formatting_args)]
// В тестах паника — законный способ упасть быстро и с понятным текстом.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

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
mod emoji_data;
#[cfg(test)]
mod fake_instance;
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
#[cfg(all(feature = "global-hotkeys", target_os = "linux"))]
mod portal_keys;
mod prefs;
mod presence;
mod registry;
mod saved_session;
mod screens;
mod self_update;
mod share_quality;
mod social;
mod sounds;
mod stage;
mod theme;
mod translate;
mod updates;
mod video_media;
mod voice;
mod voice_media;
mod widgets;
#[cfg(target_os = "linux")]
mod wm;

/// Идентификатор приложения: app_id окна Wayland, имя .desktop-файла и имя
/// для порталов (без точек портал его не примет).
pub(crate) const APP_ID: &str = "io.github.quddymusic.Rustspace";

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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    // Штатное закрытие соединения с PulseAudio крейт пишет как
                    // ERROR, а e2ee LiveKit — служебные INFO на каждый трек.
                    "info,pulseaudio::client::reactor=off,livekit::room::e2ee=warn",
                )
            }),
        )
        .init();
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
            Ok(Box::new(app::BackspaceApp::new(&cc.egui_ctx, handle)))
        }),
    );

    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}
