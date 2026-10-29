//! Custom-painted cards. Everything is drawn with the painter, so a card costs a
//! handful of shapes per frame and frames only happen when new data arrives.

use std::collections::VecDeque;
use std::f32::consts::PI;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Mesh, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2,
};

use super::theme;
use crate::poller::HISTORY_LEN;
use crate::sensors::{Reading, Unit};

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

    value_text(&painter, center + vec2(0.0, -4.0), reading, 38.0, 15.0, Align2::CENTER_CENTER);

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

/// Compact card: label, value, optional usage bar and a sparkline.
pub fn stat_card(ui: &mut Ui, reading: &Reading, history: Option<&VecDeque<f32>>) {
    let rect = card(ui, STAT_HEIGHT);
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

    let value = value_text(&painter, inner.left_top() + vec2(0.0, 22.0), reading, 25.0, 12.5, Align2::LEFT_TOP);

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
        let spark = Rect::from_min_max(pos2(inner.left(), bottom - 30.0), pos2(inner.right(), bottom));
        sparkline(&painter, spark, h, reading.range, color);
    }
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
    let (lo, hi) = r.range?;
    (hi > lo).then(|| ((r.value - lo) / (hi - lo)).clamp(0.0, 1.0) as f32)
}

/// Value and unit as displayed, scaled to a readable magnitude.
fn display(r: &Reading) -> (String, &'static str) {
    let v = r.value;
    match r.unit {
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
fn value_text(painter: &egui::Painter, anchor: Pos2, r: &Reading, size: f32, unit_size: f32, align: Align2) -> Rect {
    let (value, unit) = display(r);
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
        // Unbounded values (throughput, clocks) scale from zero to the recent peak.
        None => {
            let min = values.iter().copied().fold(0.0, f32::min);
            let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            (min, (max * 1.15).max(min + 1.0))
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
