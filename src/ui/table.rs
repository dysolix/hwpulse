//! Pieces shared by the table pages (Processes, Services, Startup apps, Users):
//! column layout, the sortable header, row backgrounds, toolbar buttons, the search
//! field, the notice banner and work that runs off the UI thread.

use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Frame, Margin, Painter, Rect, Response, Sense, Stroke, Ui, pos2,
    vec2,
};

use super::{theme, widgets};
use crate::tasks;

pub const ROW: f32 = 28.0;
pub const HEADER: f32 = 30.0;
pub const PAD: f32 = 10.0;

pub trait Column: Copy + PartialEq + std::hash::Hash + std::fmt::Debug + 'static {
    fn title(self) -> &'static str;
    /// Minimum width; flexible columns share whatever is left.
    fn width(self) -> f32;
    /// Right-aligned, and sorted largest first.
    fn numeric(self) -> bool {
        false
    }
    fn flexible(self) -> bool {
        false
    }
}

#[derive(Clone, Copy)]
pub struct Sort<C> {
    pub column: C,
    pub descending: bool,
}

impl<C: Column> Sort<C> {
    pub fn apply(&self, o: std::cmp::Ordering) -> std::cmp::Ordering {
        if self.descending { o.reverse() } else { o }
    }
}

/// `columns` with widths for `width`, dropping columns in `hide_order` while they do
/// not fit.
pub fn layout<C: Column>(columns: &[C], hide_order: &[C], width: f32) -> Vec<(C, f32)> {
    let mut columns = columns.to_vec();
    let needed = |cols: &[C]| cols.iter().map(|c| c.width()).sum::<f32>() + PAD * cols.len() as f32;
    for &hide in hide_order {
        if needed(&columns) <= width {
            break;
        }
        columns.retain(|c| *c != hide);
    }
    let extra = (width - needed(&columns)).max(0.0);
    let flexible = columns.iter().filter(|c| c.flexible()).count().max(1) as f32;
    columns.into_iter().map(|c| (c, c.width() + if c.flexible() { extra / flexible } else { 0.0 })).collect()
}

/// The table's card, filling the rest of the page; `add` draws header and rows.
pub fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let full = ui.available_rect_before_wrap();
    Frame::new()
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(theme::CARD_RADIUS))
        .inner_margin(Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_min_size(full.size() - vec2(14.0, 10.0));
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
}

/// Column titles; clicking one sorts by it, clicking again flips the direction.
pub fn header<C: Column>(ui: &mut Ui, sort: &mut Sort<C>, columns: &[(C, f32)]) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, theme::BORDER));
    for (column, cell) in cells(rect, columns) {
        let response = ui
            .interact(cell, ui.scope_id().with(("header", column)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        if response.clicked() {
            if sort.column == column {
                sort.descending = !sort.descending;
            } else {
                *sort = Sort { column, descending: column.numeric() };
            }
        }
        let active = sort.column == column;
        let mut title = column.title().to_owned();
        if active {
            title += if sort.descending { "  ↓" } else { "  ↑" };
        }
        let color = if active || response.hovered() { theme::TEXT } else { theme::TEXT_DIM };
        let (anchor, align) = anchor(cell, column);
        ui.painter().text(anchor, align, title, theme::semibold(12.0), color);
    }
}

/// Each column's cell within a row (or the header).
pub fn cells<C: Column>(row: Rect, columns: &[(C, f32)]) -> impl Iterator<Item = (C, Rect)> + '_ {
    let mut x = row.left() + PAD;
    columns.iter().map(move |&(column, width)| {
        let cell = Rect::from_min_size(pos2(x, row.top()), vec2(width, row.height()));
        x += width + PAD;
        (column, cell)
    })
}

fn anchor<C: Column>(cell: Rect, column: C) -> (egui::Pos2, Align2) {
    if column.numeric() {
        (cell.right_center(), Align2::RIGHT_CENTER)
    } else {
        (cell.left_center(), Align2::LEFT_CENTER)
    }
}

/// A clickable row with hover and selection background.
pub fn row(ui: &mut Ui, selected: bool) -> (Rect, Response, Painter) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, CornerRadius::same(6), theme::ACCENT.gamma_multiply(0.16));
    } else if response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), theme::RAISED);
    }
    (rect, response, painter)
}

/// Text in a cell, elided to fit, aligned by the column kind.
pub fn text<C: Column>(painter: &Painter, cell: Rect, column: C, text: &str, color: Color32) {
    let galley = widgets::elided(painter, text, theme::regular(12.5), color, cell.width());
    let (anchor, align) = anchor(cell, column);
    painter.galley(align.anchor_size(anchor, galley.size()).min, galley, color);
}

/// A name cell: optional chevron for expandable rows, indented by `depth`.
pub fn name(painter: &Painter, cell: Rect, text: &str, depth: u8, chevron: Option<bool>, bold: bool) {
    let mut left = cell.left() + depth as f32 * 22.0;
    if let Some(open) = chevron {
        self::chevron(painter, pos2(left + 5.0, cell.center().y), open);
        left += 18.0;
    }
    let font = if bold { theme::semibold(13.0) } else { theme::regular(13.0) };
    let galley = widgets::elided(painter, text, font, theme::TEXT, cell.right() - left);
    painter.galley(pos2(left, cell.center().y - galley.size().y / 2.0), galley, theme::TEXT);
}

/// Whether the pointer is over the chevron at the start of an expandable row.
pub fn chevron_hit(ui: &Ui, rect: Rect, depth: u8) -> bool {
    ui.ctx().pointer_interact_pos().is_some_and(|p| p.x < rect.left() + PAD + depth as f32 * 22.0 + 22.0)
}

pub fn chevron(painter: &Painter, center: egui::Pos2, open: bool) {
    let angle = if open { std::f32::consts::FRAC_PI_2 } else { 0.0 };
    let rot = egui::emath::Rot2::from_angle(angle);
    let arm = |v: egui::Vec2| center + rot * v;
    painter.line(
        vec![arm(vec2(-2.5, -4.5)), arm(vec2(2.0, 0.0)), arm(vec2(-2.5, 4.5))],
        Stroke::new(1.5, theme::TEXT_DIM),
    );
}

pub fn search(ui: &mut Ui, query: &mut String, hint: &str, width: f32) -> Response {
    ui.add(
        egui::TextEdit::singleline(query).hint_text(hint).desired_width(width).font(theme::regular(13.0)).frame(
            Frame::new()
                .fill(theme::SURFACE)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(10, 6)),
        ),
    )
}

pub fn count(ui: &mut Ui, text: String) {
    ui.label(egui::RichText::new(text).font(theme::regular(12.5)).color(theme::TEXT_FAINT));
}

pub fn button(ui: &mut Ui, label: &str, enabled: bool, danger: bool) -> Response {
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

/// "Administrator" next to the toolbar buttons while hwpulse runs elevated.
pub fn admin_badge(ui: &mut Ui) {
    if tasks::is_elevated() {
        ui.label(egui::RichText::new("Administrator").font(theme::semibold(12.0)).color(theme::LIVE));
    }
}

/// The outcome of the last action.
#[derive(Clone, Debug)]
pub struct Notice {
    pub text: String,
    /// Failed for lack of rights: offer to restart as administrator.
    pub denied: bool,
    /// A file to show in Explorer (e.g. a dump file).
    pub reveal: Option<String>,
}

impl Notice {
    pub fn info(text: impl Into<String>) -> Self {
        Self { text: text.into(), denied: false, reveal: None }
    }

    pub fn denied(text: impl Into<String>) -> Self {
        Self { text: text.into(), denied: !tasks::is_elevated(), reveal: None }
    }
}

/// The banner for `notice`, with Dismiss and, when access was denied, a way to
/// restart as administrator on `page` (the `--page` argument).
pub fn notice(ui: &mut Ui, notice: &mut Option<Notice>, page: &str) {
    let Some(n) = notice.clone() else { return };
    Frame::new()
        .fill(if n.denied { theme::ERROR.gamma_multiply(0.12) } else { theme::SURFACE })
        .stroke(Stroke::new(1.0, if n.denied { theme::ERROR.gamma_multiply(0.5) } else { theme::BORDER }))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&n.text).font(theme::regular(12.5)).color(theme::TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if super::link(ui, "Dismiss", theme::regular(12.5), theme::TEXT_DIM, false).clicked() {
                        *notice = None;
                    }
                    if let Some(path) = &n.reveal
                        && super::link(ui, "Show in folder", theme::semibold(12.5), theme::ACCENT, false).clicked()
                    {
                        crate::shell::open_location(path);
                    }
                    if n.denied
                        && super::link(ui, "Restart as administrator", theme::semibold(12.5), theme::ACCENT, true)
                            .clicked()
                    {
                        if tasks::restart_as_admin(page) {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        } else {
                            *notice = Some(Notice::info("The administrator prompt was declined."));
                        }
                    }
                });
            });
        });
}

/// Work running on its own thread; the window repaints when it is done.
pub struct Job<T> {
    rx: Receiver<T>,
}

impl<T: Send + 'static> Job<T> {
    pub fn start(ctx: &egui::Context, work: impl FnOnce() -> T + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
        Self { rx }
    }

    /// The result once, when the work has finished.
    pub fn take(&self) -> Option<T> {
        self.rx.try_recv().ok()
    }
}

/// A confirmation dialog; `Some(true)` once confirmed, `Some(false)` once cancelled.
pub fn confirm(ui: &mut Ui, id: &str, title: &str, note: &str, warning: Option<&str>, action: &str) -> Option<bool> {
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::NULL.with(id)).show(ui.ctx(), |ui| {
        ui.set_width(380.0);
        ui.label(egui::RichText::new(title).font(theme::semibold(16.0)));
        ui.add_space(-4.0);
        ui.label(egui::RichText::new(note).font(theme::regular(12.5)).color(theme::TEXT_DIM));
        if let Some(warning) = warning {
            ui.label(egui::RichText::new(warning).font(theme::semibold(12.5)).color(theme::ERROR));
        }
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if button(ui, action, true, true).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
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
    decision
}
