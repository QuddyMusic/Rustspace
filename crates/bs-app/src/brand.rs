//! Знак бренда по векторному контуру из оригинала (`logo-mark.svg`).
//!
//! Контур разбирается один раз, кривые разбиваются на отрезки, а вогнутый
//! многоугольник заливается треугольниками (метод отсечения «ушей»):
//! `egui` умеет заливать без артефактов только выпуклые фигуры. Края
//! сглаживаются тонкой обводкой того же цвета.

#![allow(
    clippy::suboptimal_flops,
    clippy::cast_precision_loss,
    clippy::many_single_char_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "рисование: небольшие числа, читаемая геометрия важнее fma"
)]

use std::sync::OnceLock;

use egui::{Color32, Mesh, Painter, Pos2, Shape, Stroke, Vec2};

/// Контур знака в координатах оригинала (viewBox `62 38 133 180`).
const PATH: &str = "M151.191 38C175.386 38 195 57.5367 195 81.6364C195 103.651 177.153 121.498 155.138 121.498H121.104C119.429 121.498 118.269 120.983 117.625 119.953C116.981 118.795 117.432 117.443 118.978 115.899L149.516 85.5845C151.32 83.7826 151.32 81.9809 149.516 80.179L144.684 75.3519C142.88 73.5498 141.076 73.5498 139.272 75.3519L89.0206 125.553C87.2168 127.355 87.2168 129.222 89.0206 131.152L139.659 181.739C141.463 183.541 143.267 183.541 145.071 181.739L149.516 177.106C151.32 175.304 151.32 173.501 149.516 171.699L118.978 141.192C117.432 139.648 116.981 138.36 117.625 137.33C118.269 136.172 119.429 135.593 121.104 135.593H156.229C177.642 135.593 195 152.951 195 174.364C195 198.463 175.386 218 151.191 218H105.809C81.614 218 62 198.463 62 174.364V81.6364C62 57.5367 81.614 38 105.809 38L151.191 38Z";
const VIEW_X: f32 = 62.0;
const VIEW_Y: f32 = 38.0;
const VIEW_W: f32 = 133.0;
const VIEW_H: f32 = 180.0;
/// На сколько отрезков разбивается каждая кривая Безье.
const CURVE_STEPS: u16 = 14;

/// Разобранный контур: вершины в единичных координатах высоты знака.
struct Shapes {
    outline: Vec<Pos2>,
    triangles: Vec<[u32; 3]>,
}

fn shapes() -> &'static Shapes {
    static CELL: OnceLock<Shapes> = OnceLock::new();
    CELL.get_or_init(|| {
        let outline = flatten(PATH);
        let triangles = triangulate(&outline);
        Shapes { outline, triangles }
    })
}

/// Числа и команды контура. Поддержаны абсолютные `M L H V C Z` —
/// ровно то, что есть в знаке.
fn flatten(path: &str) -> Vec<Pos2> {
    let mut tokens: Vec<String> = Vec::new();
    let mut number = String::new();
    for ch in path.chars() {
        if ch.is_ascii_alphabetic() {
            if !number.is_empty() {
                tokens.push(std::mem::take(&mut number));
            }
            tokens.push(ch.to_string());
        } else if ch == ',' || ch == ' ' {
            if !number.is_empty() {
                tokens.push(std::mem::take(&mut number));
            }
        } else if ch == '-' && !number.is_empty() {
            tokens.push(std::mem::take(&mut number));
            number.push(ch);
        } else {
            number.push(ch);
        }
    }
    if !number.is_empty() {
        tokens.push(number);
    }

    let mut points: Vec<Pos2> = Vec::new();
    let mut current = Pos2::ZERO;
    let mut index = 0;
    let mut command = 'M';
    let num = |i: &mut usize| -> f32 {
        let value = tokens.get(*i).and_then(|t| t.parse().ok()).unwrap_or(0.0);
        *i += 1;
        value
    };
    while index < tokens.len() {
        if let Some(letter) = tokens[index]
            .chars()
            .next()
            .filter(char::is_ascii_alphabetic)
        {
            command = letter;
            index += 1;
            if command == 'Z' {
                continue;
            }
        }
        match command {
            'M' | 'L' => {
                current = Pos2::new(num(&mut index), num(&mut index));
                points.push(current);
                command = 'L';
            }
            'H' => {
                current.x = num(&mut index);
                points.push(current);
            }
            'V' => {
                current.y = num(&mut index);
                points.push(current);
            }
            'C' => {
                let c1 = Pos2::new(num(&mut index), num(&mut index));
                let c2 = Pos2::new(num(&mut index), num(&mut index));
                let end = Pos2::new(num(&mut index), num(&mut index));
                let start = current;
                for step in 1..=CURVE_STEPS {
                    let t = f32::from(step) / f32::from(CURVE_STEPS);
                    points.push(Pos2::new(
                        cubic(t, [start.x, c1.x, c2.x, end.x]),
                        cubic(t, [start.y, c1.y, c2.y, end.y]),
                    ));
                }
                current = end;
            }
            _ => index += 1,
        }
    }
    // Убираем подряд идущие одинаковые точки: они ломают отсечение ушей.
    points.dedup_by(|a, b| (a.x - b.x).abs() < 1e-4 && (a.y - b.y).abs() < 1e-4);
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    points
}

/// Значение кубической кривой Безье в точке `t` по четырём опорным числам.
fn cubic(t: f32, [a, b, c, d]: [f32; 4]) -> f32 {
    let u = 1.0 - t;
    (t * t * t).mul_add(
        d,
        (3.0 * u * t * t).mul_add(c, (3.0 * u * u * t).mul_add(b, u * u * u * a)),
    )
}

/// Удвоенная площадь со знаком (положительна при обходе против часовой в
/// математических осях, то есть по часовой на экране).
fn signed_area(points: &[Pos2]) -> f32 {
    let mut sum = 0.0;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        sum += a.x * b.y - b.x * a.y;
    }
    sum
}

fn cross(a: Pos2, b: Pos2, c: Pos2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn inside(p: Pos2, a: Pos2, b: Pos2, c: Pos2) -> bool {
    let d1 = cross(a, b, p);
    let d2 = cross(b, c, p);
    let d3 = cross(c, a, p);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

/// Разбивает простой многоугольник на треугольники отсечением ушей.
fn triangulate(points: &[Pos2]) -> Vec<[u32; 3]> {
    let count = points.len();
    let mut order: Vec<usize> = (0..count).collect();
    if signed_area(points) < 0.0 {
        order.reverse();
    }
    let mut triangles = Vec::with_capacity(count.saturating_sub(2));
    let mut guard = count * count;
    while order.len() > 3 && guard > 0 {
        guard -= 1;
        let n = order.len();
        let mut clipped = false;
        for i in 0..n {
            let (ia, ib, ic) = (order[(i + n - 1) % n], order[i], order[(i + 1) % n]);
            let (a, b, c) = (points[ia], points[ib], points[ic]);
            if cross(a, b, c) <= 1e-6 {
                continue;
            }
            let blocked = order
                .iter()
                .filter(|&&k| k != ia && k != ib && k != ic)
                .any(|&k| inside(points[k], a, b, c));
            if blocked {
                continue;
            }
            triangles.push([ia as u32, ib as u32, ic as u32]);
            order.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if order.len() == 3 {
        triangles.push([order[0] as u32, order[1] as u32, order[2] as u32]);
    }
    triangles
}

/// Рисует знак по центру `center`; `height` — высота знака в пикселях.
pub(crate) fn paint(painter: &Painter, center: Pos2, height: f32, color: Color32) {
    let shapes = shapes();
    let scale = height / VIEW_H;
    let origin = center - Vec2::new(VIEW_W, VIEW_H) * scale / 2.0;
    let map = |p: Pos2| origin + Vec2::new(p.x - VIEW_X, p.y - VIEW_Y) * scale;
    let mut mesh = Mesh::default();
    for point in &shapes.outline {
        mesh.colored_vertex(map(*point), color);
    }
    for [a, b, c] in &shapes.triangles {
        mesh.add_triangle(*a, *b, *c);
    }
    painter.add(Shape::mesh(mesh));
    let edge: Vec<Pos2> = shapes.outline.iter().map(|p| map(*p)).collect();
    painter.add(Shape::closed_line(edge, Stroke::new(1.0, color)));
}

#[cfg(test)]
mod tests {
    use super::{shapes, signed_area};

    #[test]
    fn outline_fills_the_viewbox() {
        let outline = &shapes().outline;
        assert!(outline.len() > 60, "кривые разбиты на отрезки");
        let min_x = outline.iter().map(|p| p.x).fold(f32::MAX, f32::min);
        let max_x = outline.iter().map(|p| p.x).fold(f32::MIN, f32::max);
        let min_y = outline.iter().map(|p| p.y).fold(f32::MAX, f32::min);
        let max_y = outline.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        assert!((min_x - 62.0).abs() < 0.5 && (max_x - 195.0).abs() < 0.5);
        assert!((min_y - 38.0).abs() < 0.5 && (max_y - 218.0).abs() < 0.5);
    }

    #[test]
    fn triangles_cover_exactly_the_outline_area() {
        let shapes = shapes();
        let total: f32 = shapes
            .triangles
            .iter()
            .map(|[a, b, c]| {
                let tri = [
                    shapes.outline[*a as usize],
                    shapes.outline[*b as usize],
                    shapes.outline[*c as usize],
                ];
                signed_area(&tri).abs()
            })
            .sum();
        let outline = signed_area(&shapes.outline).abs();
        assert!(
            (total - outline).abs() / outline < 0.002,
            "треугольники {total} против контура {outline}"
        );
    }
}
