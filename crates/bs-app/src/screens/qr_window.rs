//! Окно с QR-кодом ключа двухфакторной защиты: отдельное окно поверх всех,
//! чтобы считать код телефоном, не перекрывая его настройками. QR строит
//! крейт `qrcode` (MIT/Apache-2.0, kennytm).

use egui::RichText;

use crate::i18n::tr;
use crate::theme;

/// Сколько пикселей на модуль QR и пустая рамка вокруг (в модулях).
const MODULE_PX: usize = 8;
const QUIET: usize = 4;

/// Картинка QR-кода (тёмные модули на белом) или `None`, если текст не влез.
#[must_use]
pub(crate) fn image(text: &str) -> Option<egui::ColorImage> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let side = (width + QUIET * 2) * MODULE_PX;
    let mut pixels = vec![egui::Color32::WHITE; side * side];
    for (index, color) in colors.iter().enumerate() {
        if *color != qrcode::Color::Dark {
            continue;
        }
        let (x, y) = (
            (index % width + QUIET) * MODULE_PX,
            (index / width + QUIET) * MODULE_PX,
        );
        for dy in 0..MODULE_PX {
            let row = (y + dy) * side;
            pixels[row + x..row + x + MODULE_PX].fill(egui::Color32::BLACK);
        }
    }
    Some(egui::ColorImage::new([side, side], pixels))
}

/// Текстура QR для текста; готовится один раз на текст.
fn texture(ctx: &egui::Context, text: &str) -> Option<egui::TextureHandle> {
    let id = egui::Id::unique(("two_factor_qr", text));
    if let Some(texture) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(texture);
    }
    let texture = ctx.load_texture("two_factor_qr", image(text)?, egui::TextureOptions::NEAREST);
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    Some(texture)
}

/// Отдельное окно поверх остальных: QR-код ссылки `otpauth`, ключ и кнопки
/// «скопировать». `open` сбрасывается, когда окно закрывают.
pub(crate) fn show(ctx: &egui::Context, open: &mut bool, secret: &str, otpauth: &str) {
    if !*open {
        return;
    }
    let viewport = egui::ViewportId::from_hash_of("two_factor_qr");
    let builder = egui::ViewportBuilder::default()
        .with_title(tr("Rustspace — QR-код для приложения"))
        .with_inner_size([340.0, 470.0])
        .with_resizable(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop);
    let texture = texture(ctx, otpauth);
    ctx.show_viewport_immediate(viewport, builder, |ctx, _class| {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::bg_sidebar())
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(tr("Отсканируйте в приложении-аутентификаторе"))
                            .family(theme::bold_family())
                            .color(theme::text_bright()),
                    );
                    ui.add_space(10.0);
                    match &texture {
                        Some(texture) => {
                            ui.add(
                                egui::Image::new(texture)
                                    .fit_to_exact_size(egui::vec2(260.0, 260.0))
                                    .corner_radius(theme::rounded(8)),
                            );
                        }
                        None => {
                            ui.label(theme::muted(tr("Ссылка слишком длинная для QR-кода.")));
                        }
                    }
                    ui.add_space(10.0);
                    ui.label(theme::muted(tr("Или введите ключ вручную:")).size(12.5));
                    ui.label(
                        RichText::new(secret)
                            .monospace()
                            .color(theme::text_bright()),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let width = 260.0;
                        ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                        if ui.button(tr("Скопировать ключ")).clicked() {
                            ui.ctx().copy_text(secret.to_owned());
                        }
                        if ui.button(tr("Скопировать ссылку otpauth")).clicked() {
                            ui.ctx().copy_text(otpauth.to_owned());
                        }
                    });
                    ui.add_space(8.0);
                    if ui.button(tr("Закрыть")).clicked() {
                        *open = false;
                    }
                });
            });
        if ctx.input(|i| i.viewport().close_requested()) {
            *open = false;
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn otpauth_link_becomes_a_square_code() {
        let image =
            super::image("otpauth://totp/Rustspace:quddy?secret=JBSWY3DPEHPK3PXP&issuer=Rustspace")
                .expect("QR");
        assert_eq!(image.size[0], image.size[1]);
        // Угол — рамка: белый; первый модуль метки позиционирования — чёрный.
        assert_eq!(image.pixels[0], egui::Color32::WHITE);
        let corner = super::QUIET * super::MODULE_PX;
        assert_eq!(
            image.pixels[corner * image.size[0] + corner],
            egui::Color32::BLACK
        );
    }
}
