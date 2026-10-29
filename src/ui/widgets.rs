//! Custom-painted cards. Everything is drawn with the painter, so a card costs a
//! handful of shapes per frame and frames only happen when new data arrives.

use std::collections::VecDeque;
use std::f32::consts::PI;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Mesh, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2,
};

use super::theme;
use crate::poller::HISTORY_LEN;
use crate::sensors::{BreakdownItem, Group, Reading, Unit};

pub const FEATURED_HEIGHT: f32 = 252.0;
pub const STAT_HEIGHT: f32 = 128.0;

const PAD: f32 = 18.0;

/// Large card with a 270° arc gauge and a sparkline.
pub fn gauge_card(ui: &mut Ui, reading: &Reading, history: Option<&VecDeque<f32>>) {
    let rect = card(ui, FEATURED_HEIGHT);
    let painter = ui.painter_at(rect);
    let color = theme::accent(reading.unit, reading.value);
    let inner = rect.shrink(PAD);

    painter.text(
        inner.left_top(),
        Align2::LEFT_TOP,
        reading.label.to_uppercase(),
        theme::semibold(11.5),
        theme::TEXT_DIM,
    );

    // Gauge
    let radius = 64.0;
    let width = 9.0;
    let center = pos2(inner.center().x, inner.top() + 26.0 + radius + width);
    let (start, sweep) = (0.75 * PI, 1.5 * PI);
    let frac = animated(ui, reading.id, fraction(reading).unwrap_or(0.0));

    arc(&painter, center, radius, start, sweep, Stroke::new(width, theme::TRACK));
    if frac > 0.0 {
        // Soft glow under the value arc.
        arc(&painter, center, radius, start, sweep * frac, Stroke::new(width + 8.0, color.gamma_multiply(0.12)));
        arc(&painter, center, radius, start, sweep * frac, Stroke::new(width, color));
    }

    value_text(&painter, center + vec2(0.0, -4.0), (reading.value, reading.unit), 38.0, 15.0, Align2::CENTER_CENTER);

    if let Some(detail) = &reading.detail {
        painter.text(
            center + vec2(0.0, radius * 0.72),
            Align2::CENTER_CENTER,
            detail,
            theme::regular(12.5),
            theme::TEXT_DIM,
        );
    }

    if let Some(h) = history {
        let spark = Rect::from_min_max(pos2(inner.left(), inner.bottom() - 34.0), inner.right_bottom());
        sparkline(&painter, spark, h, reading.range, color);
    }
}

/// Compact card: label, value, optional usage bar and a sparkline. Extra height
/// beyond [`STAT_HEIGHT`] goes to the sparkline.
pub fn stat_card(ui: &mut Ui, reading: &Reading, history: Option<&VecDeque<f32>>, height: f32) {
    let rect = card(ui, height);
    let spark_height = 30.0 + (height - STAT_HEIGHT).max(0.0);
    let painter = ui.painter_at(rect);
    let color = theme::accent(reading.unit, reading.value);
    let inner = rect.shrink2(vec2(PAD, PAD - 2.0));

    painter.text(
        inner.left_top(),
        Align2::LEFT_TOP,
        reading.label.to_uppercase(),
        theme::semibold(11.0),
        theme::TEXT_DIM,
    );
    painter.circle_filled(pos2(inner.right() - 3.0, inner.top() + 6.0), 3.0, color);

    let anchor = inner.left_top() + vec2(0.0, 22.0);
    let value = value_text(&painter, anchor, (reading.value, reading.unit), 25.0, 12.5, Align2::LEFT_TOP);

    if let Some(detail) = &reading.detail {
        // Right of the value when there is room, otherwise tucked underneath it.
        let galley = painter.layout_no_wrap(detail.clone(), theme::regular(12.0), theme::TEXT_FAINT);
        let pos = if value.right() + 10.0 + galley.size().x <= inner.right() {
            pos2(inner.right() - galley.size().x, inner.top() + 33.0 - galley.size().y / 2.0)
        } else {
            pos2(inner.left(), value.bottom() + 1.0)
        };
        painter.galley(pos, galley, theme::TEXT_FAINT);
    }

    let mut bottom = inner.bottom();
    // Bars are not animated on purpose: with dozens of cards some bar changes every
    // tick, which would keep the UI rendering at 60 fps almost permanently.
    if let Some(frac) = fraction(reading) {
        let bar = Rect::from_min_max(pos2(inner.left(), bottom - 4.0), pos2(inner.right(), bottom));
        painter.rect_filled(bar, CornerRadius::same(2), theme::TRACK);
        if frac > 0.0 {
            let fill = Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * frac, bar.bottom()));
            painter.rect_filled(fill, CornerRadius::same(2), color);
        }
        bottom -= 12.0;
    }

    if let Some(h) = history {
        let spark = Rect::from_min_max(pos2(inner.left(), bottom - spark_height), pos2(inner.right(), bottom));
        sparkline(&painter, spark, h, reading.range, color);
    }
}

pub const CELL_HEIGHT: f32 = 100.0;

/// Small tile for one breakdown item (e.g. a CPU thread): value, detail, sparkline.
pub fn breakdown_cell(
    ui: &mut Ui,
    item: &BreakdownItem,
    unit: Unit,
    range: Option<(f64, f64)>,
    history: Option<&VecDeque<f32>>,
) {
    let rect = card(ui, CELL_HEIGHT);
    let painter = ui.painter_at(rect);
    let color = theme::accent(unit, item.value);
    let inner = rect.shrink2(vec2(14.0, 12.0));

    painter.text(inner.left_top(), Align2::LEFT_TOP, item.label.to_uppercase(), theme::semibold(10.5), theme::TEXT_DIM);
    value_text(&painter, inner.left_top() + vec2(0.0, 17.0), (item.value, unit), 19.0, 11.0, Align2::LEFT_TOP);
    if let Some(detail) = &item.detail {
        painter.text(inner.right_top(), Align2::RIGHT_TOP, detail, theme::regular(11.0), theme::TEXT_FAINT);
    }

    if let Some(h) = history {
        let spark = Rect::from_min_max(pos2(inner.left(), inner.bottom() - 30.0), inner.right_bottom());
        sparkline(&painter, spark, h, range, color);
    }
    // Load fill along the bottom edge doubles as a bar.
    if let Some(frac) = fraction_of(item.value, range).filter(|f| *f > 0.0) {
        let bar = Rect::from_min_max(
            pos2(rect.left() + 1.0, rect.bottom() - 3.0),
            pos2(rect.left() + 1.0 + (rect.width() - 2.0) * frac, rect.bottom() - 1.0),
        );
        painter.rect_filled(bar, CornerRadius { sw: theme::CARD_RADIUS, ..Default::default() }, color);
    }
}

pub const ROW_HEIGHT: f32 = 78.0;

/// Clickable list row for the components page: kind, name, up to three key values.
pub fn component_row(ui: &mut Ui, group: &Group) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = response.hovered();
    let painter = ui.painter_at(rect);
    painter.rect(
        rect,
        CornerRadius::same(theme::CARD_RADIUS),
        if hovered { theme::RAISED } else { theme::SURFACE },
        Stroke::new(1.0, if hovered { theme::BORDER_HOVER } else { theme::BORDER }),
        StrokeKind::Inside,
    );

    let inner = rect.shrink2(vec2(20.0, 0.0));
    let kind_color = theme::kind_color(group.kind);

    // Left: coloured kind tag, name, subtitle.
    painter.circle_filled(pos2(inner.left() + 3.0, rect.top() + 21.0), 3.0, kind_color);
    painter.text(
        pos2(inner.left() + 12.0, rect.top() + 21.0),
        Align2::LEFT_CENTER,
        group.kind.label().to_uppercase(),
        theme::semibold(10.5),
        kind_color,
    );
    painter.text(pos2(inner.left(), rect.top() + 41.0), Align2::LEFT_CENTER, &group.title, theme::semibold(15.0), theme::TEXT);
    painter.text(
        pos2(inner.left(), rect.top() + 60.0),
        Align2::LEFT_CENTER,
        &group.subtitle,
        theme::regular(12.0),
        theme::TEXT_FAINT,
    );

    // Right: chevron, then key values right-to-left.
    let chevron_color = if hovered { theme::TEXT } else { theme::TEXT_FAINT };
    painter.text(inner.right_center(), Align2::RIGHT_CENTER, "›", theme::regular(22.0), chevron_color);

    const COLUMN: f32 = 112.0;
    let key: Vec<&Reading> = group
        .readings
        .iter()
        .filter(|r| r.featured)
        .chain(group.readings.iter().filter(|r| !r.featured))
        .take(3)
        .collect();
    // Leave the name room on narrow windows: drop values that would overlap it.
    let title_right = inner.left() + painter.layout_no_wrap(group.title.clone(), theme::semibold(15.0), theme::TEXT).size().x;
    let mut right = inner.right() - 30.0;
    for r in key.iter().rev() {
        let left = right - COLUMN;
        if left < title_right + 24.0 {
            break;
        }
        painter.text(pos2(right, rect.top() + 26.0), Align2::RIGHT_CENTER, r.label.to_uppercase(), theme::semibold(10.0), theme::TEXT_FAINT);
        value_text(&painter, pos2(right, rect.top() + 48.0), (r.value, r.unit), 18.0, 11.0, Align2::RIGHT_CENTER);
        right = left;
    }

    response
}

/// On/off switch.
pub fn toggle(ui: &mut Ui, on: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(40.0, 22.0), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let painter = ui.painter();

    let track = match (on, response.hovered()) {
        (true, _) => theme::ACCENT,
        (false, true) => theme::BORDER_HOVER,
        (false, false) => theme::TRACK,
    };
    painter.rect_filled(rect, CornerRadius::same(11), track);
    let x = if on { rect.right() - 11.0 } else { rect.left() + 11.0 };
    let knob = if on { theme::BG } else { theme::TEXT_DIM };
    painter.circle_filled(pos2(x, rect.center().y), 7.5, knob);
    response
}

/// Pill-shaped toggle for one value. Disabled chips are dimmed and not clickable.
pub fn chip(ui: &mut Ui, label: &str, on: bool, enabled: bool, color: Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_owned(), theme::semibold(12.0), theme::TEXT);
    let size = vec2(galley.size().x + 40.0, 30.0);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let response = if enabled { response.on_hover_cursor(egui::CursorIcon::PointingHand) } else { response };
    let hovered = enabled && response.hovered();
    let dim = |c: Color32| if enabled { c } else { c.gamma_multiply(0.45) };
    let painter = ui.painter();

    let (fill, stroke) = if on {
        (color.gamma_multiply(if hovered { 0.24 } else { 0.14 }), color.gamma_multiply(0.55))
    } else {
        (Color32::TRANSPARENT, if hovered { theme::BORDER_HOVER } else { theme::BORDER })
    };
    painter.rect(rect, CornerRadius::same(15), dim(fill), Stroke::new(1.0, dim(stroke)), StrokeKind::Inside);

    let dot = pos2(rect.left() + 16.0, rect.center().y);
    if on {
        painter.circle_filled(dot, 4.0, dim(color));
    } else {
        painter.circle_stroke(dot, 3.5, Stroke::new(1.2, dim(theme::TEXT_FAINT)));
    }
    let text = if on { theme::TEXT } else { theme::TEXT_DIM };
    painter.galley_with_override_text_color(pos2(rect.left() + 27.0, rect.center().y - galley.size().y / 2.0), galley, dim(text));
    response
}

/// Allocates and paints the card background, returning its rect.
fn card(ui: &mut Ui, height: f32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(theme::CARD_RADIUS),
        theme::SURFACE,
        Stroke::new(1.0, theme::BORDER),
        StrokeKind::Inside,
    );
    rect
}

#[derive(Clone, Copy)]
struct Anim {
    from: f32,
    to: f32,
    start: f64,
}

/// Eases (cubic ease-out) from the currently shown value to `target`. Repaints are
/// only requested while an animation runs, so between updates the app still idles.
fn animated(ui: &Ui, key: &str, target: f32) -> f32 {
    const DURATION: f32 = 0.6;
    let ctx = ui.ctx();
    let id = ui.scope_id().with(key);
    let now = ctx.input(|i| i.time);
    let ease = |a: Anim| {
        let t = ((now - a.start) as f32 / DURATION).clamp(0.0, 1.0);
        (a.from + (a.to - a.from) * (1.0 - (1.0 - t).powi(3)), t)
    };

    let mut anim = ctx.data_mut(|d| {
        *d.get_temp_mut_or(id, Anim { from: target, to: target, start: f64::NEG_INFINITY })
    });
    if anim.to != target {
        // Start from wherever the running animation currently is. Sub-pixel changes
        // (noise in power/load readings) snap instead of costing ~100 repaints.
        let current = ease(anim).0;
        let start = if (target - current).abs() < 0.004 { f64::NEG_INFINITY } else { now };
        anim = Anim { from: current, to: target, start };
        ctx.data_mut(|d| d.insert_temp(id, anim));
    }

    let (value, t) = ease(anim);
    if t < 1.0 {
        // 60 fps is plenty for a gauge; avoids rendering at the monitor's full refresh rate.
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
    value
}

fn fraction(r: &Reading) -> Option<f32> {
    fraction_of(r.value, r.range)
}

fn fraction_of(value: f64, range: Option<(f64, f64)>) -> Option<f32> {
    let (lo, hi) = range?;
    (hi > lo).then(|| ((value - lo) / (hi - lo)).clamp(0.0, 1.0) as f32)
}

/// Value and unit as displayed, scaled to a readable magnitude.
fn display(v: f64, unit: Unit) -> (String, &'static str) {
    match unit {
        Unit::Celsius => (format!("{v:.0}"), "°C"),
        Unit::Percent => (format!("{v:.0}"), "%"),
        Unit::Watts if v < 100.0 => (format!("{v:.1}"), "W"),
        Unit::Watts => (format!("{v:.0}"), "W"),
        Unit::Megahertz if v >= 1000.0 => (format!("{:.2}", v / 1000.0), "GHz"),
        Unit::Megahertz => (format!("{v:.0}"), "MHz"),
        Unit::Mebibytes if v >= 1024.0 => (format!("{:.1}", v / 1024.0), "GB"),
        Unit::Mebibytes => (format!("{v:.0}"), "MB"),
        Unit::BytesPerSec if v >= 1024.0 * 1024.0 * 1024.0 => (format!("{:.2}", v / 1024f64.powi(3)), "GB/s"),
        Unit::BytesPerSec if v >= 1024.0 * 1024.0 => (format!("{:.1}", v / 1024f64.powi(2)), "MB/s"),
        Unit::BytesPerSec => (format!("{:.0}", v / 1024.0), "KB/s"),
        Unit::BitsPerSec if v >= 1e9 => (format!("{:.2}", v / 1e9), "Gbps"),
        Unit::BitsPerSec if v >= 1e6 => (format!("{:.1}", v / 1e6), "Mbps"),
        Unit::BitsPerSec => (format!("{:.0}", v / 1e3), "Kbps"),
    }
}

/// Big number with a smaller unit suffix, positioned as one block. Returns its rect.
fn value_text(
    painter: &egui::Painter,
    anchor: Pos2,
    (value, unit): (f64, Unit),
    size: f32,
    unit_size: f32,
    align: Align2,
) -> Rect {
    let (value, unit) = display(value, unit);
    let value = painter.layout_no_wrap(value, theme::semibold(size), theme::TEXT);
    let unit = painter.layout_no_wrap(unit.to_owned(), theme::regular(unit_size), theme::TEXT_DIM);
    let gap = 3.0;
    let total = vec2(value.size().x + gap + unit.size().x, value.size().y);
    let block = align.anchor_size(anchor, total);

    painter.galley(block.min, value.clone(), theme::TEXT);
    // Align unit baseline roughly with the number's baseline.
    let unit_pos = pos2(
        block.min.x + value.size().x + gap,
        block.min.y + value.size().y * 0.78 - unit.size().y * 0.78,
    );
    painter.galley(unit_pos, unit, theme::TEXT_DIM);
    block
}

fn arc(painter: &egui::Painter, center: Pos2, radius: f32, start: f32, sweep: f32, stroke: Stroke) {
    let segments = ((sweep.abs() / (2.0 * PI)) * 96.0).ceil().max(2.0) as usize;
    let points: Vec<Pos2> = (0..=segments)
        .map(|i| {
            let a = start + sweep * i as f32 / segments as f32;
            center + radius * vec2(a.cos(), a.sin())
        })
        .collect();
    // Round caps.
    let cap = stroke.width / 2.0;
    painter.circle_filled(points[0], cap, stroke.color);
    painter.circle_filled(*points.last().unwrap(), cap, stroke.color);
    painter.add(Shape::line(points, stroke));
}

/// Line chart that fills from the right edge, with a fading area underneath.
fn sparkline(painter: &egui::Painter, rect: Rect, values: &VecDeque<f32>, range: Option<(f64, f64)>, color: Color32) {
    if values.len() < 2 {
        return;
    }
    let (lo, hi) = match range {
        Some((lo, hi)) => (lo as f32, hi as f32),
        // Unbounded values: throughput-like series that touch zero scale from zero to
        // the peak; level-like ones (clocks, free memory) zoom into their recent range
        // instead of drawing a solid block.
        None => {
            let min = values.iter().copied().fold(f32::INFINITY, f32::min);
            let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            if min <= max * 0.05 {
                (0.0f32.min(min), (max * 1.15).max(1.0))
            } else {
                let pad = ((max - min) * 0.25).max(max.abs() * 0.01).max(0.5);
                (min - pad, max + pad)
            }
        }
    };
    let step = rect.width() / (HISTORY_LEN - 1) as f32;
    let x0 = rect.right() - step * (values.len() - 1) as f32;
    let points: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
            pos2(x0 + step * i as f32, rect.bottom() - t * rect.height())
        })
        .collect();

    let mut mesh = Mesh::default();
    let top = color.gamma_multiply(0.28);
    let bottom = Color32::TRANSPARENT;
    for p in &points {
        mesh.colored_vertex(*p, top);
        mesh.colored_vertex(pos2(p.x, rect.bottom()), bottom);
    }
    for i in 0..points.len() as u32 - 1 {
        let (a, b, c, d) = (2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3);
        mesh.add_triangle(a, b, c);
        mesh.add_triangle(b, d, c);
    }
    painter.add(Shape::mesh(mesh));
    painter.add(Shape::line(points, Stroke::new(1.6, color)));
}
