//! Браузер: `<audio>` с файлом из blob-ссылки; форматы — какие знает браузер.

use web_sys::HtmlAudioElement;

use crate::i18n::tr;

/// Элемент и его blob-ссылка (освобождается при смене дорожки).
#[derive(Default)]
pub(super) struct Backend {
    audio: Option<(HtmlAudioElement, String)>,
}

fn mime(ext: &str) -> &'static str {
    match ext {
        "wav" | "wave" => "audio/wav",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "m4a" | "mp4a" | "aac" => "audio/mp4",
        _ => "",
    }
}

fn js_err(err: &wasm_bindgen::JsValue) -> String {
    err.as_string()
        .unwrap_or_else(|| tr("Браузер не смог открыть файл").to_owned())
}

impl Backend {
    pub(super) fn start(&mut self, bytes: Vec<u8>, ext: &str) -> Result<(), String> {
        self.stop();
        let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes.as_slice()));
        let options = web_sys::BlobPropertyBag::new();
        options.set_type(mime(ext));
        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)
            .map_err(|err| js_err(&err))?;
        let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|err| js_err(&err))?;
        let audio = HtmlAudioElement::new_with_src(&url).map_err(|err| js_err(&err))?;
        if !mime(ext).is_empty() && audio.can_play_type(mime(ext)).is_empty() {
            let _ = web_sys::Url::revoke_object_url(&url);
            return Err(tr("Браузер не умеет этот формат").to_owned());
        }
        let _ = audio.play();
        self.audio = Some((audio, url));
        Ok(())
    }

    pub(super) fn pause(&self) {
        if let Some((audio, _)) = &self.audio {
            let _ = audio.pause();
        }
    }

    pub(super) fn resume(&self, at: f32) -> Result<(), String> {
        let (audio, _) = self
            .audio
            .as_ref()
            .ok_or_else(|| tr("Нет файла").to_owned())?;
        if audio.ended() {
            audio.set_current_time(f64::from(at));
        }
        let _ = audio.play();
        Ok(())
    }

    pub(super) fn seek(&self, secs: f32) -> bool {
        self.audio.as_ref().is_some_and(|(audio, _)| {
            audio.set_current_time(f64::from(secs));
            true
        })
    }

    #[allow(clippy::cast_possible_truncation, reason = "секунды дорожки")]
    pub(super) fn position(&self) -> f32 {
        self.audio
            .as_ref()
            .map_or(0.0, |(audio, _)| audio.current_time() as f32)
    }

    #[allow(clippy::cast_possible_truncation, reason = "секунды дорожки")]
    pub(super) fn len(&self) -> Option<f32> {
        let duration = self.audio.as_ref()?.0.duration();
        duration.is_finite().then_some(duration as f32)
    }

    pub(super) fn finished(&self) -> bool {
        self.audio.as_ref().is_some_and(|(audio, _)| audio.ended())
    }

    pub(super) fn stop(&mut self) {
        if let Some((audio, url)) = self.audio.take() {
            let _ = audio.pause();
            let _ = web_sys::Url::revoke_object_url(&url);
        }
    }
}
