//! Точка входа GUI-приложения Backspace RS.
//!
//! Этап 14: egui/eframe поверх `bs-core`.
//! Архитектура: tokio-runtime живёт в фоне, GUI-поток владеет кэшем
//! состояния и применяет события, пришедшие через канал (см. [`bridge`]).

#![forbid(unsafe_code)]
// Бинарь: `pub` не нужен (unreachable_pub), а `pub(crate)` внутри приватных
// модулей clippy считает избыточным. Оставляем `pub(crate)` — он честнее.
#![allow(clippy::redundant_pub_crate)]
// В тестах паника — законный способ упасть быстро и с понятным текстом.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod app;
mod brand;
mod bridge;
mod emoji_data;
#[cfg(test)]
mod fake_instance;
mod fleet;
mod icons;
mod images;
mod logic;
mod markdown;
mod media;
mod registry;
mod screens;
mod social;
mod theme;
mod voice;
mod widgets;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            tracing::error!(%err, "не удалось создать tokio runtime");
            return Ok(());
        }
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title("Backspace RS"),
        ..Default::default()
    };

    let handle = runtime.handle().clone();
    let result = eframe::run_native(
        "Backspace RS",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app::BackspaceApp::new(handle)))
        }),
    );

    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}
