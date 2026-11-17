//! Ширина колонок: левой (каналы, личные сообщения) и правой (участники,
//! «Сейчас онлайн»). Край колонки тянется мышью, двойной щелчок по нему
//! возвращает ширину по умолчанию; ширина запоминается. По PR #477
//! оригинального Backspace (QuddyMusic).

use egui::{Id, Rect, Sense};

/// Какая колонка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    /// Левая: тянется за правый край.
    Left,
    /// Правая: тянется за левый край.
    Right,
}

/// Пределы и значения по умолчанию, точки.
pub(crate) const MIN: f32 = 180.0;
pub(crate) const MAX: f32 = 420.0;
pub(crate) const LEFT_DEFAULT: f32 = 250.0;
pub(crate) const RIGHT_DEFAULT: f32 = 236.0;

/// Сохраняемые ширины.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Widths {
    /// Левая колонка.
    pub(crate) left: f32,
    /// Правая колонка.
    pub(crate) right: f32,
}

impl Default for Widths {
    fn default() -> Self {
        Self {
            left: LEFT_DEFAULT,
            right: RIGHT_DEFAULT,
        }
    }
}

fn clamp(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(MIN, MAX)
    } else {
        MIN
    }
}

fn live_id(side: Side) -> Id {
    Id::unique(("column_width", side == Side::Left))
}

/// Текущая ширина колонки: во время перетаскивания — живое значение.
#[must_use]
pub(crate) fn width(ctx: &egui::Context, side: Side) -> f32 {
    if let Some(live) = ctx.data(|d| d.get_temp::<f32>(live_id(side))) {
        return live;
    }
    let saved = crate::prefs::get().columns;
    clamp(match side {
        Side::Left => saved.left,
        Side::Right => saved.right,
    })
}

/// Ручка на краю показанной колонки `panel`. Ставится после самой
/// колонки, чтобы лежать поверх её содержимого.
pub(crate) fn handle(ui: &egui::Ui, panel: Rect, side: Side) {
    let x = match side {
        Side::Left => panel.right(),
        Side::Right => panel.left(),
    };
    let area = Rect::from_x_y_ranges(x - 4.0..=x + 4.0, panel.y_range());
    let id = live_id(side).with("handle");
    let response = ui
        .interact(area, id, Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    let ctx = ui.ctx();
    if response.double_clicked() {
        let reset = match side {
            Side::Left => LEFT_DEFAULT,
            Side::Right => RIGHT_DEFAULT,
        };
        save(side, reset);
        ctx.data_mut(|d| d.remove::<f32>(live_id(side)));
        return;
    }
    if response.dragged() {
        let dx = response.drag_delta().x;
        let now = width(ctx, side)
            + match side {
                Side::Left => dx,
                Side::Right => -dx,
            };
        ctx.data_mut(|d| d.insert_temp(live_id(side), clamp(now)));
        ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if response.drag_stopped() {
        save(side, width(ctx, side));
        ctx.data_mut(|d| d.remove::<f32>(live_id(side)));
    }
    if response.hovered() || response.dragged() {
        ui.painter().vline(
            x,
            panel.y_range().shrink(12.0),
            egui::Stroke::new(2.0, crate::theme::accent().gamma_multiply(0.6)),
        );
    }
}

fn save(side: Side, width: f32) {
    let width = clamp(width);
    crate::prefs::update(|prefs| match side {
        Side::Left => prefs.columns.left = width,
        Side::Right => prefs.columns.right = width,
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn widths_stay_in_range() {
        assert!((super::clamp(10.0) - super::MIN).abs() < f32::EPSILON);
        assert!((super::clamp(9_999.0) - super::MAX).abs() < f32::EPSILON);
        assert!((super::clamp(f32::NAN) - super::MIN).abs() < f32::EPSILON);
        assert!((super::clamp(300.0) - 300.0).abs() < f32::EPSILON);
    }
}
