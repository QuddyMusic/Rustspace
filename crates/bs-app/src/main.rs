//! Точка входа GUI-приложения Backspace RS.
//!
//! Этап 14: egui/eframe поверх `bs-core`.
//! Архитектура: tokio-runtime живёт в фоне, GUI-поток владеет кэшем
//! состояния и применяет события, пришедшие через канал (см. [`bridge`]).

#![forbid(unsafe_code)]
// Бинарь: `pub` не нужен (unreachable_pub), а `pub(crate)` внутри приватных
// модулей clippy считает избыточным. Оставляем `pub(crate)` — он честнее.
#![allow(clippy::redundant_pub_crate)]

mod app;
mod bridge;
#[cfg(test)]
mod fake_instance;
mod logic;
mod screens;
mod social;

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
        Box::new(move |_cc| Ok(Box::new(app::BackspaceApp::new(handle)))),
    );

    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}
