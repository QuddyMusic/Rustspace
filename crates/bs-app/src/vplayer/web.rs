//! Браузер: его `<video>` (свои декодеры, включая аппаратные), кадр
//! переносится в текстуру через `<canvas>`.

#![allow(
    clippy::needless_pass_by_ref_mut,
    reason = "та же сигнатура, что у проигрывателя на десктопе"
)]

use std::sync::Arc;

use egui::Context;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlVideoElement};

use super::{Frame, VideoHub};
use crate::i18n::tr;

/// Кадры крупнее уменьшаем: копирование через canvas недёшево.
const MAX_SIDE: u32 = 960;

#[derive(Default)]
pub(super) struct Backend {
    video: Option<(HtmlVideoElement, String)>,
    canvas: Option<(HtmlCanvasElement, CanvasRenderingContext2d)>,
    shown_at: f64,
}

fn js_err(err: &wasm_bindgen::JsValue) -> String {
    err.as_string()
        .unwrap_or_else(|| tr("Браузер не смог открыть файл").to_owned())
}

impl Backend {
    pub(super) fn start(&mut self, bytes: Arc<[u8]>, _name: &str) -> Result<(), String> {
        self.stop();
        let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(&bytes[..]));
        let blob = web_sys::Blob::new_with_u8_array_sequence(&parts).map_err(|err| js_err(&err))?;
        let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|err| js_err(&err))?;
        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or_else(|| tr("Браузер не смог открыть файл").to_owned())?;
        let video: HtmlVideoElement = document
            .create_element("video")
            .map_err(|err| js_err(&err))?
            .dyn_into()
            .map_err(|_| tr("Браузер не смог открыть файл").to_owned())?;
        video.set_src(&url);
        video.set_preload("auto");
        let _ = video.play();
        self.video = Some((video, url));
        self.shown_at = -1.0;
        Ok(())
    }

    fn element(&self) -> Option<&HtmlVideoElement> {
        self.video.as_ref().map(|(video, _)| video)
    }

    pub(super) fn pause(&mut self) {
        if let Some(video) = self.element() {
            let _ = video.pause();
        }
    }

    pub(super) fn resume(&mut self) {
        if let Some(video) = self.element() {
            let _ = video.play();
        }
    }

    pub(super) fn seek(&mut self, secs: f32) {
        if let Some(video) = self.element() {
            video.set_current_time(f64::from(secs.max(0.0)));
        }
    }

    pub(super) fn rewind(&mut self) {
        self.seek(0.0);
        self.pause();
    }

    pub(super) fn stop(&mut self) {
        if let Some((video, url)) = self.video.take() {
            let _ = video.pause();
            video.set_src("");
            let _ = web_sys::Url::revoke_object_url(&url);
        }
    }

    #[allow(clippy::cast_possible_truncation, reason = "секунды видео")]
    pub(super) fn position(&self) -> f32 {
        self.element()
            .map_or(0.0, |video| video.current_time() as f32)
    }

    #[allow(clippy::cast_possible_truncation, reason = "секунды видео")]
    pub(super) fn len(&self) -> Option<f32> {
        let duration = self.element()?.duration();
        duration.is_finite().then_some(duration as f32)
    }

    pub(super) fn finished(&self, _pos: f32) -> bool {
        self.element().is_some_and(|video| video.ended())
    }

    /// Кадр, если с прошлого раза видео сдвинулось.
    pub(super) fn frame(&mut self, _pos: f32) -> Option<Frame> {
        let video = self.element()?.clone();
        if video.ready_state() < 2 {
            return None;
        }
        let at = video.current_time();
        if (at - self.shown_at).abs() < 1e-3 {
            return None;
        }
        let (w, h) = (video.video_width(), video.video_height());
        if w == 0 || h == 0 {
            return None;
        }
        let scale = (f64::from(MAX_SIDE) / f64::from(w.max(h))).min(1.0);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "размер кадра"
        )]
        let (width, height) = (
            (f64::from(w) * scale).round().max(1.0) as u32,
            (f64::from(h) * scale).round().max(1.0) as u32,
        );
        let context = self.canvas(width, height)?;
        context
            .draw_image_with_html_video_element_and_dw_and_dh(
                &video,
                0.0,
                0.0,
                f64::from(width),
                f64::from(height),
            )
            .ok()?;
        let data = context
            .get_image_data(0.0, 0.0, f64::from(width), f64::from(height))
            .ok()?;
        self.shown_at = at;
        Some(Frame {
            width,
            height,
            rgba: data.data().0,
        })
    }

    fn canvas(&mut self, width: u32, height: u32) -> Option<CanvasRenderingContext2d> {
        if self.canvas.is_none() {
            let canvas: HtmlCanvasElement = web_sys::window()?
                .document()?
                .create_element("canvas")
                .ok()?
                .dyn_into()
                .ok()?;
            let context: CanvasRenderingContext2d =
                canvas.get_context("2d").ok()??.dyn_into().ok()?;
            self.canvas = Some((canvas, context));
        }
        let (canvas, context) = self.canvas.as_ref()?;
        if canvas.width() != width || canvas.height() != height {
            canvas.set_width(width);
            canvas.set_height(height);
        }
        Some(context.clone())
    }

    /// В браузере превью без сервера не делаем.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "та же сигнатура, что на десктопе"
    )]
    pub(super) fn thumbnail(&mut self, _file: String, _bytes: Arc<[u8]>) {}

    pub(super) const fn poll_thumbnails(&mut self, _ctx: &Context, _hub: &VideoHub) {}
}
