//! Components page: every component in a list. Clicking one opens it next to the
//! list, Task Manager style: a large history chart, all values, smaller charts, top
//! processes and details. Clicking it again (or Esc) closes it.

use eframe::egui::{
    self, Align2, CornerRadius, CursorIcon, Key, PointerButton, Rect, ScrollArea, Sense, Stroke, Ui, pos2, vec2,
};

use super::{grid_columns, theme, widgets};
use crate::poller::Model;
use crate::sensors::{Group, Kind, Reading, Unit};
use crate::settings::Settings;

#[derive(Default)]
pub struct State {
    /// [`Group::id`] of the opened component; `None` shows only the list.
    selected: Option<String>,
    /// Components with a breakdown (the CPU): one chart per item instead of the total.
    breakdown: bool,
}

impl State {
    /// Opens a component, e.g. when its name is clicked on the dashboard.
    pub fn open(&mut self, id: String) {
        self.selected = Some(id);
    }
}

const ROW_HEIGHT: f32 = 66.0;
const WIDE_ROW_HEIGHT: f32 = 78.0;
const GAP: f32 = 12.0;

pub fn page(ui: &mut Ui, model: &Model, settings: &Settings, state: &mut State) {
    let groups = settings.ordered(&model.groups);
    if groups.is_empty() {
        widgets::waiting(ui, widgets::SENSORS.0, widgets::SENSORS.1);
        return;
    }

    // A component that stopped reporting (unplugged drive) closes its details.
    let mut index = state.selected.as_ref().and_then(|id| groups.iter().position(|g| &g.id == id));
    let (up, down, back) = ui.input(|i| {
        (
            i.key_pressed(Key::ArrowUp),
            i.key_pressed(Key::ArrowDown),
            i.key_pressed(Key::Escape) || i.pointer.button_pressed(PointerButton::Extra1),
        )
    });
    if let Some(i) = index.as_mut() {
        if up {
            *i = i.saturating_sub(1);
        }
        if down {
            *i = (*i + 1).min(groups.len() - 1);
        }
    }
    if back {
        index = None;
    }

    let full = ui.available_rect_before_wrap();
    let open = index.is_some();
    let side = if open {
        let width = (full.width() * 0.27).clamp(180.0, 280.0);
        ui.painter().vline(full.left() + width + 12.0, full.y_range(), Stroke::new(1.0, theme::BORDER));
        Rect::from_min_max(full.min, pos2(full.left() + width, full.bottom()))
    } else {
        full
    };

    ui.scope_builder(egui::UiBuilder::new().max_rect(side), |ui| {
        ScrollArea::vertical().id_salt("components_list").auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = if open { 4.0 } else { 8.0 };
            if !open {
                ui.label(
                    egui::RichText::new(format!(
                        "{} components · select one for live charts, top processes and details",
                        groups.len()
                    ))
                    .font(theme::regular(12.5))
                    .color(theme::TEXT_FAINT),
                );
                ui.add_space(2.0);
            }
            for (i, group) in groups.iter().enumerate() {
                let selected = index == Some(i);
                if list_row(ui, group, model, selected, !open).clicked() {
                    index = if selected { None } else { Some(i) };
                }
            }
        });
    });

    state.selected = index.map(|i| groups[i].id.clone());
    if let Some(i) = index {
        let group = groups[i];
        let main = Rect::from_min_max(pos2(side.right() + 25.0, full.top()), full.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(main), |ui| {
            ScrollArea::vertical().id_salt(("components", &group.id)).auto_shrink(false).show(ui, |ui| {
                details(ui, group, model, state, main.height());
            });
        });
    }
    ui.allocate_rect(full, Sense::hover());
}

/// Entry in the component list: thumbnail of the main chart, name, type and key
/// values. `wide` when the list has the page to itself: a bigger thumbnail and the
/// key values in columns on the right instead of one summary line.
fn list_row(ui: &mut Ui, group: &Group, model: &Model, selected: bool, wide: bool) -> egui::Response {
    let height = if wide { WIDE_ROW_HEIGHT } else { ROW_HEIGHT };
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let hovered = response.hovered();
    let painter = ui.painter_at(rect);
    let color = theme::kind_color(group.kind);

    if wide {
        painter.rect(
            rect,
            CornerRadius::same(theme::CARD_RADIUS),
            if hovered { theme::RAISED } else { theme::SURFACE },
            Stroke::new(1.0, if hovered { theme::BORDER_HOVER } else { theme::BORDER }),
            egui::StrokeKind::Inside,
        );
    } else if selected || hovered {
        painter.rect_filled(rect, CornerRadius::same(10), if selected { theme::RAISED } else { theme::SURFACE });
    }
    if selected {
        let bar = Rect::from_center_size(pos2(rect.left() + 1.5, rect.center().y), vec2(3.0, 26.0));
        painter.rect_filled(bar, CornerRadius::same(2), color);
    }

    let (main, _) = charts(group);
    let thumb_size = if wide { vec2(96.0, 50.0) } else { vec2(62.0, 42.0) };
    let thumb = Rect::from_min_size(pos2(rect.left() + 14.0, rect.center().y - thumb_size.y / 2.0), thumb_size);
    widgets::thumbnail(&painter, thumb, &series(group, model, &main.readings), range(&main.readings), color);
    let x = thumb.right() + if wide { 16.0 } else { 12.0 };

    if !wide {
        let width = rect.right() - 8.0 - x;
        let lines = [
            (group.title.as_str(), theme::semibold(13.5), theme::TEXT, 9.0),
            (group.subtitle.as_str(), theme::regular(11.5), theme::TEXT_FAINT, 29.0),
            (&summary(group), theme::regular(11.5), theme::TEXT_DIM, 45.0),
        ];
        for (text, font, color, y) in lines {
            painter.galley(pos2(x, rect.top() + y), widgets::elided(&painter, text, font, color, width), color);
        }
        return response;
    }

    // Chevron on the right, nudged forward on hover, so the card reads as a button.
    let nudge = ui.ctx().animate_bool_with_time(response.id.with("hover"), hovered, 0.12) * 4.0;
    painter.text(
        pos2(rect.right() - 20.0 + nudge, rect.center().y),
        Align2::RIGHT_CENTER,
        "›",
        theme::regular(24.0),
        if hovered { theme::TEXT } else { theme::TEXT_FAINT },
    );

    // Kind, name and type on the left; key values right-aligned in columns, dropped
    // from the left when the window is too narrow for them next to the name.
    const COLUMN: f32 = 112.0;
    let key: Vec<&Reading> = group.readings.iter().filter(|r| r.dashboard).take(3).collect();
    let mut right = rect.right() - 48.0;
    let mut columns_left = right;
    let title_width = painter.layout_no_wrap(group.title.clone(), theme::semibold(15.0), theme::TEXT).size().x;
    for r in key.iter().rev() {
        if right - COLUMN < x + title_width.min(260.0) + 24.0 {
            break;
        }
        painter.text(
            pos2(right, rect.top() + 26.0),
            Align2::RIGHT_CENTER,
            r.label.to_uppercase(),
            theme::semibold(10.0),
            theme::TEXT_FAINT,
        );
        widgets::value_text(
            &painter,
            pos2(right, rect.top() + 49.0),
            (r.value, r.unit),
            18.0,
            11.0,
            Align2::RIGHT_CENTER,
        );
        columns_left = right - COLUMN;
        right = columns_left;
    }
    let width = columns_left - 16.0 - x;
    let lines = [
        (group.kind.label().to_uppercase(), theme::semibold(10.5), color, 14.0),
        (group.title.clone(), theme::semibold(15.0), theme::TEXT, 30.0),
        (group.subtitle.clone(), theme::regular(12.0), theme::TEXT_FAINT, 51.0),
    ];
    for (text, font, color, y) in lines {
        painter.galley(pos2(x, rect.top() + y), widgets::elided(&painter, &text, font, color, width), color);
    }
    response
}

/// Right-hand side: name, main chart (or one per thread), all values, smaller charts.
fn details(ui: &mut Ui, group: &Group, model: &Model, state: &mut State, view_height: f32) {
    let color = theme::kind_color(group.kind);
    ui.label(egui::RichText::new(group.kind.label().to_uppercase()).font(theme::semibold(11.0)).color(color));
    ui.add_space(-8.0);
    let title = ui.label(egui::RichText::new(&group.title).font(theme::semibold(24.0)).color(theme::TEXT)).rect;
    ui.add_space(-8.0);
    ui.label(egui::RichText::new(&group.subtitle).font(theme::regular(13.0)).color(theme::TEXT_FAINT));
    ui.add_space(10.0);

    let (main, rest) = charts(group);
    let chart_height = (view_height * 0.42).clamp(220.0, 400.0);

    // View switch right of the name, or on its own row when the name is too long.
    let breakdown = group.breakdown.as_ref();
    if let Some(b) = breakdown {
        let options = [main.title, b.title];
        let width = segmented_width(ui, &options);
        let right = ui.max_rect().right();
        let bar = if title.right() + 24.0 + width <= right {
            Rect::from_min_size(pos2(right - width, title.center().y - 15.0), vec2(width, 30.0))
        } else {
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
            ui.add_space(4.0);
            Rect::from_min_size(pos2(right - width, row.top()), vec2(width, 30.0))
        };
        if let Some(choice) = segmented(ui, bar, &options, state.breakdown as usize) {
            state.breakdown = choice == 1;
        }
    }

    match breakdown.filter(|_| state.breakdown) {
        Some(b) => {
            ui.label(egui::RichText::new(b.subtitle).font(theme::regular(12.0)).color(theme::TEXT_FAINT));
            ui.add_space(2.0);
            let per_row = grid_columns(ui.available_width(), b.items.len());
            let rows = b.items.len().div_ceil(per_row);
            let cell = ((chart_height - GAP * (rows - 1) as f32) / rows as f32).max(96.0);
            for row in b.items.chunks(per_row) {
                ui.columns(per_row, |cols| {
                    for (col, item) in cols.iter_mut().zip(row) {
                        let history = model.history.get(&format!("{}/#{}", group.id, item.label));
                        let s = widgets::Series {
                            label: &item.label,
                            history,
                            value: item.value,
                            unit: b.unit,
                            dashed: false,
                        };
                        widgets::chart(col, cell, &item.label, item.detail.as_deref(), &[s], b.range, color, true);
                    }
                });
            }
        }
        None => {
            let s = series(group, model, &main.readings);
            widgets::chart(ui, chart_height, main.title, None, &s, range(&main.readings), color, false);
        }
    }

    ui.add_space(6.0);
    values(ui, group);
    ui.add_space(10.0);

    let per_row = if ui.available_width() >= 900.0 { 3 } else { 2 };
    for row in rest.chunks(per_row) {
        ui.columns(per_row, |cols| {
            for (col, chart) in cols.iter_mut().zip(row) {
                let s = series(group, model, &chart.readings);
                widgets::chart(col, 150.0, chart.title, None, &s, range(&chart.readings), color, true);
            }
        });
    }

    if let Some(processes) = group.processes.as_ref() {
        ui.add_space(14.0);
        super::subsection(ui, "Top processes", &format!("By {}, refreshed every second", processes.metric));
        super::processes_card(ui, processes, color);
    }
    if !group.info.is_empty() {
        ui.add_space(14.0);
        super::subsection(ui, "Details", "");
        super::details_card(ui, &group.info);
    }
    ui.add_space(14.0);
}

/// Every reading as label over a large value, flowing in columns like Task Manager.
fn values(ui: &mut Ui, group: &Group) {
    const WIDTH: f32 = 170.0;
    const HEIGHT: f32 = 74.0;
    let width = ui.available_width();
    let per_row = ((width / WIDTH) as usize).max(1);
    let rows = group.readings.len().div_ceil(per_row);
    let (rect, _) = ui.allocate_exact_size(vec2(width, rows as f32 * HEIGHT), Sense::hover());
    let painter = ui.painter_at(rect);
    let column = width / per_row as f32;

    for (i, r) in group.readings.iter().enumerate() {
        let origin = rect.min + vec2((i % per_row) as f32 * column, (i / per_row) as f32 * HEIGHT + 8.0);
        painter.text(origin, Align2::LEFT_TOP, r.label, theme::regular(12.0), theme::TEXT_DIM);
        let value =
            widgets::value_text(&painter, origin + vec2(0.0, 17.0), (r.value, r.unit), 24.0, 12.5, Align2::LEFT_TOP);
        if let Some(detail) = &r.detail {
            let galley = widgets::elided(&painter, detail, theme::regular(11.5), theme::TEXT_FAINT, column - 12.0);
            painter.galley(pos2(origin.x, value.bottom() + 1.0), galley, theme::TEXT_FAINT);
        }
    }
}

fn segment_widths(ui: &Ui, options: &[&str]) -> Vec<f32> {
    options
        .iter()
        .map(|t| ui.painter().layout_no_wrap(t.to_string(), theme::semibold(12.0), theme::TEXT).size().x + 26.0)
        .collect()
}

fn segmented_width(ui: &Ui, options: &[&str]) -> f32 {
    segment_widths(ui, options).iter().sum::<f32>() + 6.0
}

/// Small pill switch between views, drawn in `bar`; returns the clicked option.
fn segmented(ui: &mut Ui, bar: Rect, options: &[&str], selected: usize) -> Option<usize> {
    let font = theme::semibold(12.0);
    let widths = segment_widths(ui, options);
    let painter = ui.painter();
    painter.rect_filled(bar, CornerRadius::same(15), theme::SURFACE);

    let mut clicked = None;
    let mut x = bar.left() + 3.0;
    for (i, (title, w)) in options.iter().zip(widths).enumerate() {
        let tab = Rect::from_min_size(pos2(x, bar.top() + 3.0), vec2(w, 24.0));
        x += w;
        let response =
            ui.interact(tab, ui.scope_id().with(("view", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        if i == selected {
            painter.rect_filled(tab, CornerRadius::same(12), theme::RAISED);
        }
        let color = if i == selected || response.hovered() { theme::TEXT } else { theme::TEXT_DIM };
        painter.text(tab.center(), Align2::CENTER_CENTER, *title, font.clone(), color);
        if response.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// Readings drawn together in one chart.
struct Chart<'a> {
    title: &'a str,
    readings: Vec<&'a Reading>,
}

/// The main chart (load, active time, memory in use, network throughput) and one
/// smaller chart per remaining reading; throughputs of one unit share a chart.
fn charts(group: &Group) -> (Chart<'_>, Vec<Chart<'_>>) {
    let main: Vec<&Reading> = if group.kind == Kind::Network {
        group.readings.iter().filter(|r| r.unit == Unit::BitsPerSec).collect()
    } else {
        ["load", "active", "used"]
            .iter()
            .find_map(|id| group.readings.iter().find(|r| r.id == *id))
            .or(group.readings.first())
            .into_iter()
            .collect()
    };
    let title = if main.len() > 1 { "Throughput" } else { main.first().map_or("", |r| r.label) };

    let mut rest: Vec<Chart> = Vec::new();
    for r in group.readings.iter().filter(|r| !main.iter().any(|m| m.id == r.id)) {
        if is_throughput(r.unit)
            && let Some(chart) = rest.iter_mut().find(|c| c.readings[0].unit == r.unit)
        {
            chart.title = "Transfer rate";
            chart.readings.push(r);
            continue;
        }
        rest.push(Chart { title: r.label, readings: vec![r] });
    }
    (Chart { title, readings: main }, rest)
}

fn is_throughput(unit: Unit) -> bool {
    matches!(unit, Unit::BytesPerSec | Unit::BitsPerSec)
}

fn series<'a>(group: &Group, model: &'a Model, readings: &[&'a Reading]) -> Vec<widgets::Series<'a>> {
    readings
        .iter()
        .enumerate()
        .map(|(i, r)| widgets::Series {
            label: r.label,
            history: model.history.get(&format!("{}/{}", group.id, r.id)),
            value: r.value,
            unit: r.unit,
            dashed: i > 0,
        })
        .collect()
}

/// Fixed scale of a chart. Throughput scales to its peak instead of the link speed,
/// otherwise everyday traffic would be a flat line at the bottom.
fn range(readings: &[&Reading]) -> Option<(f64, f64)> {
    readings.first().filter(|r| !is_throughput(r.unit)).and_then(|r| r.range)
}

/// Key values for the list row, e.g. `"14% · 4.55 GHz"`.
fn summary(group: &Group) -> String {
    let get = |id: &str| group.readings.iter().find(|r| r.id == id);
    let fmt = |r: &Reading| widgets::format_value(r.value, r.unit);
    let parts: Vec<String> = match group.kind {
        Kind::Cpu => ["load", "clock"].iter().filter_map(|id| get(id)).map(fmt).collect(),
        Kind::Gpu | Kind::IntegratedGpu => ["load", "temp"].iter().filter_map(|id| get(id)).map(fmt).collect(),
        Kind::Memory => get("used")
            .map(|r| {
                let mut parts = vec![fmt(r)];
                if let Some((_, total)) = r.range.filter(|(_, t)| *t > 0.0) {
                    parts.push(format!("{:.0}%", r.value / total * 100.0));
                }
                parts
            })
            .unwrap_or_default(),
        Kind::Network => group
            .readings
            .iter()
            .filter(|r| r.unit == Unit::BitsPerSec)
            .zip(["↓", "↑"])
            .map(|(r, arrow)| format!("{arrow} {}", fmt(r)))
            .collect(),
        Kind::Disk | Kind::ExternalDisk => {
            ["active", "read", "write"].iter().filter_map(|id| get(id)).take(1).map(fmt).collect()
        }
        Kind::Battery => ["charge", "rate"].iter().filter_map(|id| get(id)).map(fmt).collect(),
    };
    parts.join(" · ")
}
