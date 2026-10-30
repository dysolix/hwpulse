//! Processes page: every running process in a sortable, searchable table, Task
//! Manager's Details tab. Only the visible rows are laid out and drawn, so hundreds
//! of processes cost no more than a screenful. Rows can be grouped per executable.
//! Selected processes can be ended, suspended and resumed; without admin rights that
//! fails for elevated and SYSTEM processes, and the page offers to restart as admin.

use std::cmp::Ordering;
use std::collections::HashSet;

use eframe::egui::{
    self, Align2, CornerRadius, CursorIcon, Frame, Key, Margin, Rect, ScrollArea, Sense, Stroke, Ui, pos2, vec2,
};

use super::{theme, widgets};
use crate::poller::Model;
use crate::sensors::{ProcessRow, Unit};
use crate::tasks::{self, Action, Target};

const ROW: f32 = 28.0;
const HEADER: f32 = 30.0;
const PAD: f32 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Column {
    Name,
    Pid,
    Status,
    User,
    Cpu,
    Memory,
    Disk,
    Threads,
    Handles,
    Architecture,
    Description,
}

impl Column {
    const ALL: [Column; 11] = [
        Column::Name,
        Column::Pid,
        Column::Status,
        Column::User,
        Column::Cpu,
        Column::Memory,
        Column::Disk,
        Column::Threads,
        Column::Handles,
        Column::Architecture,
        Column::Description,
    ];
    /// Dropped from the end first when the window gets narrow.
    const HIDE_ORDER: [Column; 7] = [
        Column::Description,
        Column::Architecture,
        Column::Handles,
        Column::Threads,
        Column::User,
        Column::Status,
        Column::Disk,
    ];

    fn title(self) -> &'static str {
        match self {
            Column::Name => "Name",
            Column::Pid => "PID",
            Column::Status => "Status",
            Column::User => "User",
            Column::Cpu => "CPU",
            Column::Memory => "Memory",
            Column::Disk => "Disk",
            Column::Threads => "Threads",
            Column::Handles => "Handles",
            Column::Architecture => "Arch",
            Column::Description => "Description",
        }
    }

    /// Minimum width; Name and Description share whatever is left.
    fn width(self) -> f32 {
        match self {
            Column::Name => 170.0,
            Column::Pid => 64.0,
            Column::Status => 80.0,
            Column::User => 110.0,
            Column::Cpu => 64.0,
            Column::Memory => 86.0,
            Column::Disk => 86.0,
            Column::Threads => 62.0,
            Column::Handles => 66.0,
            Column::Architecture => 58.0,
            Column::Description => 150.0,
        }
    }

    fn numeric(self) -> bool {
        matches!(self, Column::Pid | Column::Cpu | Column::Memory | Column::Disk | Column::Threads | Column::Handles)
    }

    fn flexible(self) -> bool {
        matches!(self, Column::Name | Column::Description)
    }
}

/// What the toolbar and context menu act on.
#[derive(Clone, Debug, PartialEq)]
enum Selected {
    Process(Target),
    /// Every process of an executable, by lowercase name.
    Group(String),
}

/// An action waiting for confirmation in a dialog.
struct Pending {
    action: Action,
    targets: Vec<Target>,
    label: String,
    critical: bool,
}

pub struct State {
    query: String,
    sort: Column,
    descending: bool,
    grouped: bool,
    /// Lowercase names of expanded groups.
    expanded: HashSet<String>,
    selected: Option<Selected>,
    pending: Option<Pending>,
    /// Result of the last action; `true` when it failed for lack of rights.
    message: Option<(String, bool)>,
    elevated: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            query: String::new(),
            sort: Column::Cpu,
            descending: true,
            grouped: false,
            expanded: HashSet::new(),
            selected: None,
            pending: None,
            message: None,
            elevated: tasks::is_elevated(),
        }
    }
}

/// One line of the table: a process, or several processes of one executable.
enum Line<'a> {
    Process { row: &'a ProcessRow, child: bool },
    Group { key: String, rows: Vec<&'a ProcessRow> },
}

impl Line<'_> {
    fn rows(&self) -> Vec<&ProcessRow> {
        match self {
            Line::Process { row, .. } => vec![row],
            Line::Group { rows, .. } => rows.clone(),
        }
    }

    fn selection(&self) -> Selected {
        match self {
            Line::Process { row, .. } => Selected::Process(target(row)),
            Line::Group { key, .. } => Selected::Group(key.clone()),
        }
    }
}

fn target(row: &ProcessRow) -> Target {
    Target { pid: row.process.pid, create_time: row.process.create_time }
}

pub fn page(ui: &mut Ui, model: &Model, state: &mut State) {
    let Some(rows) = &model.processes else {
        widgets::waiting(ui);
        return;
    };

    let query = state.query.trim().to_lowercase();
    let matches = |r: &ProcessRow| {
        query.is_empty()
            || r.process.name.to_lowercase().contains(&query)
            || r.process.pid.to_string().starts_with(&query)
            || r.facts.user.as_ref().is_some_and(|u| u.to_lowercase().contains(&query))
            || r.facts.description.as_ref().is_some_and(|d| d.to_lowercase().contains(&query))
    };
    let mut visible: Vec<&ProcessRow> = rows.iter().filter(|r| matches(r)).collect();
    let (sort, descending) = (state.sort, state.descending);
    let order = |a: &ProcessRow, b: &ProcessRow| {
        let o = compare(sort, a, b);
        let o = if descending { o.reverse() } else { o };
        o.then_with(|| a.process.name.to_lowercase().cmp(&b.process.name.to_lowercase()))
            .then(a.process.pid.cmp(&b.process.pid))
    };
    visible.sort_by(|a, b| order(a, b));
    let lines = lines(&visible, state, &order);

    // Forget a selection whose processes ended.
    let selected_rows: Vec<&ProcessRow> = match &state.selected {
        Some(Selected::Process(t)) => rows.iter().filter(|r| target(r) == *t).collect(),
        Some(Selected::Group(key)) => rows.iter().filter(|r| r.process.name.to_lowercase() == *key).collect(),
        None => Vec::new(),
    };
    if selected_rows.is_empty() {
        state.selected = None;
    }

    toolbar(ui, state, rows.len(), visible.len(), &selected_rows);
    if let Some((text, denied)) = state.message.clone() {
        message(ui, state, &text, denied);
    }

    let (delete, escape) = ui.input(|i| (i.key_pressed(Key::Delete), i.key_pressed(Key::Escape)));
    if delete && !selected_rows.is_empty() && state.pending.is_none() {
        request(state, Action::End, &selected_rows);
    }
    if escape && state.pending.is_none() {
        state.selected = None;
    }

    table(ui, state, &lines);
    confirm(ui, state);
}

/// Groups per executable when switched on; a group of one stays a plain row.
fn lines<'a>(
    visible: &[&'a ProcessRow],
    state: &State,
    order: &dyn Fn(&ProcessRow, &ProcessRow) -> Ordering,
) -> Vec<Line<'a>> {
    if !state.grouped {
        return visible.iter().map(|&row| Line::Process { row, child: false }).collect();
    }
    let mut groups: Vec<(String, Vec<&ProcessRow>)> = Vec::new();
    for &row in visible {
        let key = row.process.name.to_lowercase();
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, rows)) => rows.push(row),
            None => groups.push((key, vec![row])),
        }
    }
    // Groups by their total, using a stand-in row that holds the sums.
    let totals: Vec<ProcessRow> = groups.iter().map(|(_, rows)| sum(rows)).collect();
    let mut indices: Vec<usize> = (0..groups.len()).collect();
    indices.sort_by(|&a, &b| order(&totals[a], &totals[b]));

    let mut lines = Vec::new();
    for i in indices {
        let (key, rows) = &groups[i];
        if rows.len() == 1 {
            lines.push(Line::Process { row: rows[0], child: false });
            continue;
        }
        let open = state.expanded.contains(key);
        lines.push(Line::Group { key: key.clone(), rows: rows.clone() });
        if open {
            lines.extend(rows.iter().map(|&row| Line::Process { row, child: true }));
        }
    }
    lines
}

/// A row with the summed values of `rows`, for sorting and showing a group.
fn sum(rows: &[&ProcessRow]) -> ProcessRow {
    let mut total = rows[0].clone();
    let p = &mut total.process;
    p.cpu = rows.iter().map(|r| r.process.cpu).sum();
    p.private_bytes = rows.iter().map(|r| r.process.private_bytes).sum();
    p.io_bytes_per_sec = rows.iter().map(|r| r.process.io_bytes_per_sec).sum();
    p.threads = rows.iter().map(|r| r.process.threads).sum();
    p.handles = rows.iter().map(|r| r.process.handles).sum();
    p.suspended = rows.iter().all(|r| r.process.suspended);
    total
}

fn compare(column: Column, a: &ProcessRow, b: &ProcessRow) -> Ordering {
    let (pa, pb) = (&a.process, &b.process);
    let text = |x: &Option<String>, y: &Option<String>| {
        x.as_deref().unwrap_or("").to_lowercase().cmp(&y.as_deref().unwrap_or("").to_lowercase())
    };
    match column {
        Column::Name => pa.name.to_lowercase().cmp(&pb.name.to_lowercase()),
        Column::Pid => pa.pid.cmp(&pb.pid),
        Column::Status => pa.suspended.cmp(&pb.suspended),
        Column::User => text(&a.facts.user, &b.facts.user),
        Column::Cpu => pa.cpu.total_cmp(&pb.cpu),
        Column::Memory => pa.private_bytes.cmp(&pb.private_bytes),
        Column::Disk => pa.io_bytes_per_sec.total_cmp(&pb.io_bytes_per_sec),
        Column::Threads => pa.threads.cmp(&pb.threads),
        Column::Handles => pa.handles.cmp(&pb.handles),
        Column::Architecture => a.facts.architecture.cmp(&b.facts.architecture),
        Column::Description => text(&a.facts.description, &b.facts.description),
    }
}

// ---------------------------------------------------------------------------
// Toolbar

fn toolbar(ui: &mut Ui, state: &mut State, total: usize, shown: usize, selected: &[&ProcessRow]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .hint_text("Search name, PID, user or description")
                .desired_width(280.0)
                .font(theme::regular(13.0))
                .frame(
                    Frame::new()
                        .fill(theme::SURFACE)
                        .stroke(Stroke::new(1.0, theme::BORDER))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(10, 6)),
                ),
        );
        if widgets::chip(ui, "Group by app", state.grouped, true, theme::ACCENT).clicked() {
            state.grouped = !state.grouped;
        }
        let count = if shown == total { format!("{total} processes") } else { format!("{shown} of {total}") };
        ui.label(egui::RichText::new(count).font(theme::regular(12.5)).color(theme::TEXT_FAINT));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let any = !selected.is_empty();
            if button(ui, "End task", any, true).clicked() {
                request(state, Action::End, selected);
            }
            let suspended = any && selected.iter().all(|r| r.process.suspended);
            let (label, action) = if suspended { ("Resume", Action::Resume) } else { ("Suspend", Action::Suspend) };
            if button(ui, label, any, false).clicked() {
                request(state, action, selected);
            }
            if state.elevated {
                ui.label(egui::RichText::new("Administrator").font(theme::semibold(12.0)).color(theme::LIVE));
            }
        });
    });
    ui.add_space(-2.0);
}

fn button(ui: &mut Ui, label: &str, enabled: bool, danger: bool) -> egui::Response {
    let font = theme::semibold(12.5);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, theme::TEXT);
    let size = vec2(galley.size().x + 28.0, 30.0);
    let (rect, response) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && response.hovered();
    let (fill, text) = match (enabled, danger) {
        (false, _) => (theme::SURFACE, theme::TEXT_FAINT),
        (true, true) => (theme::ERROR.gamma_multiply(if hovered { 0.35 } else { 0.22 }), theme::TEXT),
        (true, false) => (if hovered { theme::RAISED } else { theme::SURFACE }, theme::TEXT),
    };
    ui.painter().rect(rect, CornerRadius::same(8), fill, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
    ui.painter().galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, text);
    if enabled { response.on_hover_cursor(CursorIcon::PointingHand) } else { response }
}

/// The outcome of the last action, with a way to retry as administrator.
fn message(ui: &mut Ui, state: &mut State, text: &str, denied: bool) {
    Frame::new()
        .fill(if denied { theme::ERROR.gamma_multiply(0.12) } else { theme::SURFACE })
        .stroke(Stroke::new(1.0, if denied { theme::ERROR.gamma_multiply(0.5) } else { theme::BORDER }))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(text).font(theme::regular(12.5)).color(theme::TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if super::link(ui, "Dismiss", theme::regular(12.5), theme::TEXT_DIM, false).clicked() {
                        state.message = None;
                    }
                    if denied
                        && !state.elevated
                        && super::link(ui, "Restart as administrator", theme::semibold(12.5), theme::ACCENT, true)
                            .clicked()
                    {
                        if tasks::restart_as_admin("processes") {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        } else {
                            state.message = Some(("The administrator prompt was declined.".into(), false));
                        }
                    }
                });
            });
        });
}

// ---------------------------------------------------------------------------
// Table

/// Visible columns with their widths for the available `width`.
fn layout(width: f32) -> Vec<(Column, f32)> {
    let mut columns: Vec<Column> = Column::ALL.to_vec();
    let needed = |cols: &[Column]| cols.iter().map(|c| c.width()).sum::<f32>() + PAD * cols.len() as f32;
    for hide in Column::HIDE_ORDER {
        if needed(&columns) <= width {
            break;
        }
        columns.retain(|c| *c != hide);
    }
    let extra = (width - needed(&columns)).max(0.0);
    let flexible = columns.iter().filter(|c| c.flexible()).count().max(1) as f32;
    columns.into_iter().map(|c| (c, c.width() + if c.flexible() { extra / flexible } else { 0.0 })).collect()
}

fn table(ui: &mut Ui, state: &mut State, lines: &[Line]) {
    let full = ui.available_rect_before_wrap();
    Frame::new()
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(theme::CARD_RADIUS))
        .inner_margin(Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_min_size(full.size() - vec2(14.0, 10.0));
            let columns = layout(ui.available_width() - 12.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            header(ui, state, &columns);
            ScrollArea::vertical().id_salt("processes").auto_shrink(false).show_rows(
                ui,
                ROW,
                lines.len(),
                |ui, range| {
                    for line in &lines[range] {
                        row(ui, state, line, &columns);
                    }
                },
            );
        });
}

fn header(ui: &mut Ui, state: &mut State, columns: &[(Column, f32)]) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, theme::BORDER));
    let mut x = rect.left() + PAD;
    for &(column, width) in columns {
        let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(width, HEADER));
        x += width + PAD;
        let response = ui
            .interact(cell, ui.scope_id().with(("header", column)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        if response.clicked() {
            if state.sort == column {
                state.descending = !state.descending;
            } else {
                state.sort = column;
                state.descending = column.numeric() && column != Column::Pid;
            }
        }
        let active = state.sort == column;
        let mut title = column.title().to_owned();
        if active {
            title += if state.descending { "  ↓" } else { "  ↑" };
        }
        let color = if active || response.hovered() { theme::TEXT } else { theme::TEXT_DIM };
        let (anchor, align) = cell_anchor(cell, column);
        ui.painter().text(anchor, align, title, theme::semibold(12.0), color);
    }
}

fn cell_anchor(cell: Rect, column: Column) -> (egui::Pos2, Align2) {
    if column.numeric() {
        (cell.right_center(), Align2::RIGHT_CENTER)
    } else {
        (cell.left_center(), Align2::LEFT_CENTER)
    }
}

fn row(ui: &mut Ui, state: &mut State, line: &Line, columns: &[(Column, f32)]) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let selection = line.selection();
    let selected = state.selected.as_ref() == Some(&selection);
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, CornerRadius::same(6), theme::ACCENT.gamma_multiply(0.16));
    } else if response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), theme::RAISED);
    }

    if response.clicked() || response.secondary_clicked() {
        state.selected = Some(selection.clone());
    }
    if let Line::Group { key, .. } = line
        && (response.double_clicked() || response.clicked() && chevron_hit(ui, rect))
        && !state.expanded.remove(key)
    {
        state.expanded.insert(key.clone());
    }
    response.context_menu(|ui| {
        state.selected = Some(selection.clone());
        let rows = line.rows();
        let suspended = rows.iter().all(|r| r.process.suspended);
        if ui.button("End task").clicked() {
            request(state, Action::End, &rows);
            ui.close();
        }
        let (label, action) = if suspended { ("Resume", Action::Resume) } else { ("Suspend", Action::Suspend) };
        if ui.button(label).clicked() {
            request(state, action, &rows);
            ui.close();
        }
        if ui.button("Copy name").clicked() {
            ui.ctx().copy_text(rows[0].process.name.clone());
            ui.close();
        }
        if let Some(path) = rows[0].facts.path.clone()
            && ui.button("Copy path").clicked()
        {
            ui.ctx().copy_text(path);
            ui.close();
        }
    });

    let (rows, child) = match line {
        Line::Process { row, child } => (vec![*row], *child),
        Line::Group { rows, .. } => (rows.clone(), false),
    };
    let group = matches!(line, Line::Group { .. });
    let shown = if group { sum(&rows) } else { rows[0].clone() };
    let (p, f) = (&shown.process, &shown.facts);

    let mut x = rect.left() + PAD;
    for &(column, width) in columns {
        let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(width, ROW));
        x += width + PAD;
        let faint = |s: &str| (s.to_owned(), theme::TEXT_FAINT);
        let (text, color) = match column {
            Column::Name => {
                let mut left = cell.left();
                if child {
                    left += 22.0;
                }
                if let Line::Group { key, .. } = line {
                    chevron(&painter, pos2(left + 5.0, cell.center().y), state.expanded.contains(key));
                    left += 18.0;
                }
                let name = if group { format!("{}  ×{}", p.name, rows.len()) } else { p.name.clone() };
                let font = if group { theme::semibold(13.0) } else { theme::regular(13.0) };
                let galley = widgets::elided(&painter, &name, font, theme::TEXT, cell.right() - left);
                painter.galley(pos2(left, cell.center().y - galley.size().y / 2.0), galley, theme::TEXT);
                continue;
            }
            Column::Pid if group => faint(""),
            Column::Pid => (p.pid.to_string(), theme::TEXT_DIM),
            Column::Status if p.suspended => ("Suspended".into(), theme::ACCENT),
            Column::Status => ("Running".into(), theme::TEXT_DIM),
            Column::User => match (&f.user, f.elevated) {
                (Some(user), Some(true)) => (format!("{user} · admin"), theme::TEXT_DIM),
                (Some(user), _) => (user.clone(), theme::TEXT_DIM),
                (None, _) => faint("—"),
            },
            Column::Cpu if p.cpu < 0.05 => faint("0"),
            Column::Cpu => (format!("{:.1}%", p.cpu), theme::accent(Unit::Percent, p.cpu * 4.0)),
            Column::Memory => (widgets::format_value(p.private_bytes as f64 / 1048576.0, Unit::Mebibytes), theme::TEXT),
            Column::Disk if p.io_bytes_per_sec < 512.0 => faint("0"),
            Column::Disk => (widgets::format_value(p.io_bytes_per_sec, Unit::BytesPerSec), theme::TEXT),
            Column::Threads => (p.threads.to_string(), theme::TEXT_DIM),
            Column::Handles => (p.handles.to_string(), theme::TEXT_DIM),
            Column::Architecture => match f.architecture {
                Some(a) => (a.to_owned(), theme::TEXT_DIM),
                None => faint("—"),
            },
            Column::Description => match &f.description {
                Some(d) => (d.clone(), theme::TEXT_DIM),
                None => faint(""),
            },
        };
        let galley = widgets::elided(&painter, &text, theme::regular(12.5), color, width);
        let (anchor, align) = cell_anchor(cell, column);
        let pos = align.anchor_size(anchor, galley.size()).min;
        painter.galley(pos, galley, color);
    }
}

/// Whether the pointer is over the expand chevron at the start of a group row.
fn chevron_hit(ui: &Ui, rect: Rect) -> bool {
    ui.ctx().pointer_interact_pos().is_some_and(|p| p.x < rect.left() + PAD + 22.0)
}

fn chevron(painter: &egui::Painter, center: egui::Pos2, open: bool) {
    let angle = if open { std::f32::consts::FRAC_PI_2 } else { 0.0 };
    let rot = egui::emath::Rot2::from_angle(angle);
    let arm = |v: egui::Vec2| center + rot * v;
    painter.line(
        vec![arm(vec2(-2.5, -4.5)), arm(vec2(2.0, 0.0)), arm(vec2(-2.5, 4.5))],
        Stroke::new(1.5, theme::TEXT_DIM),
    );
}

// ---------------------------------------------------------------------------
// Actions

/// Asks for confirmation before ending; suspending and resuming run right away.
fn request(state: &mut State, action: Action, rows: &[&ProcessRow]) {
    let targets: Vec<Target> = rows.iter().map(|r| target(r)).collect();
    let name = rows.first().map(|r| r.process.name.clone()).unwrap_or_default();
    let label = if rows.len() > 1 { format!("{} processes of {name}", rows.len()) } else { name };
    match action {
        Action::End => {
            let critical = targets.iter().any(|t| tasks::is_critical(*t) == Some(true));
            state.pending = Some(Pending { action, targets, label, critical });
        }
        _ => execute(state, action, &targets, &label),
    }
}

fn execute(state: &mut State, action: Action, targets: &[Target], label: &str) {
    let mut denied = 0;
    let mut other = None;
    for &t in targets {
        match tasks::run(action, t) {
            Ok(()) | Err(tasks::Error::Gone) => {}
            Err(tasks::Error::AccessDenied) => denied += 1,
            Err(tasks::Error::Other(e)) => other = Some(e),
        }
    }
    let verb = match action {
        Action::End => "end",
        Action::Suspend => "suspend",
        Action::Resume => "resume",
    };
    state.message = if denied > 0 {
        let hint = if state.elevated {
            "It is protected by Windows."
        } else {
            "It runs as administrator, as SYSTEM or as another user."
        };
        Some((format!("Access denied: could not {verb} {label}. {hint}"), !state.elevated))
    } else {
        other.map(|e| (format!("Could not {verb} {label}: {e}"), false))
    };
}

fn confirm(ui: &mut Ui, state: &mut State) {
    let Some(pending) = &state.pending else { return };
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::NULL.with("confirm_end")).show(ui.ctx(), |ui| {
        ui.set_width(380.0);
        ui.label(egui::RichText::new(format!("End {}?", pending.label)).font(theme::semibold(16.0)));
        ui.add_space(-4.0);
        ui.label(egui::RichText::new("Unsaved data in it is lost.").font(theme::regular(12.5)).color(theme::TEXT_DIM));
        if pending.critical {
            ui.label(
                egui::RichText::new("This is a critical Windows process. Ending it crashes Windows.")
                    .font(theme::semibold(12.5))
                    .color(theme::ERROR),
            );
        }
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if button(ui, "End process", true, true).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                    decision = Some(true);
                }
                if button(ui, "Cancel", true, false).clicked() {
                    decision = Some(false);
                }
            });
        });
    });
    if modal.should_close() && decision.is_none() {
        decision = Some(false);
    }
    if let Some(go) = decision {
        let pending = state.pending.take().expect("checked above");
        if go {
            execute(state, pending.action, &pending.targets, &pending.label);
        }
    }
}
