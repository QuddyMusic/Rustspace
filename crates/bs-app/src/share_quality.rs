//! Качество демонстрации экрана и ограничения инстанса.
//!
//! Повторяет `utils/screenShare.ts` оригинала: сохранённый выбор
//! пользователя (`screenShareConfig`) подгоняется под ограничения того
//! инстанса, который выдал токен LiveKit, — разрешения, частоты, свой
//! битрейт и его пределы. Сам выбор при этом не переписывается: строгий
//! хост не должен портить настройку для всех следующих трансляций.

use bs_api::StreamingLimits;
use serde_json::Value;

/// Стандартные высоты кадра.
pub(crate) const RESOLUTIONS: [u32; 5] = [540, 720, 1080, 1440, 2160];
/// Стандартные частоты кадров.
pub(crate) const FRAMERATES: [u32; 6] = [30, 45, 60, 75, 90, 120];
/// Высота «как есть» (`'native'` оригинала).
pub(crate) const NATIVE: u32 = 0;

/// Ширина для стандартной высоты (16:9).
const fn width_of(height: u32) -> u32 {
    match height {
        540 => 960,
        720 => 1280,
        1440 => 2560,
        2160 => 3840,
        _ => 1920,
    }
}

/// Битрейт по умолчанию для ячейки «высота × частота», кбит/с.
fn matrix_kbps(height: u32, fps: u32) -> u32 {
    let row: [u32; 6] = match height {
        540 => [1500, 2000, 2500, 2800, 3200, 4000],
        720 => [3000, 3500, 4000, 4500, 5000, 6000],
        1080 => [6000, 7000, 8000, 9000, 10000, 12000],
        1440 => [10000, 12000, 14000, 16000, 18000, 22000],
        2160 => [20000, 24000, 28000, 32000, 38000, 45000],
        _ => return 8000,
    };
    FRAMERATES
        .iter()
        .position(|f| *f == fps)
        .map_or(8000, |index| row[index])
}

/// Ограничения, когда хост их не сообщил (`DEFAULT_STREAMING_LIMITS`).
#[must_use]
pub(crate) fn default_limits() -> StreamingLimits {
    StreamingLimits {
        max_bitrate_kbps: 20_000,
        min_bitrate_kbps: 500,
        bitrate_step_kbps: 500,
        allowed_resolutions: [540, 720, 1080].map(Value::from).to_vec(),
        allowed_framerates: vec![30, 45, 60],
        max_resolution: 1080,
        max_framerate: 60,
        allow_custom_bitrate: true,
        bitrate_matrix_overrides: None,
    }
}

/// Разрешённые высоты хоста (`NATIVE` — «как есть»).
#[must_use]
pub(crate) fn allowed_heights(limits: &StreamingLimits) -> Vec<u32> {
    limits
        .allowed_resolutions
        .iter()
        .filter_map(|value| match value {
            Value::String(text) if text == "native" => Some(NATIVE),
            other => other
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| RESOLUTIONS.contains(n)),
        })
        .collect()
}

/// Разрешённые частоты хоста.
#[must_use]
pub(crate) fn allowed_rates(limits: &StreamingLimits) -> Vec<u32> {
    limits
        .allowed_framerates
        .iter()
        .copied()
        .filter(|f| FRAMERATES.contains(f))
        .collect()
}

/// Что беречь при нехватке канала.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ShareMode {
    /// Игры и видео: плавность.
    #[default]
    Gaming,
    /// Текст и код: чёткость.
    Detail,
}

/// Кодек трансляции.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ShareCodec {
    /// VP9 — лучше картинка на том же битрейте.
    #[default]
    Vp9,
    /// H264 — аппаратное кодирование почти везде.
    H264,
}

/// Сохранённый выбор качества (`screenShareConfig` оригинала).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ShareQuality {
    /// Высота кадра; [`NATIVE`] — без уменьшения.
    pub(crate) height: u32,
    /// Кадров в секунду.
    pub(crate) fps: u32,
    /// Плавность или чёткость.
    pub(crate) mode: ShareMode,
    /// Свой битрейт, кбит/с; `None` — по таблице.
    pub(crate) custom_bitrate_kbps: Option<u32>,
    /// Кодек.
    pub(crate) codec: ShareCodec,
    /// Транслировать системный звук.
    pub(crate) share_audio: bool,
}

impl Default for ShareQuality {
    fn default() -> Self {
        Self {
            height: 720,
            fps: 60,
            mode: ShareMode::Gaming,
            custom_bitrate_kbps: None,
            codec: ShareCodec::Vp9,
            share_audio: true,
        }
    }
}

/// Ближайшее к `target` значение из непустого списка.
fn nearest(values: &[u32], target: u32) -> u32 {
    values
        .iter()
        .copied()
        .min_by_key(|v| v.abs_diff(target))
        .unwrap_or(target)
}

/// Битрейт, который трансляция реально получит: всё уже подогнано.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShareSettings {
    /// Предел ширины и высоты кадра.
    pub(crate) max_size: (u32, u32),
    /// Кадров в секунду.
    pub(crate) fps: u32,
    /// Битрейт, бит/с.
    pub(crate) bitrate: u64,
    /// Плавность или чёткость.
    pub(crate) mode: ShareMode,
    /// Кодек.
    pub(crate) codec: ShareCodec,
    /// Транслировать системный звук.
    pub(crate) audio: bool,
}

impl ShareQuality {
    /// Чинит значения из старых или испорченных настроек.
    pub(crate) fn sanitize(&mut self) {
        if self.height != NATIVE && !RESOLUTIONS.contains(&self.height) {
            self.height = nearest(&RESOLUTIONS, self.height);
        }
        if !FRAMERATES.contains(&self.fps) {
            self.fps = nearest(&FRAMERATES, self.fps);
        }
    }

    /// Выбор, подогнанный под ограничения хоста (`effectiveScreenShareConfig`).
    #[must_use]
    pub(crate) fn effective(self, limits: &StreamingLimits) -> Self {
        let mut out = self;
        let heights = allowed_heights(limits);
        if !heights.is_empty() && !heights.contains(&out.height) {
            let numeric: Vec<u32> = heights.iter().copied().filter(|h| *h != NATIVE).collect();
            out.height = if numeric.is_empty() {
                NATIVE
            } else if out.height == NATIVE {
                numeric.iter().copied().max().unwrap_or(1080)
            } else {
                nearest(&numeric, out.height)
            };
        }
        let rates = allowed_rates(limits);
        if !rates.is_empty() && !rates.contains(&out.fps) {
            out.fps = nearest(&rates, out.fps);
        }
        if let Some(kbps) = out.custom_bitrate_kbps {
            out.custom_bitrate_kbps = limits.allow_custom_bitrate.then(|| {
                kbps.clamp(
                    limits.min_bitrate_kbps,
                    limits.max_bitrate_kbps.max(limits.min_bitrate_kbps),
                )
            });
        }
        out
    }

    /// Настройки трансляции на этом хосте (`buildScreenShareOptions`).
    #[must_use]
    pub(crate) fn settings(self, limits: &StreamingLimits) -> ShareSettings {
        let config = self.effective(limits);
        let overrides = limits.bitrate_matrix_overrides.as_ref();
        let cell = |height: u32, fps: u32| -> u32 {
            overrides
                .and_then(|map| map.get(&format!("{height}_{fps}")))
                .and_then(Value::as_f64)
                .filter(|kbps| *kbps > 0.0)
                .map_or_else(
                    || matrix_kbps(height, fps),
                    // Значения проверены сервером: положительные и не огромные.
                    |kbps| {
                        #[allow(
                            clippy::cast_possible_truncation,
                            clippy::cast_sign_loss,
                            reason = "кбит/с с сервера в пределах u32"
                        )]
                        let kbps = kbps.round() as u32;
                        kbps
                    },
                )
        };
        let native = config.height == NATIVE;
        let raw = config.custom_bitrate_kbps.unwrap_or_else(|| {
            if native {
                cell(2160, nearest(&FRAMERATES, config.fps))
            } else {
                cell(config.height, config.fps)
            }
        });
        let low = limits.min_bitrate_kbps;
        let high = limits.max_bitrate_kbps.max(low);
        let kbps = if high == 0 { raw } else { raw.clamp(low, high) };
        let max_size = if native {
            (7680, 4320)
        } else {
            // Ширина с запасом под широкие мониторы: пропорции сохраняются.
            (width_of(config.height) * 2, config.height)
        };
        ShareSettings {
            max_size,
            fps: config.fps,
            bitrate: u64::from(kbps) * 1000,
            mode: config.mode,
            codec: config.codec,
            audio: config.share_audio,
        }
    }

    /// Подпись высоты.
    #[must_use]
    pub(crate) fn height_label(height: u32) -> String {
        match height {
            NATIVE => crate::i18n::tr("Как есть").to_owned(),
            2160 => "4K".to_owned(),
            other => format!("{other}p"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(resolutions: &[Value], rates: &[u32]) -> StreamingLimits {
        StreamingLimits {
            allowed_resolutions: resolutions.to_vec(),
            allowed_framerates: rates.to_vec(),
            ..default_limits()
        }
    }

    #[test]
    fn effective_snaps_to_allowed_values() {
        let quality = ShareQuality {
            height: 1440,
            fps: 120,
            ..ShareQuality::default()
        };
        let fitted = quality.effective(&default_limits());
        assert_eq!((fitted.height, fitted.fps), (1080, 60));
        // Сам выбор не трогаем.
        assert_eq!(quality.height, 1440);
    }

    #[test]
    fn native_falls_back_to_highest_numeric() {
        let quality = ShareQuality {
            height: NATIVE,
            ..ShareQuality::default()
        };
        assert_eq!(quality.effective(&default_limits()).height, 1080);
        let only_native = limits(&[Value::from("native")], &[30]);
        assert_eq!(
            ShareQuality::default().effective(&only_native).height,
            NATIVE
        );
    }

    #[test]
    fn custom_bitrate_is_clamped_or_dropped() {
        let quality = ShareQuality {
            custom_bitrate_kbps: Some(90_000),
            ..ShareQuality::default()
        };
        assert_eq!(
            quality.effective(&default_limits()).custom_bitrate_kbps,
            Some(20_000)
        );
        let strict = StreamingLimits {
            allow_custom_bitrate: false,
            ..default_limits()
        };
        assert_eq!(quality.effective(&strict).custom_bitrate_kbps, None);
    }

    #[test]
    fn bitrate_follows_matrix_and_limits() {
        let quality = ShareQuality {
            height: 1080,
            fps: 60,
            ..ShareQuality::default()
        };
        assert_eq!(quality.settings(&default_limits()).bitrate, 8_000_000);
        let capped = StreamingLimits {
            max_bitrate_kbps: 3000,
            ..default_limits()
        };
        assert_eq!(quality.settings(&capped).bitrate, 3_000_000);
        let mut overridden = default_limits();
        overridden.bitrate_matrix_overrides =
            Some(std::iter::once(("1080_60".to_owned(), Value::from(5000))).collect());
        assert_eq!(quality.settings(&overridden).bitrate, 5_000_000);
    }

    #[test]
    fn sanitize_repairs_old_prefs() {
        let mut quality = ShareQuality {
            height: 700,
            fps: 15,
            ..ShareQuality::default()
        };
        quality.sanitize();
        assert_eq!((quality.height, quality.fps), (720, 30));
    }

    #[test]
    fn old_prefs_still_parse() {
        let quality: ShareQuality =
            serde_json::from_str(r#"{"height":1080,"fps":30}"#).expect("старый формат");
        assert_eq!(quality.height, 1080);
        assert_eq!(quality.codec, ShareCodec::Vp9);
    }
}
