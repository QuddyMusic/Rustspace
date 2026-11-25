//! Превью трансляции при наведении на голосовой канал: кто показывает экран,
//! раз в несколько секунд кладёт на сервер маленький кадр, остальные видят
//! его во всплывающем окне, не заходя в канал.

use std::sync::Arc;

use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;

/// Как часто обновляем свой кадр.
pub(crate) const UPLOAD_EVERY: std::time::Duration = std::time::Duration::from_secs(8);
/// Ширина кадра превью.
const WIDTH: u32 = 320;
/// Ширина картинки во всплывающем окне.
const SHOWN_WIDTH: f32 = 288.0;

/// Наведение на трансляцию в списке каналов.
#[derive(Debug, Clone)]
pub(crate) struct Hover {
    /// Инстанс.
    pub(crate) origin: usize,
    /// Голосовой канал.
    pub(crate) channel_id: String,
    /// Кто транслирует.
    pub(crate) user_id: String,
    /// Его имя.
    pub(crate) name: String,
    /// Строка, на которую навели.
    pub(crate) rect: egui::Rect,
}

fn hover_id() -> egui::Id {
    egui::Id::unique("stream_preview_hover")
}

/// Отмечает наведение на трансляцию (окно рисуется на следующем кадре,
/// там, где есть доступ к сессии).
pub(crate) fn hovered(ctx: &egui::Context, hover: Hover) {
    ctx.data_mut(|data| data.insert_temp(hover_id(), hover));
}

/// Маленький JPEG из кадра: ширина [`WIDTH`], пропорции сохраняются.
#[must_use]
pub(crate) fn snapshot(size: [usize; 2], rgba: &[u8]) -> Option<Vec<u8>> {
    let width = u32::try_from(size[0]).ok()?;
    let height = u32::try_from(size[1]).ok()?;
    let frame = image::RgbaImage::from_raw(width, height, rgba.to_vec())?;
    let scaled_width = width.clamp(1, WIDTH);
    let scaled_height = (u64::from(height) * u64::from(scaled_width) / u64::from(width.max(1)))
        .clamp(1, u64::from(WIDTH));
    let small =
        image::imageops::thumbnail(&frame, scaled_width, u32::try_from(scaled_height).ok()?);
    let rgb = image::DynamicImage::ImageRgba8(small).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 70)
        .encode_image(&rgb)
        .ok()?;
    (out.len() <= bs_api::stream_preview::MAX_PREVIEW_BYTES).then_some(out)
}

/// Текстура кадра: декодируется один раз на поколение.
fn texture(
    ctx: &egui::Context,
    key: &(String, String),
    jpeg: &Arc<[u8]>,
    generation: u64,
) -> Option<egui::TextureHandle> {
    let id = egui::Id::unique(("stream_preview_texture", key));
    if let Some((cached, texture)) =
        ctx.data(|data| data.get_temp::<(u64, egui::TextureHandle)>(id))
        && cached == generation
    {
        return Some(texture);
    }
    let rgba = image::load_from_memory(jpeg).ok()?.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    let texture = ctx.load_texture(
        format!("stream_preview_{}_{}", key.0, key.1),
        image,
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|data| data.insert_temp(id, (generation, texture.clone())));
    Some(texture)
}

/// Рисует всплывающее окно превью, если на прошлом кадре навели на
/// трансляцию.
pub(crate) fn show(ctx: &egui::Context, bridge: &mut Bridge) {
    let Some(hover) = ctx.data_mut(|data| {
        let hover = data.get_temp::<Hover>(hover_id());
        data.remove::<Hover>(hover_id());
        hover
    }) else {
        return;
    };
    let frame = bridge
        .e2ee_session(hover.origin)
        .and_then(|session| session.stream_preview(&hover.channel_id, &hover.user_id));
    let key = (hover.channel_id.clone(), hover.user_id.clone());
    let texture = frame.and_then(|(jpeg, generation)| texture(ctx, &key, &jpeg, generation));
    egui::Area::new(egui::Id::unique("stream_preview_popup"))
        .order(egui::Order::Tooltip)
        .fixed_pos(hover.rect.right_top() + egui::vec2(8.0, 0.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(SHOWN_WIDTH);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(tr("В ЭФИРЕ"))
                            .small()
                            .strong()
                            .color(theme::red()),
                    );
                    ui.label(egui::RichText::new(&hover.name).strong());
                });
                ui.add_space(4.0);
                match texture {
                    Some(texture) => {
                        let size = texture.size_vec2();
                        let scale = SHOWN_WIDTH / size.x.max(1.0);
                        ui.add(
                            egui::Image::new(&texture)
                                .fit_to_exact_size(size * scale)
                                .corner_radius(6.0),
                        );
                    }
                    None => {
                        ui.label(theme::muted(tr("Превью трансляции пока нет")));
                    }
                }
            });
        });
    ctx.request_repaint_after(std::time::Duration::from_secs(1));
}

#[cfg(test)]
mod tests {
    use super::snapshot;

    #[test]
    fn snapshot_is_small_jpeg() {
        let rgba = vec![200_u8; 1280 * 720 * 4];
        let jpeg = snapshot([1280, 720], &rgba).expect("кадр");
        assert!(jpeg.starts_with(&[0xFF, 0xD8, 0xFF]));
        let decoded = image::load_from_memory(&jpeg).expect("jpeg");
        assert_eq!((decoded.width(), decoded.height()), (320, 180));
        assert!(snapshot([10, 10], &[0; 3]).is_none());
    }
}
