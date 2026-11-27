//! Видео в ленте и на всё окно: превью, кнопка «играть», полоса перемотки.
//!
//! Строка сообщения перекрывает виджеты внутри, поэтому щелчки ловим по
//! положению указателя (как у аудио и кнопки скачивания).

use egui::epaint::{Mesh, RectShape, Vertex};
use egui::{Align2, Color32, FontId, Rect, Sense, TextureHandle, Vec2, pos2};

use crate::images::{ImageHub, ImageState, cover_uv, paint_cover};
use crate::media::{
    AttachmentView, DL_BUTTON, DownloadHit, download_badge, download_of, f32_of, fit, human_size,
    target_of,
};
use crate::player::clock;
use crate::theme;
use crate::vplayer::{Status, VideoHub, View, is_round};

/// Наибольший размер видео в ленте.
const MAX_BOX: Vec2 = Vec2::new(380.0, 300.0);
/// Диаметр «кружочка».
const ROUND: f32 = 220.0;
/// Высота полосы перемотки снизу (зона щелчка).
const BAR: f32 = 18.0;

/// Что нарисовать вместо кадра, пока видео не играет.
enum Still {
    Server(std::sync::Arc<crate::images::Loaded>),
    Texture(TextureHandle),
    Nothing,
}

/// Карточка видео в ленте.
pub(crate) fn video_card(
    ui: &mut egui::Ui,
    att: &AttachmentView,
    downloads: &mut Vec<DownloadHit>,
) {
    let round = is_round(&att.name);
    let hub = VideoHub::of(ui.ctx());
    let view = hub.as_ref().and_then(|hub| hub.view(&att.filename));
    let still = still_of(ui, att, hub.as_ref());
    let natural = view
        .as_ref()
        .and_then(|view| view.frame.as_ref())
        .map(texture_size)
        .or_else(|| att.dims.map(|(w, h)| [f32_of(w), f32_of(h)]))
        .or_else(|| match &still {
            Still::Server(image) => Some(image.size_f32()),
            Still::Texture(texture) => Some(texture_size(texture)),
            Still::Nothing => None,
        })
        .unwrap_or([320.0, 180.0]);
    let size = if round {
        Vec2::splat(ROUND)
    } else {
        fit(natural, MAX_BOX).max(Vec2::new(160.0, 90.0))
    };
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    match (view.as_ref().and_then(|view| view.frame.as_ref()), &still) {
        (Some(frame), _) | (None, Still::Texture(frame)) => {
            paint_texture(painter, rect, frame, round);
        }
        (None, Still::Server(image)) if round => paint_texture(painter, rect, image.first(), true),
        (None, Still::Server(image)) => {
            paint_cover(
                painter,
                rect,
                image,
                theme::rounded(10),
                false,
                Color32::WHITE,
            );
        }
        (None, Still::Nothing) => {
            if round {
                painter.circle_filled(rect.center(), rect.width() / 2.0, Color32::from_gray(24));
            } else {
                painter.rect_filled(rect, theme::rounded(10), Color32::from_gray(24));
            }
        }
    }
    let hover = ui.rect_contains_pointer(rect);
    let status = view.as_ref().map(|view| view.status.clone());
    if status != Some(Status::Playing) || hover {
        play_button(
            ui,
            rect.center(),
            status.as_ref(),
            status == Some(Status::Playing),
        );
    }
    let bar = Rect::from_min_max(
        pos2(rect.min.x + 10.0, rect.max.y - BAR),
        pos2(rect.max.x - 10.0, rect.max.y),
    );
    caption(ui, rect, att, view.as_ref(), round);
    match &view {
        Some(view) if round => ring(ui, rect, view),
        Some(view) => progress(ui, bar, view),
        None => {}
    }
    // Кнопки в углах: скачать — справа, на всё окно — слева.
    let download = Rect::from_min_size(
        pos2(rect.max.x - DL_BUTTON - 8.0, rect.min.y + 8.0),
        Vec2::splat(DL_BUTTON),
    );
    let expand = Rect::from_min_size(rect.min + Vec2::splat(8.0), Vec2::splat(DL_BUTTON));
    let state = download_of(ui, att);
    if !round && (hover || state.is_some()) {
        download_badge(
            ui,
            download,
            ui.auto_id_with(("dl", &att.filename)),
            state.as_ref(),
            true,
        );
        downloads.push((download, crate::download::Click::One(target_of(att))));
    }
    let can_expand = !round && view.as_ref().is_some_and(|view| view.frame.is_some());
    if can_expand && hover {
        corner_button(ui, expand, "⛶");
    }
    if hover {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if ui.input(|i| i.pointer.primary_clicked()) && hover {
        let zones = Zones {
            download: (!round).then_some(download),
            expand: can_expand.then_some(expand),
            bar: (!round).then_some(bar),
        };
        click(ui, hub.as_ref(), &att.filename, view.as_ref(), &zones);
    }
}

/// Где на карточке что нажимать.
struct Zones {
    download: Option<Rect>,
    expand: Option<Rect>,
    bar: Option<Rect>,
}

/// Щелчок по карточке видео.
fn click(ui: &egui::Ui, hub: Option<&VideoHub>, file: &str, view: Option<&View>, zones: &Zones) {
    let Some(hub) = hub else { return };
    let inside = |rect: Option<Rect>| rect.is_some_and(|rect| ui.rect_contains_pointer(rect));
    if inside(zones.download) {
        return;
    }
    if inside(zones.expand) {
        hub.expand(true);
    } else if let (true, Some(bar), Some(len), Some(pos)) = (
        inside(zones.bar),
        zones.bar,
        view.and_then(|view| view.len),
        ui.ctx().pointer_interact_pos(),
    ) {
        let frac = ((pos.x - bar.min.x) / bar.width()).clamp(0.0, 1.0);
        hub.seek(file, frac * len);
    } else {
        hub.toggle(file);
    }
}

/// Превью: от сервера или сделанное клиентом.
fn still_of(ui: &egui::Ui, att: &AttachmentView, hub: Option<&VideoHub>) -> Still {
    if let Some(thumb) = &att.thumbnail
        && let Some(images) = ImageHub::of(ui.ctx())
    {
        return match images.state(thumb) {
            ImageState::Ready(image) => Still::Server(image),
            _ => Still::Nothing,
        };
    }
    hub.and_then(|hub| hub.thumbnail(&att.filename, att.size))
        .map_or(Still::Nothing, Still::Texture)
}

#[allow(clippy::cast_precision_loss, reason = "размер текстуры")]
fn texture_size(texture: &TextureHandle) -> [f32; 2] {
    let [w, h] = texture.size();
    [w as f32, h as f32]
}

/// Кадр «по заполнению»: прямоугольником со скруглением или кругом.
fn paint_texture(painter: &egui::Painter, rect: Rect, texture: &TextureHandle, round: bool) {
    let uv = cover_uv(texture_size(texture), [rect.width(), rect.height()]);
    if !round {
        painter.add(
            RectShape::filled(rect, theme::rounded(10), Color32::WHITE)
                .with_texture(texture.id(), uv),
        );
        return;
    }
    let mut mesh = Mesh::with_texture(texture.id());
    let (center, radius) = (rect.center(), rect.width().min(rect.height()) / 2.0);
    let to_uv = |p: egui::Pos2| {
        pos2(
            ((p.x - rect.min.x) / rect.width()).mul_add(uv.width(), uv.min.x),
            ((p.y - rect.min.y) / rect.height()).mul_add(uv.height(), uv.min.y),
        )
    };
    mesh.vertices.push(Vertex {
        pos: center,
        uv: to_uv(center),
        color: Color32::WHITE,
    });
    let steps = 72_u32;
    for i in 0..=steps {
        #[allow(clippy::cast_precision_loss, reason = "небольшие числа")]
        let angle = std::f32::consts::TAU * i as f32 / steps as f32;
        let p = center + Vec2::angled(angle) * radius;
        mesh.vertices.push(Vertex {
            pos: p,
            uv: to_uv(p),
            color: Color32::WHITE,
        });
        if i > 0 {
            mesh.add_triangle(0, i, i + 1);
        }
    }
    painter.add(mesh);
}

/// Круглая кнопка в центре: треугольник, пауза или многоточие.
fn play_button(ui: &egui::Ui, center: egui::Pos2, status: Option<&Status>, playing: bool) {
    let painter = ui.painter();
    painter.circle_filled(center, 24.0, Color32::from_black_alpha(150));
    let ink = Color32::WHITE;
    match status {
        Some(Status::Loading) => {
            painter.text(
                center,
                Align2::CENTER_CENTER,
                "…",
                FontId::proportional(20.0),
                ink,
            );
            ui.ctx().request_repaint();
        }
        _ if playing => {
            for dx in [-5.0, 5.0] {
                painter.rect_filled(
                    Rect::from_center_size(center + Vec2::new(dx, 0.0), Vec2::new(5.0, 18.0)),
                    1.0,
                    ink,
                );
            }
        }
        _ => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    center + Vec2::new(-6.0, -10.0),
                    center + Vec2::new(11.0, 0.0),
                    center + Vec2::new(-6.0, 10.0),
                ],
                ink,
                egui::Stroke::NONE,
            ));
        }
    }
}

/// Подпись снизу слева: время, размер или ошибка.
fn caption(ui: &egui::Ui, rect: Rect, att: &AttachmentView, view: Option<&View>, round: bool) {
    let text = view.map_or_else(
        || human_size(att.size),
        |view| match &view.status {
            Status::Failed(err) => theme::truncate_chars(err, 48),
            Status::Loading => crate::i18n::tr("Загрузка…").to_owned(),
            Status::Playing | Status::Paused => view.len.map_or_else(
                || clock(view.pos),
                |len| format!("{} / {}", clock(view.pos), clock(len)),
            ),
        },
    );
    let at = if round {
        pos2(rect.center().x, rect.max.y - 14.0)
    } else {
        pos2(rect.min.x + 10.0, rect.max.y - BAR - 4.0)
    };
    let align = if round {
        Align2::CENTER_BOTTOM
    } else {
        Align2::LEFT_BOTTOM
    };
    let font = FontId::proportional(11.5);
    let galley = ui.painter().layout_no_wrap(text, font, Color32::WHITE);
    let back = align
        .anchor_size(at, galley.size())
        .expand2(Vec2::new(5.0, 2.0));
    ui.painter()
        .rect_filled(back, 4.0, Color32::from_black_alpha(150));
    ui.painter()
        .galley(back.min + Vec2::new(5.0, 2.0), galley, Color32::WHITE);
}

/// Тонкая полоса хода видео.
fn progress(ui: &egui::Ui, bar: Rect, view: &View) {
    let Some(len) = view.len.filter(|len| *len > 0.0) else {
        return;
    };
    let line = Rect::from_center_size(bar.center(), Vec2::new(bar.width(), 4.0));
    let painter = ui.painter();
    painter.rect_filled(line, 2.0, Color32::from_white_alpha(70));
    let x = line
        .width()
        .mul_add((view.pos / len).clamp(0.0, 1.0), line.min.x);
    painter.rect_filled(
        Rect::from_min_max(line.min, pos2(x, line.max.y)),
        2.0,
        theme::sky(),
    );
    if ui.rect_contains_pointer(bar) {
        painter.circle_filled(pos2(x, line.center().y), 6.0, theme::sky());
    }
}

/// Ход «кружочка» — дугой по краю.
fn ring(ui: &egui::Ui, rect: Rect, view: &View) {
    let Some(len) = view.len.filter(|len| *len > 0.0) else {
        return;
    };
    let frac = (view.pos / len).clamp(0.0, 1.0);
    let (center, radius) = (rect.center(), rect.width() / 2.0 - 2.0);
    let steps = 96_u16;
    let points: Vec<egui::Pos2> = (0..=steps)
        .map(|i| {
            let angle = std::f32::consts::TAU.mul_add(
                frac * f32::from(i) / f32::from(steps),
                -std::f32::consts::FRAC_PI_2,
            );
            center + Vec2::angled(angle) * radius
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        points,
        egui::Stroke::new(3.0, theme::sky()),
    ));
}

/// Квадратная полупрозрачная кнопка в углу.
fn corner_button(ui: &egui::Ui, rect: Rect, glyph: &str) {
    let hover = ui.rect_contains_pointer(rect);
    ui.painter().rect_filled(
        rect,
        6.0,
        Color32::from_black_alpha(if hover { 200 } else { 150 }),
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(15.0),
        Color32::WHITE,
    );
}

/// Видео на всё окно. Esc или щелчок мимо — свернуть.
pub(crate) fn overlay(ctx: &egui::Context) {
    let Some(hub) = VideoHub::of(ctx) else { return };
    let Some(view) = hub.expanded() else { return };
    let screen = ctx.content_rect();
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        hub.expand(false);
        return;
    }
    egui::Area::new(egui::Id::unique("bs-video-full"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let back = ui.allocate_rect(screen, Sense::click());
            ui.painter()
                .rect_filled(screen, 0.0, Color32::from_black_alpha(235));
            let Some(frame) = &view.frame else { return };
            let room = (screen.size() - Vec2::new(48.0, 120.0)).max(Vec2::splat(64.0));
            let size = {
                let [w, h] = texture_size(frame);
                let scale = (room.x / w.max(1.0)).min(room.y / h.max(1.0));
                Vec2::new(w * scale, h * scale)
            };
            let video = Rect::from_center_size(screen.center() - Vec2::new(0.0, 30.0), size);
            ui.painter().image(
                frame.id(),
                video,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            let controls = Rect::from_center_size(
                pos2(screen.center().x, video.max.y + 36.0),
                Vec2::new(video.width().max(320.0), 40.0),
            );
            let button = Rect::from_center_size(
                pos2(controls.min.x + 20.0, controls.center().y),
                Vec2::splat(36.0),
            );
            play_button(
                ui,
                button.center(),
                Some(&view.status),
                view.status == Status::Playing,
            );
            let close = Rect::from_center_size(
                pos2(controls.max.x - 18.0, controls.center().y),
                Vec2::splat(DL_BUTTON),
            );
            corner_button(ui, close, "✕");
            let bar = Rect::from_min_max(
                pos2(button.max.x + 12.0, controls.min.y),
                pos2(close.min.x - 110.0, controls.max.y),
            );
            progress(ui, bar, &view);
            let time = view.len.map_or_else(
                || clock(view.pos),
                |len| format!("{} / {}", clock(view.pos), clock(len)),
            );
            ui.painter().text(
                pos2(bar.max.x + 10.0, controls.center().y),
                Align2::LEFT_CENTER,
                time,
                FontId::proportional(13.0),
                Color32::WHITE,
            );
            if !back.clicked() {
                return;
            }
            let pointer = ctx.pointer_interact_pos().unwrap_or_default();
            if close.contains(pointer) {
                hub.expand(false);
            } else if bar.contains(pointer) {
                let frac = ((pointer.x - bar.min.x) / bar.width()).clamp(0.0, 1.0);
                hub.seek(&view.file, frac * view.len.unwrap_or(0.0));
            } else if button.contains(pointer) || video.contains(pointer) {
                hub.toggle(&view.file);
            } else if !controls.contains(pointer) {
                hub.expand(false);
            }
        });
    if view.status == Status::Playing {
        ctx.request_repaint();
    }
}
