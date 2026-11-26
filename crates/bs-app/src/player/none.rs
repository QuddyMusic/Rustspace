//! Сборка без звука: проигрыватель честно говорит, что не умеет.
#![allow(
    clippy::unused_self,
    clippy::needless_pass_by_ref_mut,
    reason = "та же сигнатура, что у настоящих проигрывателей"
)]

use crate::i18n::tr;

#[derive(Default)]
pub(super) struct Backend;

impl Backend {
    pub(super) fn start(&mut self, _bytes: Vec<u8>, _ext: &str) -> Result<(), String> {
        Err(tr("Эта сборка собрана без звука").to_owned())
    }

    pub(super) const fn pause(&self) {}

    pub(super) fn resume(&mut self, _at: f32) -> Result<(), String> {
        Err(tr("Эта сборка собрана без звука").to_owned())
    }

    pub(super) const fn seek(&mut self, _secs: f32) -> bool {
        false
    }

    pub(super) const fn position(&self) -> f32 {
        0.0
    }

    pub(super) const fn len(&self) -> Option<f32> {
        None
    }

    pub(super) const fn finished(&self) -> bool {
        true
    }

    pub(super) const fn stop(&mut self) {}
}
