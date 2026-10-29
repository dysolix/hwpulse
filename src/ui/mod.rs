pub mod theme;
pub mod widgets;
pub mod window;

use eframe::egui::{
    self, Align2, CentralPanel, CornerRadius, CursorIcon, Frame, Key, Margin, PointerButton, Rect, Response,
    ScrollArea, Sense, Ui, vec2,
};

use crate::poller::{self, Model, Shared};
use crate::sensors::Group;

#[derive(Clone, Debug, PartialEq)]
enum Page {
    Dashboard,
    Components,
    /// Detail view of one group, by [`Group::id`].
    Component(String),
}

pub struct App {
    model: Shared,
    page: Page,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        window::style_title_bar(cc);
        Self { model: poller::spawn(cc.egui_ctx.clone()), page: Page::Dashboard }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let model = self.model.lock().unwrap();
        let page = &self.page;
        // Navigation requested during this frame; applied after drawing.
        let mut nav: Option<Page> = None;

        let back = ui.input(|i| i.key_pressed(Key::Escape) || i.pointer.button_pressed(PointerButton::Extra1));
        if back && matches!(page, Page::Component(_)) {
            nav = Some(Page::Components);
        }

        CentralPanel::default()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(22)))
            .show(ui, |ui| {
                header(ui, &model, page, &mut nav);
                ScrollArea::vertical().id_salt(format!("{page:?}")).auto_shrink(false).show(ui, |ui| {
                    for error in &model.errors {
                        ui.label(egui::RichText::new(error).color(theme::ERROR).font(theme::regular(13.0)));
                    }
                    if model.groups.is_empty() && model.errors.is_empty() {
                        ui.label(egui::RichText::new("Waiting for sensors…").color(theme::TEXT_DIM));
                    }
                    match page {
                        Page::Dashboard => {
                            dashboard(ui, &model, &mut nav);
                            footer(ui);
                        }
                        Page::Components => components(ui, &model, &mut nav),
                        Page::Component(id) => detail(ui, &model, id, &mut nav),
                    }
                });
            });

        drop(model);
        if let Some(next) = nav {
            self.page = next;
            ui.ctx().request_repaint();
        }
    }
}

// ---------------------------------------------------------------------------
// Header

fn header(ui: &mut Ui, model: &Model, page: &Page, nav: &mut Option<Page>) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    let p = ui.painter().clone();
    p.text(rect.left_center(), Align2::LEFT_CENTER, "Hardware Monitor", theme::semibold(22.0), theme::TEXT);
    if let Some(machine) = &model.machine {
        p.text(
            rect.left_bottom() + vec2(0.0, 12.0),
            Align2::LEFT_CENTER,
            machine,
            theme::regular(12.5),
            theme::TEXT_FAINT,
        );
    }

    // "Live" pill on the right.
    let live = !model.groups.is_empty();
    let (dot, label) = if live { (theme::LIVE, "Live") } else { (theme::TEXT_FAINT, "Idle") };
    let galley = p.layout_no_wrap(label.to_owned(), theme::semibold(12.0), theme::TEXT_DIM);
    let pill_w = galley.size().x + 32.0;
    let pill = Rect::from_center_size(rect.right_center() - vec2(pill_w / 2.0, 0.0), vec2(pill_w, 24.0));
    p.rect_filled(pill, CornerRadius::same(12), theme::SURFACE);
    p.circle_filled(pill.left_center() + vec2(13.0, 0.0), 3.5, dot);
    p.circle_filled(pill.left_center() + vec2(13.0, 0.0), 6.5, dot.gamma_multiply(0.18));
    p.galley(pill.left_center() + vec2(22.0, -galley.size().y / 2.0), galley, theme::TEXT_DIM);

    // Segmented page switcher left of the pill.
    let tabs = [("Dashboard", Page::Dashboard), ("Components", Page::Components)];
    let font = theme::semibold(12.5);
    let widths: Vec<f32> = tabs.iter().map(|(t, _)| p.layout_no_wrap(t.to_string(), font.clone(), theme::TEXT).size().x + 28.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0;
    let bar = Rect::from_min_size(
        egui::pos2(pill.left() - 14.0 - total, rect.center().y - 15.0),
        vec2(total, 30.0),
    );
    p.rect_filled(bar, CornerRadius::same(15), theme::SURFACE);

    let mut x = bar.left() + 3.0;
    for ((title, target), w) in tabs.into_iter().zip(widths) {
        let tab = Rect::from_min_size(egui::pos2(x, bar.top() + 3.0), vec2(w, 24.0));
        x += w;
        let response = ui.interact(tab, ui.scope_id().with(title), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        let active = match (&target, page) {
            (Page::Components, Page::Component(_)) => true,
            (a, b) => a == b,
        };
        if active {
            p.rect_filled(tab, CornerRadius::same(12), theme::RAISED);
        }
        let color = if active || response.hovered() { theme::TEXT } else { theme::TEXT_DIM };
        p.text(tab.center(), Align2::CENTER_CENTER, title, font.clone(), color);
        if response.clicked() && page != &target {
            *nav = Some(target);
        }
    }

    ui.add_space(if model.machine.is_some() { 28.0 } else { 10.0 });
}

// ---------------------------------------------------------------------------
// Dashboard

const STATS_PER_ROW: usize = 3;
/// Vertical gap egui puts after every widget (`item_spacing.y`).
const GAP: f32 = 12.0;
const SECTION_HEADER: f32 = 42.0;
const SECTION_GAP: f32 = 14.0;
const MIN_COLUMN: f32 = 520.0;
const MAX_COLUMNS: usize = 4;
/// Upper bound for growing stat cards to even out columns; beyond it a gap remains.
const MAX_STRETCH: f32 = 60.0;

/// Masonry-style dashboard: as many columns as fit, components balanced across them,
/// then shorter columns stretch their stat cards so all columns end at the same height.
fn dashboard(ui: &mut Ui, model: &Model, nav: &mut Option<Page>) {
    let groups = &model.groups;
    if groups.is_empty() {
        return;
    }

    let fit = ((ui.available_width() + GAP) / (MIN_COLUMN + GAP)) as usize;
    let columns = fit.clamp(1, MAX_COLUMNS).min(groups.len());
    let heights: Vec<f32> = groups.iter().map(section_height).collect();
    let assignment = balance_columns(&heights, columns);
    let column_heights: Vec<f32> = assignment.iter().map(|col| col.iter().map(|&i| heights[i]).sum()).collect();
    let tallest = column_heights.iter().copied().fold(0.0, f32::max);

    ui.columns(columns, |cols| {
        for ((ui, members), height) in cols.iter_mut().zip(assignment).zip(column_heights) {
            let rows: usize = members.iter().map(|&i| stat_rows(&groups[i])).sum();
            let stretch = if rows > 0 { ((tallest - height) / rows as f32).min(MAX_STRETCH) } else { 0.0 };
            for i in members {
                group_section(ui, &groups[i], model, nav, stretch);
            }
        }
    });
}

/// Longest-processing-time scheduling: tallest sections first, each into the currently
/// shortest column (ties go left). Each column is then sorted back into display order,
/// so related components stay in sequence within a column.
fn balance_columns(heights: &[f32], k: usize) -> Vec<Vec<usize>> {
    let mut by_height: Vec<usize> = (0..heights.len()).collect();
    by_height.sort_by(|&a, &b| heights[b].total_cmp(&heights[a]).then(a.cmp(&b)));

    let mut columns = vec![Vec::new(); k];
    let mut totals = vec![0.0f32; k];
    for i in by_height {
        let shortest = (0..k).min_by(|&a, &b| totals[a].total_cmp(&totals[b])).unwrap_or(0);
        totals[shortest] += heights[i];
        columns[shortest].push(i);
    }
    for column in &mut columns {
        column.sort_unstable();
    }
    // Put the column holding the first component on the left.
    columns.sort_by_key(|c| c.first().copied().unwrap_or(usize::MAX));
    columns
}

fn stat_rows(group: &Group) -> usize {
    group.readings.iter().filter(|r| !r.featured).count().div_ceil(STATS_PER_ROW)
}

/// Exact height of [`group_section`] without stretch; every part has a fixed size.
fn section_height(group: &Group) -> f32 {
    let featured = group.readings.iter().any(|r| r.featured);
    SECTION_HEADER
        + GAP
        + if featured { widgets::FEATURED_HEIGHT + GAP } else { 0.0 }
        + stat_rows(group) as f32 * (widgets::STAT_HEIGHT + GAP)
        + SECTION_GAP
}

fn group_section(ui: &mut Ui, group: &Group, model: &Model, nav: &mut Option<Page>, stretch: f32) {
    // Fixed-height header so `section_height` stays exact.
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), SECTION_HEADER), Sense::hover());
    let p = ui.painter();
    let title = p.layout_no_wrap(format!("{}  ›", group.title), theme::semibold(16.0), theme::TEXT);
    let title_rect = Rect::from_min_size(rect.left_top() + vec2(0.0, 4.0), title.size());
    let response = ui
        .interact(title_rect, ui.scope_id().with(&group.id), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let color = if response.hovered() { theme::ACCENT } else { theme::TEXT };
    p.galley_with_override_text_color(title_rect.min, title, color);
    p.text(
        rect.left_bottom() - vec2(0.0, 6.0),
        Align2::LEFT_BOTTOM,
        &group.subtitle,
        theme::regular(12.0),
        theme::TEXT_FAINT,
    );
    if response.clicked() {
        *nav = Some(Page::Component(group.id.clone()));
    }

    cards(ui, group, model, stretch);
    ui.add_space(SECTION_GAP);
}

/// Gauges, then stat cards in rows. `stretch` adds height to every stat row.
fn cards(ui: &mut Ui, group: &Group, model: &Model, stretch: f32) {
    let history = |id: &str| model.history.get(&format!("{}/{}", group.id, id));

    let featured: Vec<_> = group.readings.iter().filter(|r| r.featured).collect();
    if !featured.is_empty() {
        ui.columns(featured.len(), |cols| {
            for (col, r) in cols.iter_mut().zip(&featured) {
                widgets::gauge_card(col, r, history(r.id));
            }
        });
    }

    let stats: Vec<_> = group.readings.iter().filter(|r| !r.featured).collect();
    for row in stats.chunks(STATS_PER_ROW) {
        ui.columns(STATS_PER_ROW, |cols| {
            for (col, r) in cols.iter_mut().zip(row) {
                widgets::stat_card(col, r, history(r.id), widgets::STAT_HEIGHT + stretch);
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Components list

fn components(ui: &mut Ui, model: &Model, nav: &mut Option<Page>) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Components").font(theme::semibold(18.0)).color(theme::TEXT));
    ui.add_space(-8.0);
    ui.label(
        egui::RichText::new(format!("{} detected · select one for details", model.groups.len()))
            .font(theme::regular(12.0))
            .color(theme::TEXT_FAINT),
    );
    ui.add_space(6.0);

    ui.spacing_mut().item_spacing.y = 10.0;
    for group in &model.groups {
        if widgets::component_row(ui, group).clicked() {
            *nav = Some(Page::Component(group.id.clone()));
        }
    }
}

// ---------------------------------------------------------------------------
// Component detail

fn detail(ui: &mut Ui, model: &Model, id: &str, nav: &mut Option<Page>) {
    if link(ui, "‹  All components", theme::regular(13.0), theme::TEXT_DIM, false).clicked() {
        *nav = Some(Page::Components);
    }
    ui.add_space(2.0);

    let Some(group) = model.groups.iter().find(|g| g.id == id) else {
        ui.label(egui::RichText::new("This component is not reporting right now.").color(theme::TEXT_DIM));
        return;
    };

    ui.label(
        egui::RichText::new(group.kind.label().to_uppercase())
            .font(theme::semibold(11.0))
            .color(theme::kind_color(group.kind)),
    );
    ui.add_space(-8.0);
    ui.label(egui::RichText::new(&group.title).font(theme::semibold(24.0)).color(theme::TEXT));
    ui.add_space(-8.0);
    ui.label(egui::RichText::new(&group.subtitle).font(theme::regular(13.0)).color(theme::TEXT_FAINT));
    ui.add_space(8.0);

    cards(ui, group, model, 0.0);

    if let Some(breakdown) = &group.breakdown {
        ui.add_space(14.0);
        ui.label(egui::RichText::new(breakdown.title).font(theme::semibold(16.0)).color(theme::TEXT));
        ui.add_space(-8.0);
        ui.label(egui::RichText::new(breakdown.subtitle).font(theme::regular(12.0)).color(theme::TEXT_FAINT));
        ui.add_space(4.0);

        let per_row = grid_columns(ui.available_width(), breakdown.items.len());
        for row in breakdown.items.chunks(per_row) {
            ui.columns(per_row, |cols| {
                for (col, item) in cols.iter_mut().zip(row) {
                    let history = model.history.get(&format!("{}/#{}", group.id, item.label));
                    widgets::breakdown_cell(col, item, breakdown.unit, breakdown.range, history);
                }
            });
        }
    }
    ui.add_space(14.0);
}

/// As many columns as fit (min. cell width), preferring a count that divides the
/// items evenly so rows end up full (16 threads → 8 × 2 or 4 × 4).
fn grid_columns(width: f32, items: usize) -> usize {
    const MIN_CELL: f32 = 128.0;
    let max = ((width / MIN_CELL) as usize).clamp(2, 8).min(items.max(1));
    (2..=max).rev().find(|c| items % c == 0).unwrap_or(max)
}

// ---------------------------------------------------------------------------
// Shared bits

/// Text that acts as a link: pointer cursor, brightens on hover, optional chevron.
fn link(ui: &mut Ui, text: &str, font: egui::FontId, color: egui::Color32, chevron: bool) -> Response {
    let label = if chevron { format!("{text}  ›") } else { text.to_owned() };
    let galley = ui.painter().layout_no_wrap(label, font, color);
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let tint = if response.hovered() { theme::ACCENT } else { color };
    ui.painter().galley_with_override_text_color(rect.min, galley, tint);
    response
}

fn footer(ui: &mut Ui) {
    ui.add_space(2.0);
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "Polling every {} s · {} min history",
                poller::INTERVAL.as_secs(),
                poller::HISTORY_LEN as u64 * poller::INTERVAL.as_secs() / 60
            ))
            .font(theme::regular(11.5))
            .color(theme::TEXT_FAINT),
        );
    });
}
