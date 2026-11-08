//! Векторные иконки, нарисованные линиями на сетке 24×24.
//!
//! Юникод-символы и эмодзи зависят от шрифта и системы и выглядят
//! по-разному (или вовсе пропадают), поэтому интерфейс рисует свои значки.

#![allow(
    clippy::suboptimal_flops,
    clippy::cast_precision_loss,
    clippy::many_single_char_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "рисование: небольшие числа, читаемая геометрия важнее fma"
)]

use egui::{Color32, Painter, Pos2, Shape, Stroke, Vec2};

/// Значок.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    /// Крестик.
    Close,
    /// Галочка.
    Check,
    /// Плюс.
    Plus,
    /// Шестерёнка.
    Gear,
    /// Облачко сообщения.
    Chat,
    /// Компас.
    Compass,
    /// Стрелка в лоток: вступить.
    Join,
    /// Один человек.
    User,
    /// Несколько людей.
    Users,
    /// Динамик с волнами.
    Speaker,
    /// Улыбающееся лицо.
    Smile,
    /// Лист с загнутым углом.
    File,
    /// Картинка: рамка, солнце, горы.
    Image,
    /// Наклейка с загнутым углом и улыбкой.
    Sticker,
    /// Бумажный самолётик.
    Send,
    /// Микрофон.
    Mic,
    /// Микрофон, перечёркнутый.
    MicOff,
    /// Наушники.
    Headphones,
    /// Наушники, перечёркнутые.
    HeadphonesOff,
    /// Телефонная трубка.
    Phone,
    /// Трубка: завершить звонок.
    HangUp,
    /// Лупа.
    Search,
    /// Стрелка вниз над лотком: скачать.
    Download,
}

/// Точки дуги окружности: углы в градусах, 0° — вправо, по часовой на экране.
fn arc(center: Pos2, radius: f32, from: f32, to: f32, steps: usize) -> Vec<Pos2> {
    (0..=steps)
        .map(|i| {
            let t = from + (to - from) * i as f32 / steps as f32;
            let rad = t.to_radians();
            center + Vec2::new(rad.cos(), rad.sin()) * radius
        })
        .collect()
}

#[allow(clippy::too_many_lines, reason = "по ветке на значок")]
/// Рисует значок по центру `center`; `size` — сторона квадрата в пикселях.
pub(crate) fn paint(painter: &Painter, center: Pos2, size: f32, icon: Icon, color: Color32) {
    let unit = size / 24.0;
    let width = (unit * 2.0).max(1.4);
    let stroke = Stroke::new(width, color);
    let p = |x: f32, y: f32| center + Vec2::new(x - 12.0, y - 12.0) * unit;
    let line = |points: Vec<Pos2>| {
        painter.add(Shape::line(points, stroke));
    };
    let ring = |points: Vec<Pos2>| {
        painter.add(Shape::closed_line(points, stroke));
    };
    match icon {
        Icon::Close => {
            line(vec![p(6.0, 6.0), p(18.0, 18.0)]);
            line(vec![p(18.0, 6.0), p(6.0, 18.0)]);
        }
        Icon::Check => line(vec![p(5.0, 12.5), p(10.0, 17.5), p(19.0, 7.0)]),
        Icon::Plus => {
            line(vec![p(12.0, 5.0), p(12.0, 19.0)]);
            line(vec![p(5.0, 12.0), p(19.0, 12.0)]);
        }
        Icon::Gear => {
            let mut outline = Vec::new();
            for tooth in 0..8 {
                let base = tooth as f32 * 45.0;
                for (offset, radius) in [(-11.0, 7.2), (-7.0, 9.6), (7.0, 9.6), (11.0, 7.2)] {
                    let rad = (base + offset).to_radians();
                    outline.push(p(12.0 + radius * rad.cos(), 12.0 + radius * rad.sin()));
                }
            }
            ring(outline);
            painter.circle_stroke(p(12.0, 12.0), 3.2 * unit, stroke);
        }
        Icon::Chat => {
            painter.rect_stroke(
                egui::Rect::from_min_max(p(3.5, 4.5), p(20.5, 17.0)),
                egui::CornerRadius::same((3.5 * unit).round().clamp(0.0, 255.0) as u8),
                stroke,
                egui::StrokeKind::Middle,
            );
            line(vec![p(8.0, 17.0), p(8.0, 21.0), p(12.5, 17.0)]);
        }
        Icon::Compass => {
            painter.circle_stroke(p(12.0, 12.0), 9.0 * unit, stroke);
            painter.add(Shape::convex_polygon(
                vec![p(16.5, 7.5), p(13.2, 13.2), p(7.5, 16.5), p(10.8, 10.8)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Join => {
            line(vec![p(12.0, 15.5), p(12.0, 4.5)]);
            line(vec![p(7.5, 9.0), p(12.0, 4.5), p(16.5, 9.0)]);
            line(vec![p(4.5, 15.0), p(4.5, 19.5), p(19.5, 19.5), p(19.5, 15.0)]);
        }
        Icon::User => {
            painter.circle_stroke(p(12.0, 8.0), 3.9 * unit, stroke);
            line(arc(p(12.0, 20.5), 7.5 * unit, 180.0, 360.0, 14));
        }
        Icon::Download => {
            line(vec![p(12.0, 3.5), p(12.0, 15.0)]);
            line(vec![p(7.0, 10.5), p(12.0, 15.5), p(17.0, 10.5)]);
            line(vec![p(4.5, 19.5), p(19.5, 19.5)]);
        }
        Icon::Search => {
            painter.circle_stroke(p(10.5, 10.5), 6.0 * unit, stroke);
            line(vec![p(15.0, 15.0), p(20.5, 20.5)]);
        }
        Icon::Users => {
            painter.circle_stroke(p(9.0, 8.5), 3.5 * unit, stroke);
            line(arc(p(9.0, 20.0), 6.5 * unit, 180.0, 360.0, 12));
            line(arc(p(16.5, 9.5), 3.0 * unit, 250.0, 430.0, 10));
            line(arc(p(17.5, 20.0), 5.0 * unit, 270.0, 360.0, 8));
        }
        Icon::Speaker => {
            ring(vec![
                p(4.0, 10.0),
                p(8.0, 10.0),
                p(13.0, 6.0),
                p(13.0, 18.0),
                p(8.0, 14.0),
                p(4.0, 14.0),
            ]);
            line(arc(p(13.0, 12.0), 4.5 * unit, -45.0, 45.0, 8));
            line(arc(p(13.0, 12.0), 8.0 * unit, -45.0, 45.0, 10));
        }
        Icon::Smile => {
            ring(arc(p(12.0, 12.0), 9.0 * unit, 0.0, 360.0, 32));
            line(arc(p(12.0, 12.5), 5.0 * unit, 25.0, 155.0, 10));
            painter.circle_filled(p(9.0, 9.5), unit * 1.3, color);
            painter.circle_filled(p(15.0, 9.5), unit * 1.3, color);
        }
        Icon::File => {
            ring(vec![
                p(6.0, 3.5),
                p(14.0, 3.5),
                p(19.0, 8.5),
                p(19.0, 20.5),
                p(6.0, 20.5),
            ]);
            line(vec![p(14.0, 3.5), p(14.0, 8.5), p(19.0, 8.5)]);
        }
        Icon::Image => {
            ring(vec![p(3.5, 5.0), p(20.5, 5.0), p(20.5, 19.0), p(3.5, 19.0)]);
            painter.circle_filled(p(8.5, 9.5), unit * 1.8, color);
            line(vec![
                p(3.5, 17.0),
                p(9.0, 12.5),
                p(13.0, 16.0),
                p(16.0, 13.5),
                p(20.5, 17.5),
            ]);
        }
        Icon::Sticker => {
            ring(vec![
                p(4.0, 4.0),
                p(20.0, 4.0),
                p(20.0, 13.0),
                p(13.0, 20.0),
                p(4.0, 20.0),
            ]);
            line(vec![
                p(20.0, 13.0),
                p(14.0, 13.0),
                p(13.0, 14.0),
                p(13.0, 20.0),
            ]);
            painter.circle_filled(p(9.0, 9.5), unit * 1.2, color);
            painter.circle_filled(p(14.5, 9.5), unit * 1.2, color);
            line(arc(p(11.5, 11.0), 4.0 * unit, 30.0, 150.0, 8));
        }
        Icon::Send => {
            ring(vec![p(3.5, 4.5), p(21.0, 12.0), p(3.5, 19.5), p(6.5, 12.0)]);
            line(vec![p(6.5, 12.0), p(13.0, 12.0)]);
        }
        Icon::Mic | Icon::MicOff => {
            let mut body = arc(p(12.0, 8.0), 3.5 * unit, 180.0, 360.0, 8);
            body.extend(arc(p(12.0, 12.0), 3.5 * unit, 0.0, 180.0, 8));
            ring(body);
            line(arc(p(12.0, 11.0), 6.5 * unit, 0.0, 180.0, 12));
            line(vec![p(12.0, 17.5), p(12.0, 21.0)]);
            line(vec![p(8.5, 21.0), p(15.5, 21.0)]);
            if icon == Icon::MicOff {
                line(vec![p(4.5, 3.5), p(19.5, 20.5)]);
            }
        }
        Icon::Headphones | Icon::HeadphonesOff => {
            line(arc(p(12.0, 13.0), 8.0 * unit, 180.0, 360.0, 14));
            line(vec![p(4.0, 13.0), p(4.0, 18.0)]);
            line(vec![p(20.0, 13.0), p(20.0, 18.0)]);
            ring(vec![p(4.0, 14.0), p(7.5, 14.0), p(7.5, 19.5), p(4.0, 19.5)]);
            ring(vec![
                p(16.5, 14.0),
                p(20.0, 14.0),
                p(20.0, 19.5),
                p(16.5, 19.5),
            ]);
            if icon == Icon::HeadphonesOff {
                line(vec![p(4.5, 3.5), p(19.5, 20.5)]);
            }
        }
        Icon::Phone => {
            ring(vec![
                p(4.5, 5.5),
                p(8.0, 4.0),
                p(10.0, 8.5),
                p(8.0, 10.5),
                p(13.5, 16.0),
                p(15.5, 14.0),
                p(20.0, 16.0),
                p(18.5, 19.5),
                p(13.0, 20.0),
                p(4.5, 11.0),
            ]);
        }
        Icon::HangUp => {
            line(arc(p(12.0, 17.0), 9.0 * unit, 200.0, 340.0, 14));
            line(vec![p(3.5, 14.5), p(3.5, 17.5)]);
            line(vec![p(20.5, 14.5), p(20.5, 17.5)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::arc;
    use egui::Pos2;

    #[test]
    fn arc_runs_from_first_to_last_angle() {
        let points = arc(Pos2::ZERO, 10.0, 0.0, 90.0, 9);
        assert_eq!(points.len(), 10);
        assert!((points[0].x - 10.0).abs() < 1e-4 && points[0].y.abs() < 1e-4);
        assert!(points[9].x.abs() < 1e-4 && (points[9].y - 10.0).abs() < 1e-4);
    }
}
