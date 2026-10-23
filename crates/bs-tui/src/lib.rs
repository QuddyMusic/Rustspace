//! Терминальный интерфейс для проверки протокола Backspace вживую.
//!
//! Крейт сознательно тонкий: вся работа с сетью и состоянием живёт в
//! [`bs_core::CoreSession`], а здесь — только отрисовка ratatui, ввод с
//! клавиатуры и чистые функции раскладки, которые легко тестировать.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod app;
pub mod chat;
pub mod login;
pub mod net;
pub mod social;
pub mod term;
pub mod ui;
