//! Знак Rustspace внутри интерфейса: буква «R» значка
//! (`packaging/brand/rustspace/icon.svg`), одним цветом.
//!
//! «R» в значке нарисована штрихами со скруглёнными концами и стыками:
//! ствол, чаша (полуокружность) и нога. Здесь те же штрихи: отрезки и
//! круги в каждой вершине, которые дают скругление. Цвет непрозрачный,
//! поэтому наложения не видны.

use std::f32::consts::PI;
use std::sync::OnceLock;

use egui::{Color32, Painter, Pos2, Stroke, Vec2};

/// Название продукта.
pub(crate) const NAME: &str = "Rustspace";
/// Репозиторий Rustspace: исходный код, выпуски, сообщения об ошибках.
pub(crate) const REPOSITORY: &str = "https://github.com/QuddyMusic/Rustspace";
/// Инстанс, который подставляется на экране входа, пока нет ни одного
/// своего. Меняется только здесь (или при сборке: `BS_DEFAULT_INSTANCE`).
pub(crate) const DEFAULT_INSTANCE: &str = match option_env!("BS_DEFAULT_INSTANCE") {
    Some(domain) => domain,
    None => "rustspace.duckdns.org",
};
/// Тот же репозиторий как `owner/repo` — для проверки обновлений.
pub(crate) const UPDATE_REPO: &str = "QuddyMusic/Rustspace";
/// Официальный репозиторий оригинального Backspace, на котором основан
/// Rustspace.
pub(crate) const UPSTREAM: &str = "https://github.com/TheZwiss/backspace";

/// Значок приложения (PNG 256×256) как текстура; грузится один раз.
pub(crate) fn icon_texture(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::unique("rustspace_icon_texture");
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return Some(texture);
    }
    let icon =
        // Плитка без светлого ободка: на 46 точках он выглядит белой рамкой.
        // Исходник — `packaging/brand/rustspace/tile.svg`.
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/brand-tile.png")).ok()?;
    let size = [
        usize::try_from(icon.width).ok()?,
        usize::try_from(icon.height).ok()?,
    ];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, &icon.rgba);
    let texture = ctx.load_texture("rustspace_icon", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    Some(texture)
}

/// Рамка «R» в координатах значка 256×256 (с учётом толщины штриха).
const VIEW_X: f32 = 82.0;
const VIEW_Y: f32 = 46.0;
const VIEW_W: f32 = 109.0;
const VIEW_H: f32 = 164.0;
/// Толщина штриха в координатах значка.
const STROKE: f32 = 32.0;
/// На сколько отрезков разбивается полуокружность чаши.
const ARC_STEPS: u16 = 16;

/// Штрихи «R»: ломаные в координатах значка.
fn strokes() -> &'static [Vec<Pos2>] {
    static STROKES: OnceLock<Vec<Vec<Pos2>>> = OnceLock::new();
    STROKES.get_or_init(|| {
        // Ствол снизу вверх, верх чаши, полуокружность, низ чаши.
        let mut body = vec![
            Pos2::new(98.0, 194.0),
            Pos2::new(98.0, 62.0),
            Pos2::new(138.0, 62.0),
        ];
        let center = Pos2::new(138.0, 99.0);
        for step in 1..=ARC_STEPS {
            let angle = -PI / 2.0 + PI * f32::from(step) / f32::from(ARC_STEPS);
            body.push(center + Vec2::new(angle.cos(), angle.sin()) * 37.0);
        }
        body.push(Pos2::new(98.0, 136.0));
        let leg = vec![Pos2::new(134.0, 136.0), Pos2::new(170.0, 194.0)];
        vec![body, leg]
    })
}

/// Рисует знак высотой `height` с центром в `center`.
pub(crate) fn paint(painter: &Painter, center: Pos2, height: f32, color: Color32) {
    let scale = height / VIEW_H;
    let origin = center - Vec2::new(VIEW_W, VIEW_H) * scale / 2.0;
    let map = |p: Pos2| origin + Vec2::new(p.x - VIEW_X, p.y - VIEW_Y) * scale;
    let width = STROKE * scale;
    for line in strokes() {
        for pair in line.windows(2) {
            painter.line_segment([map(pair[0]), map(pair[1])], Stroke::new(width, color));
        }
        for point in line {
            painter.circle_filled(map(*point), width / 2.0, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{STROKE, VIEW_H, VIEW_W, VIEW_X, VIEW_Y, strokes};

    #[test]
    fn strokes_fill_the_frame() {
        let points: Vec<_> = strokes().iter().flatten().collect();
        let half = STROKE / 2.0;
        let min_x = points.iter().map(|p| p.x).fold(f32::MAX, f32::min) - half;
        let max_x = points.iter().map(|p| p.x).fold(f32::MIN, f32::max) + half;
        let min_y = points.iter().map(|p| p.y).fold(f32::MAX, f32::min) - half;
        let max_y = points.iter().map(|p| p.y).fold(f32::MIN, f32::max) + half;
        assert!((min_x - VIEW_X).abs() < 0.5 && (max_x - (VIEW_X + VIEW_W)).abs() < 0.5);
        assert!((min_y - VIEW_Y).abs() < 0.5 && (max_y - (VIEW_Y + VIEW_H)).abs() < 0.5);
    }

    #[test]
    fn bowl_closes_on_the_stem() {
        let body = &strokes()[0];
        let last = body.last().copied().unwrap_or_default();
        assert!((last.x - 98.0).abs() < f32::EPSILON && (last.y - 136.0).abs() < f32::EPSILON);
        // Полуокружность проходит через крайнюю правую точку чаши.
        assert!(
            body.iter()
                .any(|p| (p.x - 175.0).abs() < 0.01 && (p.y - 99.0).abs() < 0.01)
        );
    }
}
