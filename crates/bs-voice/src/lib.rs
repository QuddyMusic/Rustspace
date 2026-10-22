//! Голос/видео через LiveKit.
//!
//! Этап 0: каркас. Реализация — Этап 4. Главный риск: у LiveKit нет Rust SDK.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

/// Заглушка, чтобы крейт собирался на этапе 0.
pub const STAGE: &str = "0";
