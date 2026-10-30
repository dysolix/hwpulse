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
        sparkline(ui, reading.id, spark, h, reading.range, color);
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
        sparkline(ui, reading.id, spark, h, reading.range, color);
    }
}

/// Grip of six dots for reordering; returns the drag response.
pub fn drag_handle(ui: &mut Ui, id: egui::Id) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(18.0, 36.0), Sense::hover());
    let response = ui.interact(rect, id, Sense::drag());
    let cursor = if response.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab };
    let response = response.on_hover_cursor(cursor);
    if response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }

    let color = if response.hovered() || response.dragged() { theme::TEXT } else { theme::TEXT_FAINT };
    for row in -1..=1 {
        for col in [-1.0f32, 1.0] {
            let c = rect.center() + vec2(col * 3.5, row as f32 * 6.0);
            ui.painter().circle_filled(c, 1.8, color);
        }
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
    painter.galley_with_override_text_color(
        pos2(rect.left() + 27.0, rect.center().y - galley.size().y / 2.0),
        galley,
        dim(text),
    );
    response
}

/// Text of [`waiting`] until the first samples arrive.
pub const SENSORS: (&str, &str) = ("Waiting for sensors…", "Reading processor, graphics, memory, drives and network");

/// Loading screen, e.g. until the first samples arrive: a spinning arc with a short
/// message, centred in the remaining space.
pub fn waiting(ui: &mut Ui, title: &str, subtitle: &str) {
    const RADIUS: f32 = 18.0;
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    let painter = ui.painter_at(rect);
    // Slightly above the middle, which reads as centred with the header above.
    let center = rect.center() - vec2(0.0, 40.0);

    painter.circle_stroke(center, RADIUS, Stroke::new(3.0, theme::TRACK));
    let time = ui.input(|i| i.time) as f32;
    let start = time * 1.6 * PI;
    // The arc grows and shrinks while it turns, like system spinners.
    let sweep = PI * (0.9 + 0.6 * (time * 2.2).sin());
    let points: Vec<Pos2> = (0..=32)
        .map(|i| {
            let a = start + sweep * i as f32 / 32.0;
            center + vec2(a.cos(), a.sin()) * RADIUS
        })
        .collect();
    painter.add(Shape::line(points, Stroke::new(3.0, theme::ACCENT)));

    painter.text(center + vec2(0.0, RADIUS + 24.0), Align2::CENTER_TOP, title, theme::semibold(15.0), theme::TEXT);
    painter.text(
        center + vec2(0.0, RADIUS + 48.0),
        Align2::CENTER_TOP,
        subtitle,
        theme::regular(12.5),
        theme::TEXT_FAINT,
    );
    // Only shown for a moment at startup, so a smooth 60 fps costs nothing.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
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

    let mut anim =
        ctx.data_mut(|d| *d.get_temp_mut_or(id, Anim { from: target, to: target, start: f64::NEG_INFINITY }));
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
        Unit::Milliseconds if v < 10.0 => (format!("{v:.1}"), "ms"),
        Unit::Milliseconds => (format!("{v:.0}"), "ms"),
        Unit::PerSecond if v >= 1e6 => (format!("{:.1}", v / 1e6), "M/s"),
        Unit::PerSecond if v >= 1e4 => (format!("{:.0}", v / 1e3), "k/s"),
        Unit::PerSecond => (format!("{v:.0}"), "/s"),
        Unit::Count if v.fract() != 0.0 && v < 10.0 => (format!("{v:.1}"), ""),
        Unit::Count => (format!("{v:.0}"), ""),
    }
}

/// Value with its unit as one string, e.g. `"4.55 GHz"` or `"14%"`.
pub fn format_value(v: f64, unit: Unit) -> String {
    let (value, unit) = display(v, unit);
    if unit == "%" { format!("{value}%") } else { format!("{value} {unit}") }
}

/// Big number with a smaller unit suffix, positioned as one block. Returns its rect.
pub fn value_text(
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
    let unit_pos = pos2(block.min.x + value.size().x + gap, block.min.y + value.size().y * 0.78 - unit.size().y * 0.78);
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
fn sparkline(ui: &Ui, key: &str, rect: Rect, values: &VecDeque<f32>, range: Option<(f64, f64)>, color: Color32) {
    if values.len() < 2 {
        return;
    }
    let bounds = smooth_bounds(ui, key, bounds(extent([values]), range, None));
    plot_line(&ui.painter_at(rect.expand(2.0)), rect, values, bounds, color, false);
}

/// How long new samples take to slide in from the right edge.
/// Short on purpose: every frame of it costs a full redraw.
const SLIDE: f32 = 0.3;

/// Where the charts are between two polls; shared by every chart in a frame.
#[derive(Clone, Copy, Default)]
struct Timeline {
    /// Part of one sample step the lines still have to slide left (1 → 0, eased).
    shift: f32,
}

fn timeline_id() -> egui::Id {
    egui::Id::NULL.with("chart_timeline")
}

/// Call once per frame, before drawing charts, with the time since the newest samples
/// arrived.
pub fn set_timeline(ctx: &egui::Context, since: Option<std::time::Duration>) {
    let t = since.map_or(1.0, |d| (d.as_secs_f32() / SLIDE).min(1.0));
    let shift = (1.0 - t).powi(3); // cubic ease-out, like the gauges
    ctx.data_mut(|d| d.insert_temp(timeline_id(), Timeline { shift }));
}

fn timeline(ctx: &egui::Context) -> Timeline {
    ctx.data(|d| d.get_temp(timeline_id())).unwrap_or_default()
}

/// Eases scale changes (a peak leaving the window, a new maximum) instead of jumping.
fn smooth_bounds(ui: &Ui, key: &str, (lo, hi): (f32, f32)) -> (f32, f32) {
    const DURATION: f32 = 0.4;
    let id = ui.scope_id().with(("bounds", key));
    let ctx = ui.ctx();
    (ctx.animate_value_with_time(id.with(0), lo, DURATION), ctx.animate_value_with_time(id.with(1), hi, DURATION))
}

/// Smallest and largest sample across several series.
fn extent<'a>(series: impl IntoIterator<Item = &'a VecDeque<f32>>) -> (f32, f32) {
    series.into_iter().flatten().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| (lo.min(v), hi.max(v)))
}

/// Vertical scale for samples between `min` and `max`. Unbounded throughput-like
/// series that touch zero scale from zero to the peak; level-like ones (clocks, free
/// memory) zoom into their recent range instead of drawing a solid block. With
/// `axis` the zero-based top is rounded to a value worth printing as axis label.
fn bounds((min, max): (f32, f32), range: Option<(f64, f64)>, axis: Option<Unit>) -> (f32, f32) {
    if let Some((lo, hi)) = range {
        return (lo as f32, hi as f32);
    }
    if min > max {
        return (0.0, 1.0); // no samples yet
    }
    if min <= max * 0.05 {
        let top = match axis {
            Some(unit) => nice_ceiling(max, unit),
            None => (max * 1.15).max(1.0),
        };
        (0.0f32.min(min), top)
    } else {
        let pad = ((max - min) * 0.25).max(max.abs() * 0.01).max(0.5);
        (min - pad, max + pad)
    }
}

/// Next 1 / 2 / 5 step at or above `v`, in the magnitude it is displayed in
/// (KB/s → MB/s steps by 1024, Kbps → Mbps by 1000).
fn nice_ceiling(v: f32, unit: Unit) -> f32 {
    let base = match unit {
        Unit::BytesPerSec | Unit::Mebibytes => 1024.0,
        Unit::BitsPerSec => 1000.0,
        _ => 0.0,
    };
    let floor = match unit {
        Unit::BytesPerSec => 1024.0,
        Unit::BitsPerSec => 1000.0,
        _ => 1.0,
    };
    let v = v.max(floor);
    let mut scale = 1.0;
    if base > 0.0 {
        while v / scale >= base {
            scale *= base;
        }
    }
    let x = v / scale;
    let magnitude = 10f32.powf(x.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * magnitude).find(|s| *s >= x).unwrap_or(x);
    step * scale
}

/// One line in a [`chart`] or [`thumbnail`].
pub struct Series<'a> {
    pub label: &'a str,
    pub history: Option<&'a VecDeque<f32>>,
    pub value: f64,
    pub unit: Unit,
    /// Secondary lines (upload, write) are dashed and unfilled.
    pub dashed: bool,
}

/// Task Manager style history chart: grid, one or more lines, scale labels and a
/// hover readout. `compact` charts show the current value instead of axis labels;
/// `note` is a faint addition after the title (e.g. a thread's clock).
#[allow(clippy::too_many_arguments)]
pub fn chart(
    ui: &mut Ui,
    height: f32,
    title: &str,
    note: Option<&str>,
    series: &[Series],
    range: Option<(f64, f64)>,
    color: Color32,
    compact: bool,
) {
    let rect = card(ui, height);
    let Some(first) = series.first() else { return };
    let painter = ui.painter_at(rect);
    let inner = rect.shrink2(vec2(16.0, 14.0));
    let (lo, hi) = bounds(extent(series.iter().filter_map(|s| s.history)), range, Some(first.unit));
    // Labels show the target scale, the lines ease towards it.
    let (line_lo, line_hi) = smooth_bounds(ui, title, (lo, hi));

    // Header: title and legend on the left, scale or current value on the right.
    let title =
        painter.text(inner.left_top(), Align2::LEFT_TOP, title.to_uppercase(), theme::semibold(11.0), theme::TEXT_DIM);
    let corner = if compact {
        painter.text(
            inner.right_top(),
            Align2::RIGHT_TOP,
            format_value(first.value, first.unit),
            theme::semibold(12.5),
            theme::TEXT,
        )
    } else {
        painter.text(
            inner.right_top(),
            Align2::RIGHT_TOP,
            format_value(hi as f64, first.unit),
            theme::regular(11.0),
            theme::TEXT_FAINT,
        )
    };
    let title = match note {
        Some(note) => {
            let width = corner.left() - title.right() - 18.0;
            let galley = elided(&painter, note, theme::regular(11.0), theme::TEXT_FAINT, width);
            let at = pos2(title.right() + 8.0, title.center().y - galley.size().y / 2.0);
            let rect = Rect::from_min_size(at, galley.size());
            painter.galley(at, galley, theme::TEXT_FAINT);
            title.union(rect)
        }
        None => title,
    };
    if series.len() > 1 {
        let mut x = title.right() + 16.0;
        let y = title.center().y;
        for s in series {
            let label = painter.layout_no_wrap(s.label.to_owned(), theme::regular(11.5), theme::TEXT_DIM);
            if x + 22.0 + label.size().x > corner.left() - 10.0 {
                break;
            }
            let stroke = Stroke::new(1.6, color);
            if s.dashed {
                painter.extend(Shape::dashed_line(&[pos2(x, y), pos2(x + 16.0, y)], stroke, 4.0, 3.0));
            } else {
                painter.line_segment([pos2(x, y), pos2(x + 16.0, y)], stroke);
            }
            x += 22.0;
            painter.galley(pos2(x, y - label.size().y / 2.0), label.clone(), theme::TEXT_DIM);
            x += label.size().x + 14.0;
        }
    }

    let footer = if compact { 0.0 } else { 20.0 };
    let plot = Rect::from_min_max(pos2(inner.left(), inner.top() + 24.0), pos2(inner.right(), inner.bottom() - footer));
    grid(&painter, plot, if compact { (6, 2) } else { (12, 4) });
    if !compact {
        let span = HISTORY_LEN as u64 * crate::poller::INTERVAL.as_secs();
        let span = if span.is_multiple_of(60) { format!("{} minutes", span / 60) } else { format!("{span} seconds") };
        let y = plot.bottom() + 6.0;
        painter.text(pos2(plot.left(), y), Align2::LEFT_TOP, span, theme::regular(11.0), theme::TEXT_FAINT);
        let low = format_value(lo as f64, first.unit);
        painter.text(pos2(plot.right(), y), Align2::RIGHT_TOP, low, theme::regular(11.0), theme::TEXT_FAINT);
    }
    for s in series.iter().rev() {
        if let Some(h) = s.history.filter(|h| h.len() >= 2) {
            plot_line(&painter, plot, h, (line_lo, line_hi), color, s.dashed);
        }
    }

    // Hover readout. The marker stays on the sample slot under the pointer while the
    // line slides through it: the dots ride the line, while the text always shows
    // the sample that settles in this slot, so it changes once per update.
    let Some(pointer) = ui.ctx().pointer_hover_pos().filter(|p| plot.contains(*p) && ui.rect_contains_pointer(rect))
    else {
        return;
    };
    let step = plot.width() / (HISTORY_LEN - 1) as f32;
    let slot = ((plot.right() - pointer.x) / step).round().max(0.0);
    let x = plot.right() - slot * step;
    // Samples back from the newest that sit under the marker right now (fractional mid-slide).
    let under = slot + timeline(ui.ctx()).shift;
    let ago = slot as usize;
    let mut lines = Vec::new();
    for s in series {
        let Some(h) = s.history.filter(|h| ago < h.len()) else { continue };
        let v = h[h.len() - 1 - ago];
        let at = |i: usize| h[h.len() - 1 - i.min(h.len() - 1)];
        let (newer, frac) = (under.floor() as usize, under.fract());
        let line_v = at(newer) + (at(newer + 1) - at(newer)) * frac;
        let t = ((line_v - line_lo) / (line_hi - line_lo)).clamp(0.0, 1.0);
        painter.circle_filled(pos2(x, plot.bottom() - t * plot.height()), 3.5, color);
        lines.push(if series.len() > 1 {
            format!("{}  {}", s.label, format_value(v as f64, s.unit))
        } else {
            format_value(v as f64, s.unit)
        });
    }
    if lines.is_empty() {
        return;
    }
    painter.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], Stroke::new(1.0, theme::BORDER_HOVER));
    let when =
        if ago == 0 { "now".to_owned() } else { format!("{} s ago", ago as u64 * crate::poller::INTERVAL.as_secs()) };
    let when = painter.layout_no_wrap(when, theme::regular(11.0), theme::TEXT_FAINT);
    let text = painter.layout_no_wrap(lines.join("\n"), theme::semibold(12.5), theme::TEXT);
    let size = vec2(text.size().x.max(when.size().x), when.size().y + 2.0 + text.size().y) + vec2(20.0, 14.0);
    let left = if x + 10.0 + size.x <= plot.right() { x + 10.0 } else { (x - 10.0 - size.x).max(plot.left()) };
    let tip = Rect::from_min_size(pos2(left, plot.top() + 4.0), size);
    painter.rect(tip, CornerRadius::same(8), theme::RAISED, Stroke::new(1.0, theme::BORDER_HOVER), StrokeKind::Inside);
    let when_height = when.size().y;
    painter.galley(tip.min + vec2(10.0, 7.0), when, theme::TEXT_FAINT);
    painter.galley(tip.min + vec2(10.0, 9.0 + when_height), text, theme::TEXT);
}

/// Tiny framed chart without labels, for list rows.
pub fn thumbnail(painter: &egui::Painter, rect: Rect, series: &[Series], range: Option<(f64, f64)>, color: Color32) {
    painter.rect(
        rect,
        CornerRadius::same(5),
        theme::BG,
        Stroke::new(1.0, color.gamma_multiply(0.45)),
        StrokeKind::Inside,
    );
    let Some(first) = series.first() else { return };
    let bounds = bounds(extent(series.iter().filter_map(|s| s.history)), range, Some(first.unit));
    let plot = rect.shrink2(vec2(2.0, 3.0));
    for s in series.iter().rev() {
        if let Some(h) = s.history.filter(|h| h.len() >= 2) {
            plot_line(painter, plot, h, bounds, color, s.dashed);
        }
    }
}

fn grid(painter: &egui::Painter, rect: Rect, (columns, rows): (usize, usize)) {
    let stroke = Stroke::new(1.0, theme::TRACK);
    for i in 1..columns {
        painter.vline(rect.left() + rect.width() * i as f32 / columns as f32, rect.y_range(), stroke);
    }
    for i in 1..rows {
        painter.hline(rect.x_range(), rect.top() + rect.height() * i as f32 / rows as f32, stroke);
    }
    painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, theme::BORDER), StrokeKind::Outside);
}

/// Draws `values` right-aligned in `rect` (one step per sample of the full history).
/// Solid lines get a fading area underneath; dashed ones stay unfilled.
///
/// Right after new samples arrive the line still sits one step further right and
/// slides into place (see [`Timeline`]); the oldest sample slides out on the left.
fn plot_line(
    painter: &egui::Painter,
    rect: Rect,
    values: &VecDeque<f32>,
    (lo, hi): (f32, f32),
    color: Color32,
    dashed: bool,
) {
    let step = rect.width() / (HISTORY_LEN - 1) as f32;
    let shift = timeline(painter.ctx()).shift;
    if shift > 0.0 {
        // 30 fps keeps the slide smooth at half the cost of 60.
        painter.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
    // Clip only sideways, so strokes along the top and bottom stay whole.
    let clip = Rect::from_x_y_ranges(rect.x_range(), painter.clip_rect().y_range()).intersect(painter.clip_rect());
    let painter = painter.with_clip_rect(clip);
    let x0 = rect.right() - step * ((values.len() - 1) as f32 - shift);
    let points: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
            pos2(x0 + step * i as f32, rect.bottom() - t * rect.height())
        })
        .collect();

    if dashed {
        painter.extend(Shape::dashed_line(&points, Stroke::new(1.4, color), 4.0, 3.0));
        return;
    }
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

/// Single-line text cut off with "…" at `width`.
pub fn elided(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
    painter.layout_job(job)
}
