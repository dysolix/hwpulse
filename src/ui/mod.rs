mod compact;
mod components;
mod processes;
mod services;
mod startup;
mod table;
pub mod theme;
mod users;
pub mod widgets;
pub mod window;

use eframe::egui::{
    self, Align2, CentralPanel, CornerRadius, CursorIcon, Frame, Margin, Rect, Response, ScrollArea, Sense, Ui, vec2,
};

use crate::poller::{self, Model, Shared};
use crate::sensors::Group;
use crate::settings::{self, Settings};
use crate::{autostart, tasks};

#[derive(Clone, Debug, PartialEq)]
enum Page {
    Dashboard,
    Components,
    /// Only a navigation request: the Components page with this [`Group::id`] opened.
    Component(String),
    Processes,
    /// Only a navigation request: the Processes page with this process selected.
    Process(u32),
    Services,
    /// Only a navigation request: the Services page showing the services in this process.
    ServicesOf(u32),
    Startup,
    Users,
    Settings,
}

pub struct App {
    model: Shared,
    page: Page,
    settings: Settings,
    components: components::State,
    processes: processes::State,
    services: services::State,
    startup: startup::State,
    users: users::State,
    /// Window options as last applied, so a changed setting takes effect once.
    applied: Option<(bool, bool, settings::Speed)>,
    /// The window size to return to when leaving the compact view.
    full_size: Option<egui::Vec2>,
    /// Whether this copy starts with Windows; checked (and repaired) at startup.
    autostart: autostart::State,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, model: Shared) -> Self {
        model.attach(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);
        window::style_title_bar(cc);
        let settings = cc.storage.and_then(|s| eframe::get_value(s, settings::STORAGE_KEY)).unwrap_or_default();
        tasks::enable_debug_privilege();
        // `--page processes` after "Restart as administrator".
        let args: Vec<String> = std::env::args().collect();
        let page = match args.windows(2).find(|a| a[0] == "--page").map(|a| a[1].as_str()) {
            Some("processes") => Page::Processes,
            Some("services") => Page::Services,
            Some("startup") => Page::Startup,
            Some("users") => Page::Users,
            Some("components") => Page::Components,
            Some("settings") => Page::Settings,
            _ => Page::Dashboard,
        };
        Self {
            model,
            page,
            settings,
            components: Default::default(),
            processes: Default::default(),
            services: Default::default(),
            startup: Default::default(),
            users: Default::default(),
            applied: None,
            full_size: None,
            autostart: autostart::check(),
        }
    }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, settings::STORAGE_KEY, &self.settings);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.apply_window_options(ui.ctx());
        let mut model = self.model.lock();
        // The process list is built only while a page shows it.
        model.want_processes = !self.settings.compact && matches!(self.page, Page::Processes | Page::Users);
        if !model.want_processes {
            model.processes = None;
        } else if model.processes.is_none() {
            self.model.wake();
        }
        let model = model;
        let focused = ui.ctx().input(|i| i.viewport().focused.unwrap_or(true));
        let slide = focused || self.settings.slide_in_background;
        widgets::set_timeline(ui.ctx(), model.updated.filter(|_| slide).map(|t| t.elapsed()));
        let page = &self.page;
        let settings = &mut self.settings;
        let components = &mut self.components;
        let processes = &mut self.processes;
        let autostart = &mut self.autostart;
        // Navigation requested during this frame; applied after drawing.
        let mut nav: Option<Page> = None;

        if settings.compact {
            CentralPanel::default().frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(14))).show(ui, |ui| {
                compact::page(ui, &model, settings);
            });
            return;
        }

        CentralPanel::default().frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(22))).show(ui, |ui| {
            header(ui, &model, page, &mut nav);
            // These pages work without sensor data; the others wait for the first samples.
            let independent = matches!(page, Page::Settings | Page::Services | Page::Startup | Page::Users);
            if model.groups.is_empty() && model.errors.is_empty() && !independent {
                widgets::waiting(ui, widgets::SENSORS.0, widgets::SENSORS.1);
                return;
            }
            match page {
                // The list and the details scroll on their own instead of one page scroll.
                Page::Components => return components::page(ui, &model, settings, components),
                Page::Processes => return processes::page(ui, &model, settings, processes, &mut nav),
                Page::Services => return services::page(ui, &mut self.services, &mut nav),
                Page::Startup => return startup::page(ui, &mut self.startup),
                Page::Users => return users::page(ui, &model, &mut self.users, &mut nav),
                _ => {}
            }
            let viewport_height = ui.available_height();
            ScrollArea::vertical().id_salt(format!("{page:?}")).auto_shrink(false).show(ui, |ui| {
                let content_top = ui.cursor().top();
                for error in &model.errors {
                    ui.label(egui::RichText::new(error).color(theme::ERROR).font(theme::regular(13.0)));
                }
                match page {
                    Page::Dashboard => {
                        dashboard(ui, &model, settings, &mut nav);
                        footer(ui, settings, viewport_height - (ui.cursor().top() - content_top));
                    }
                    Page::Settings => settings_page(ui, &model, settings, autostart),
                    _ => {}
                }
            });
        });

        drop(model);
        if let Some(next) = nav {
            self.page = match next {
                Page::Component(id) => {
                    self.components.open(id);
                    Page::Components
                }
                Page::Process(pid) => {
                    self.processes.focus(pid);
                    Page::Processes
                }
                Page::ServicesOf(pid) => {
                    self.services.show_pid(pid);
                    Page::Services
                }
                page => page,
            };
            ui.ctx().request_repaint();
        }
    }
}

impl App {
    /// Sends always-on-top, the compact window size and the update speed to the
    /// window and poller whenever they differ from what was last applied.
    fn apply_window_options(&mut self, ctx: &egui::Context) {
        let s = &self.settings;
        let wanted = (s.always_on_top, s.compact, s.speed);
        let previous = self.applied.replace(wanted);
        if previous == Some(wanted) {
            return;
        }
        let first = previous.is_none();
        if first || previous.is_some_and(|p| p.0 != s.always_on_top) {
            let level = if s.always_on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
        }
        if first || previous.is_some_and(|p| p.2 != s.speed) {
            self.model.set_interval(s.speed.interval());
        }
        if previous.is_some_and(|p| p.1 != s.compact) || first && s.compact {
            if s.compact {
                if !first {
                    self.full_size = ctx.input(|i| i.viewport().inner_rect).map(|r| r.size());
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(compact::MIN_SIZE));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(compact::SIZE));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(vec2(520.0, 420.0)));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(self.full_size.unwrap_or(vec2(1180.0, 940.0))));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Header

fn header(ui: &mut Ui, model: &Model, page: &Page, nav: &mut Option<Page>) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    let p = ui.painter().clone();
    let title = p.layout_no_wrap("Hardware Monitor".to_owned(), theme::semibold(22.0), theme::TEXT);
    let title_width = title.size().x;
    p.galley(rect.left_center() - vec2(0.0, title.size().y / 2.0), title, theme::TEXT);
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
    let (dot, label) = match (live, poller::paused()) {
        (true, true) => (theme::ACCENT, "Paused"),
        (true, false) => (theme::LIVE, "Live"),
        (false, _) => (theme::TEXT_FAINT, "Idle"),
    };
    let galley = p.layout_no_wrap(label.to_owned(), theme::semibold(12.0), theme::TEXT_DIM);
    let pill_w = galley.size().x + 32.0;
    let pill = Rect::from_center_size(rect.right_center() - vec2(pill_w / 2.0, 0.0), vec2(pill_w, 24.0));
    p.rect_filled(pill, CornerRadius::same(12), theme::SURFACE);
    p.circle_filled(pill.left_center() + vec2(13.0, 0.0), 3.5, dot);
    p.circle_filled(pill.left_center() + vec2(13.0, 0.0), 6.5, dot.gamma_multiply(0.18));
    p.galley(pill.left_center() + vec2(22.0, -galley.size().y / 2.0), galley, theme::TEXT_DIM);

    // Segmented page switcher: left of the pill when everything fits in one row,
    // otherwise on its own row below the title so nothing overlaps.
    let tabs = [
        ("Dashboard", Page::Dashboard),
        ("Components", Page::Components),
        ("Processes", Page::Processes),
        ("Services", Page::Services),
        ("Startup apps", Page::Startup),
        ("Users", Page::Users),
        ("Settings", Page::Settings),
    ];
    let font = theme::semibold(12.5);
    let widths: Vec<f32> =
        tabs.iter().map(|(t, _)| p.layout_no_wrap(t.to_string(), font.clone(), theme::TEXT).size().x + 28.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0;
    let inline = title_width + 32.0 + total + 14.0 <= pill.left() - rect.left();
    let machine_space = if model.machine.is_some() { 28.0 } else { 10.0 };

    let bar = if inline {
        Rect::from_min_size(egui::pos2(pill.left() - 14.0 - total, rect.center().y - 15.0), vec2(total, 30.0))
    } else {
        ui.add_space(machine_space - 2.0);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
        Rect::from_min_size(row.min, vec2(total, 30.0))
    };
    p.rect_filled(bar, CornerRadius::same(15), theme::SURFACE);

    let mut x = bar.left() + 3.0;
    for ((title, target), w) in tabs.into_iter().zip(widths) {
        let tab = Rect::from_min_size(egui::pos2(x, bar.top() + 3.0), vec2(w, 24.0));
        x += w;
        let response =
            ui.interact(tab, ui.scope_id().with(title), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        let active = &target == page;
        if active {
            p.rect_filled(tab, CornerRadius::same(12), theme::RAISED);
        }
        let color = if active || response.hovered() { theme::TEXT } else { theme::TEXT_DIM };
        p.text(tab.center(), Align2::CENTER_CENTER, title, font.clone(), color);
        if response.clicked() && page != &target {
            *nav = Some(target);
        }
    }

    ui.add_space(if inline { machine_space } else { 6.0 });
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

/// An ongoing drag of a dashboard section.
#[derive(Clone)]
struct SectionDrag {
    id: String,
    /// Where it would be inserted among the other visible sections.
    insert: usize,
    /// Pointer position relative to the section's top-left corner.
    grab: egui::Vec2,
}

/// Masonry-style dashboard: as many columns as fit, components balanced across them,
/// then shorter columns stretch their stat cards so all columns end at the same height.
///
/// Sections can be reordered by dragging their header. While dragging, the dashboard
/// shows the layout as it would be after the drop: for every possible insertion point
/// the resulting layout is computed, and the one whose free slot is closest to the
/// floating section wins. That choice depends only on where the floating section is,
/// not on the layout currently shown, so the preview cannot flip back and forth.
fn dashboard(ui: &mut Ui, model: &Model, settings: &mut Settings, nav: &mut Option<Page>) {
    const SLIDE: f32 = 0.22;
    /// A different slot must be this much closer before the preview switches to it.
    const HYSTERESIS: f32 = 24.0;

    let ctx = ui.ctx().clone();
    let base = ui.scope_id().with("dashboard");
    let drag_id = |g: &Group| base.with(("drag", &g.id));
    let state_id = base.with("drag_state");
    let animate_until_id = base.with("animate_until");
    let now = ctx.input(|i| i.time);
    let released = ctx.input(|i| i.pointer.any_released());

    // Apply a finished drag before laying out, so everything slides from there.
    let state: Option<SectionDrag> = ctx.data(|d| d.get_temp(state_id));
    if released && let Some(drag) = &state {
        let visible = settings.dashboard_groups(&model.groups);
        let others: Vec<&Group> = visible.iter().filter(|g| g.id != drag.id).collect();
        let full = settings.ordered(&model.groups);
        let from = full.iter().position(|g| g.id == drag.id);
        // Insert before the section that follows in the preview, or after the last one.
        let to = match others.get(drag.insert) {
            Some(next) => full.iter().position(|g| g.id == next.id),
            None => others.last().and_then(|last| full.iter().position(|g| g.id == last.id)).map(|i| i + 1),
        };
        if let (Some(from), Some(to)) = (from, to)
            && to != from
            && to != from + 1
        {
            settings.reorder(&full, from, to);
        }
        ctx.data_mut(|d| {
            d.remove::<SectionDrag>(state_id);
            d.insert_temp(animate_until_id, now + SLIDE as f64 + 0.05);
        });
    }

    let groups = settings.dashboard_groups(&model.groups);
    if groups.is_empty() {
        if !model.groups.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Everything is hidden").font(theme::semibold(16.0)).color(theme::TEXT));
                if link(ui, "Choose what to show  ›", theme::regular(13.0), theme::TEXT_DIM, false).clicked() {
                    *nav = Some(Page::Settings);
                }
            });
        }
        return;
    }

    let width = ui.available_width();
    let origin = ui.cursor().min;
    let pointer = ctx.pointer_latest_pos();
    let all: Vec<&Group> = groups.iter().collect();
    let dragged = if released { None } else { all.iter().position(|g| ctx.dragged_id() == Some(drag_id(g))) };

    // The order to show: as is, or the drop preview while dragging.
    let mut order = all.clone();
    let mut layout = masonry(&order, width, origin);
    let mut lifted: Option<(&Group, egui::Pos2)> = None;
    if let (Some(d), Some(p)) = (dragged, pointer) {
        let group = all[d];
        let grab = state.as_ref().filter(|s| s.id == group.id).map(|s| s.grab).unwrap_or(p - layout.slots[d].0.min);
        let float_pos = p - grab;
        let others: Vec<&Group> = all.iter().copied().filter(|g| g.id != group.id).collect();
        let current = state.as_ref().filter(|s| s.id == group.id).map_or(d, |s| s.insert.min(others.len()));

        let candidate = |k: usize| {
            let mut order = others.clone();
            order.insert(k, group);
            let layout = masonry(&order, width, origin);
            let distance = (layout.slots[k].0.min - float_pos).length();
            (order, layout, distance)
        };
        let (mut best_k, mut best) = (current, candidate(current));
        for k in (0..=others.len()).filter(|&k| k != current) {
            let c = candidate(k);
            if c.2 < best.2 - HYSTERESIS {
                (best_k, best) = (k, c);
            }
        }
        (order, layout) = (best.0, best.1);
        ctx.data_mut(|data| data.insert_temp(state_id, SectionDrag { id: group.id.clone(), insert: best_k, grab }));
        lifted = Some((group, float_pos));
    } else if !released && state.is_some() {
        ctx.data_mut(|d| d.remove::<SectionDrag>(state_id));
    }

    let animating = lifted.is_some() || ctx.data(|d| d.get_temp::<f64>(animate_until_id)).is_some_and(|t| now < t);
    let slide = if animating { SLIDE } else { 0.0 };

    for (i, group) in order.iter().enumerate() {
        let (rect, stretch) = layout.slots[i];
        if lifted.is_some_and(|(g, _)| g.id == group.id) {
            // The free slot where the dragged section will land.
            let slot = rect.with_max_y(rect.bottom() - SECTION_GAP);
            ui.painter().rect(
                slot,
                CornerRadius::same(theme::CARD_RADIUS),
                theme::ACCENT.gamma_multiply(0.07),
                egui::Stroke::new(1.5, theme::ACCENT.gamma_multiply(0.6)),
                egui::StrokeKind::Inside,
            );
            continue;
        }
        let x = ctx.animate_value_with_time(base.with(("x", &group.id)), rect.left() - origin.x, slide);
        let y = ctx.animate_value_with_time(base.with(("y", &group.id)), rect.top() - origin.y, slide);
        let shown = Rect::from_min_size(origin + vec2(x, y), rect.size());
        ui.scope_builder(egui::UiBuilder::new().max_rect(shown), |ui| {
            group_section(ui, group, model, nav, stretch, drag_id(group));
        });
    }
    ui.allocate_rect(Rect::from_min_max(origin, egui::pos2(origin.x + width, layout.bottom)), Sense::hover());

    let Some((group, float_pos)) = lifted else { return };
    let i = order.iter().position(|g| g.id == group.id).unwrap_or(0);
    let (rect, stretch) = layout.slots[i];

    // The section floats under the pointer; its slide position follows, so on
    // release it glides from there into the slot.
    const LIFT_PAD: f32 = 12.0;
    ctx.animate_value_with_time(base.with(("x", &group.id)), float_pos.x - origin.x, 0.0);
    ctx.animate_value_with_time(base.with(("y", &group.id)), float_pos.y - origin.y, 0.0);
    egui::Area::new(base.with("floating"))
        .order(egui::Order::Tooltip)
        .fixed_pos(float_pos - vec2(LIFT_PAD, LIFT_PAD))
        .show(&ctx, |ui| {
            ui.set_opacity(0.94);
            Frame::new()
                .fill(theme::BG)
                .stroke(egui::Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.6)))
                .corner_radius(CornerRadius::same(theme::CARD_RADIUS + 4))
                .inner_margin(Margin::same(LIFT_PAD as i8))
                .shadow(egui::Shadow {
                    offset: [0, 16],
                    blur: 40,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(170),
                })
                .show(ui, |ui| {
                    ui.set_width(rect.width());
                    let mut no_nav = None;
                    group_section(ui, group, model, &mut no_nav, stretch, drag_id(group));
                });
        });

    // Scroll when dragging near the top or bottom edge of the view.
    if let Some(p) = pointer {
        let view = ui.clip_rect();
        let delta = if p.y < view.top() + 48.0 {
            12.0
        } else if p.y > view.bottom() - 48.0 {
            -12.0
        } else {
            0.0
        };
        if delta != 0.0 {
            ui.scroll_with_delta(vec2(0.0, delta));
            ctx.request_repaint();
        }
    }
}

struct Layout {
    /// Per section (same order as the input): its rect and stat card stretch.
    slots: Vec<(Rect, f32)>,
    bottom: f32,
}

/// Positions `groups` (in display order) in masonry columns starting at `origin`.
fn masonry(groups: &[&Group], width: f32, origin: egui::Pos2) -> Layout {
    let fit = ((width + GAP) / (MIN_COLUMN + GAP)) as usize;
    let columns = fit.clamp(1, MAX_COLUMNS).min(groups.len().max(1));
    let column_width = (width - GAP * (columns - 1) as f32) / columns as f32;
    let heights: Vec<f32> = groups.iter().map(|g| section_height(g)).collect();
    let assignment = balance_columns(&heights, columns);
    let column_heights: Vec<f32> = assignment.iter().map(|col| col.iter().map(|&i| heights[i]).sum()).collect();
    let tallest = column_heights.iter().copied().fold(0.0, f32::max);

    let mut slots = vec![(Rect::NOTHING, 0.0f32); groups.len()];
    for (c, (members, height)) in assignment.iter().zip(&column_heights).enumerate() {
        let rows: usize = members.iter().map(|&i| stat_rows(groups[i])).sum();
        let stretch = if rows > 0 { ((tallest - height) / rows as f32).min(MAX_STRETCH) } else { 0.0 };
        let x = origin.x + c as f32 * (column_width + GAP);
        let mut y = origin.y;
        for &i in members {
            let h = heights[i] + stretch * stat_rows(groups[i]) as f32;
            slots[i] = (Rect::from_min_size(egui::pos2(x, y), vec2(column_width, h)), stretch);
            y += h;
        }
    }
    let bottom = slots.iter().map(|(r, _)| r.bottom()).fold(origin.y, f32::max);
    Layout { slots, bottom }
}

/// Classic masonry: sections in the user's order, each into the currently shortest
/// column (ties go left). Reading left-to-right, top-to-bottom follows that order.
fn balance_columns(heights: &[f32], k: usize) -> Vec<Vec<usize>> {
    let mut columns = vec![Vec::new(); k];
    let mut totals = vec![0.0f32; k];
    for (i, h) in heights.iter().enumerate() {
        let shortest = (0..k).min_by(|&a, &b| totals[a].total_cmp(&totals[b])).unwrap_or(0);
        totals[shortest] += h;
        columns[shortest].push(i);
    }
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

/// One dashboard section. The header doubles as drag handle (`drag_id`) for
/// reordering; the title itself opens the component on the Components page.
fn group_section(ui: &mut Ui, group: &Group, model: &Model, nav: &mut Option<Page>, stretch: f32, drag_id: egui::Id) {
    // Fixed-height header so `section_height` stays exact.
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), SECTION_HEADER), Sense::hover());
    let handle = ui.interact(rect, drag_id, Sense::drag());
    let handle = if handle.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        handle
    } else {
        handle.on_hover_cursor(CursorIcon::Grab)
    };
    let p = ui.painter();
    if handle.hovered() || handle.dragged() {
        // Grip in the top-right corner hints that the header can be dragged.
        let grip = egui::pos2(rect.right() - 8.0, rect.top() + 14.0);
        for row in -1..=1 {
            for col in [-1.0f32, 1.0] {
                p.circle_filled(grip + vec2(col * 3.5, row as f32 * 6.0), 1.8, theme::TEXT_DIM);
            }
        }
    }
    let title = p.layout_no_wrap(format!("{}  ›", group.title), theme::semibold(16.0), theme::TEXT);
    let title_rect = Rect::from_min_size(rect.left_top() + vec2(0.0, 4.0), title.size());
    let response =
        ui.interact(title_rect, drag_id.with("title"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
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
                col.push_id((&group.id, r.id), |ui| widgets::gauge_card(ui, r, history(r.id)));
            }
        });
    }

    let stats: Vec<_> = group.readings.iter().filter(|r| !r.featured).collect();
    for row in stats.chunks(STATS_PER_ROW) {
        ui.columns(STATS_PER_ROW, |cols| {
            for (col, r) in cols.iter_mut().zip(row) {
                col.push_id((&group.id, r.id), |ui| {
                    widgets::stat_card(ui, r, history(r.id), widgets::STAT_HEIGHT + stretch)
                });
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Settings

/// One section per topic; add new sections below the dashboard one.
fn settings_page(ui: &mut Ui, model: &Model, settings: &mut Settings, autostart: &mut autostart::State) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Settings").font(theme::semibold(18.0)).color(theme::TEXT));
    ui.add_space(-8.0);
    ui.label(egui::RichText::new("Saved automatically").font(theme::regular(12.0)).color(theme::TEXT_FAINT));
    ui.add_space(10.0);

    dashboard_settings(ui, model, settings);
    speed_settings(ui, settings);
    chart_settings(ui, settings);
    window_settings(ui, settings);
    startup_settings(ui, autostart);
}

fn speed_settings(ui: &mut Ui, settings: &mut Settings) {
    section_heading(
        ui,
        "Update speed",
        "How often sensors and processes are read. Faster costs more CPU; paused keeps the last values.",
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for speed in settings::Speed::ALL {
            if widgets::chip(ui, speed.label(), settings.speed == speed, true, theme::ACCENT).clicked() {
                settings.speed = speed;
            }
        }
    });
    ui.add_space(14.0);
}

fn window_settings(ui: &mut Ui, settings: &mut Settings) {
    section_heading(ui, "Window", "How the hwmon window behaves.");
    ui.add_space(6.0);
    if switch_card(ui, "Always on top", "Stay above other windows, also in the compact view.", settings.always_on_top) {
        settings.always_on_top = !settings.always_on_top;
    }
    if switch_card(
        ui,
        "Minimize on use",
        "Minimize hwmon after \u{201c}Switch to\u{201d} brings an app to the front.",
        settings.minimize_on_use,
    ) {
        settings.minimize_on_use = !settings.minimize_on_use;
    }
    if switch_card(
        ui,
        "Compact view",
        "A small window with just the dashboard's values, like Task Manager's \u{201c}Fewer details\u{201d}.",
        settings.compact,
    ) {
        settings.compact = !settings.compact;
    }
    ui.add_space(14.0);
}

fn chart_settings(ui: &mut Ui, settings: &mut Settings) {
    section_heading(ui, "Charts", "New samples slide in from the right while hwmon is the active window.");
    ui.add_space(6.0);
    if switch_card(
        ui,
        "Animate in the background",
        "Keep sliding when another window is active. Uses a bit more CPU.",
        settings.slide_in_background,
    ) {
        settings.slide_in_background = !settings.slide_in_background;
    }
    ui.add_space(14.0);
}

/// `autostart` mirrors the registry and is re-read after every change.
fn startup_settings(ui: &mut Ui, autostart: &mut autostart::State) {
    section_heading(ui, "Startup", "Stored in Windows, so it matches the choice made in the installer.");
    ui.add_space(6.0);
    let description = match autostart {
        autostart::State::OtherCopy(path) => {
            format!("Another copy starts with Windows ({}). Switch on to start this one instead.", path.display())
        }
        _ => "Open hwmon when you sign in.".to_owned(),
    };
    let on = *autostart == autostart::State::On;
    if switch_card(ui, "Start with Windows", &description, on) {
        let _ = autostart::set(!on);
        *autostart = autostart::check();
    }
    ui.add_space(14.0);
}

/// Card with a title, description and an on/off switch; true when the switch was clicked.
fn switch_card(ui: &mut Ui, title: &str, description: &str, on: bool) -> bool {
    let mut clicked = false;
    Frame::new()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(theme::CARD_RADIUS))
        .inner_margin(Margin::symmetric(20, 16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(egui::RichText::new(title).font(theme::semibold(15.0)).color(theme::TEXT));
                    ui.label(egui::RichText::new(description).font(theme::regular(12.0)).color(theme::TEXT_DIM));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    clicked = widgets::toggle(ui, on).clicked();
                });
            });
        });
    clicked
}

fn section_heading(ui: &mut Ui, title: &str, description: &str) {
    ui.label(egui::RichText::new(title).font(theme::semibold(16.0)).color(theme::TEXT));
    ui.add_space(-8.0);
    ui.label(egui::RichText::new(description).font(theme::regular(12.0)).color(theme::TEXT_FAINT));
}

/// Section heading that collapses its body when clicked. The open state is kept
/// across restarts with the rest of egui's memory.
fn collapsible_section(ui: &mut Ui, title: &str, note: &str, description: &str, body: impl FnOnce(&mut Ui)) {
    let id = ui.scope_id().with(("section", title));
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let openness = state.openness(ui.ctx());

    let response = ui
        .horizontal(|ui| {
            let title = ui.painter().layout_no_wrap(title.to_owned(), theme::semibold(16.0), theme::TEXT);
            let (rect, _) = ui.allocate_exact_size(vec2(title.size().x + 26.0, title.size().y), Sense::hover());
            let hovered = ui.rect_contains_pointer(ui.max_rect().with_max_y(rect.bottom()));
            ui.painter().galley(rect.min, title, theme::TEXT);
            // Chevron pointing right when collapsed, down when open.
            let c = egui::pos2(rect.right() - 8.0, rect.center().y);
            let angle = openness * std::f32::consts::FRAC_PI_2;
            let rot = egui::emath::Rot2::from_angle(angle);
            let stroke = egui::Stroke::new(1.6, if hovered { theme::TEXT } else { theme::TEXT_FAINT });
            let arm = |v: egui::Vec2| c + rot * v;
            ui.painter().line(vec![arm(vec2(-2.5, -5.0)), arm(vec2(2.5, 0.0)), arm(vec2(-2.5, 5.0))], stroke);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(note).font(theme::regular(12.0)).color(theme::TEXT_FAINT));
            });
        })
        .response
        .interact(Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    if response.clicked() {
        state.toggle(ui);
    }
    ui.add_space(-8.0);
    ui.label(egui::RichText::new(description).font(theme::regular(12.0)).color(theme::TEXT_FAINT));
    state.show_body_unindented(ui, body);
}

fn dashboard_settings(ui: &mut Ui, model: &Model, settings: &mut Settings) {
    let shown = model.groups.iter().filter(|g| settings.group_visible(g)).count();
    collapsible_section(
        ui,
        "Dashboard",
        &format!("{shown} of {} shown", model.groups.len()),
        "Drag the handle to reorder, switch components on or off and pick which values get a card. \
         GPUs, CPU and memory are shown by default; drives and network adapters start hidden. \
         Values for a closer look (latencies, pools, PCIe traffic …) start off and stay on the component page.",
        |ui| dashboard_list(ui, model, settings),
    );
    ui.add_space(14.0);
}

fn dashboard_list(ui: &mut Ui, model: &Model, settings: &mut Settings) {
    let resets = [(!settings.visibility_is_default(), "Reset to default"), (!settings.order.is_empty(), "Reset order")];
    if resets.iter().any(|(show, _)| *show) {
        ui.add_space(-4.0);
        ui.horizontal(|ui| {
            for (show, label) in resets {
                if show && link(ui, label, theme::regular(12.5), theme::ACCENT, false).clicked() {
                    match label {
                        "Reset order" => settings.order.clear(),
                        _ => settings.reset_visibility(),
                    }
                }
            }
        });
    }
    ui.add_space(6.0);
    ui.spacing_mut().item_spacing.y = 10.0;

    reorder_list(ui, model, settings);
}

/// Drag-and-drop list of component cards. The dragged card floats above everything
/// and follows the pointer; the others slide aside to open a gap where it will land.
/// Cards are positioned manually (heights remembered from the previous frame) so their
/// movement can be animated.
fn reorder_list(ui: &mut Ui, model: &Model, settings: &mut Settings) {
    const GAP: f32 = 10.0;
    const SLIDE: f32 = 0.18;

    let ctx = ui.ctx().clone();
    let base = ui.scope_id().with("reorder");
    let handle_id = |g: &Group| base.with(("handle", &g.id));
    let anim_id = |g: &Group| base.with(("y", &g.id));
    let heights_id = base.with("heights");
    // While dragging: (dragged group id, drop slot among the other cards, grab offset).
    let state_id = base.with("state");

    // Apply a drop from the previous frames before laying out, so the released card
    // already slides from where it was let go into its new place.
    let released = ctx.input(|i| i.pointer.any_released());
    let state: Option<(String, usize, f32)> = ctx.data(|d| d.get_temp(state_id));
    if released && let Some((id, slot, _)) = &state {
        let ordered = settings.ordered(&model.groups);
        if let Some(from) = ordered.iter().position(|g| &g.id == id) {
            let to = if *slot >= from { slot + 1 } else { *slot };
            if to != from && to != from + 1 {
                settings.reorder(&ordered, from, to);
            }
        }
        ctx.data_mut(|d| d.remove::<(String, usize, f32)>(state_id));
    }

    let ordered = settings.ordered(&model.groups);
    let mut heights: std::collections::HashMap<String, f32> = ctx.data(|d| d.get_temp(heights_id)).unwrap_or_default();
    // Where each resting card was drawn, relative to the list top.
    let positions_id = base.with("positions");
    let mut positions: std::collections::HashMap<String, f32> =
        ctx.data(|d| d.get_temp(positions_id)).unwrap_or_default();
    let height = |g: &Group, h: &std::collections::HashMap<String, f32>| h.get(&g.id).copied().unwrap_or(110.0);

    let (left, top) = (ui.cursor().left(), ui.cursor().top());
    let width = ui.available_width();
    let pointer = ctx.pointer_latest_pos();
    let dragged = if released { None } else { ordered.iter().position(|g| ctx.dragged_id() == Some(handle_id(g))) };

    // Where the floating card is (relative to the list top) and which slot it is over.
    let mut lifted: Option<(usize, f32, usize)> = None;
    if let (Some(d), Some(p)) = (dragged, pointer) {
        let grab = state.as_ref().filter(|s| s.0 == ordered[d].id).map(|s| s.2).unwrap_or_else(|| {
            // First frame of the drag: grab offset from where the card was drawn last.
            p.y - (top + positions.get(&ordered[d].id).copied().unwrap_or(0.0))
        });
        let float_y = p.y - grab - top;
        let center = float_y + height(ordered[d], &heights) / 2.0;
        let mut y = 0.0;
        let mut slot = 0;
        for g in ordered.iter().enumerate().filter(|(i, _)| *i != d).map(|(_, g)| g) {
            if y + height(g, &heights) / 2.0 < center {
                slot += 1;
            }
            y += height(g, &heights) + GAP;
        }
        ctx.data_mut(|d_| d_.insert_temp(state_id, (ordered[d].id.clone(), slot, grab)));
        lifted = Some((d, float_y, slot));
    } else if !released && state.is_some() {
        ctx.data_mut(|d| d.remove::<(String, usize, f32)>(state_id));
    }

    // Lay out the resting cards, leaving a gap at the drop slot.
    let mut y = 0.0;
    let mut slot_index = 0;
    let mut gap_at = None;
    for (i, group) in ordered.iter().enumerate() {
        if lifted.is_some_and(|(d, _, _)| d == i) {
            continue;
        }
        if let Some((d, _, slot)) = lifted
            && slot == slot_index
        {
            gap_at = Some(y);
            y += height(ordered[d], &heights) + GAP;
        }
        let shown = ctx.animate_value_with_time(anim_id(group), y, SLIDE);
        positions.insert(group.id.clone(), shown);
        let rect = Rect::from_min_size(egui::pos2(left, top + shown), vec2(width, height(group, &heights)));
        let drawn = ui
            .scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                settings_card(ui, group, settings, handle_id(group), false)
            })
            .inner;
        heights.insert(group.id.clone(), drawn.height());
        y += height(group, &heights) + GAP;
        slot_index += 1;
    }
    if let Some((d, _, slot)) = lifted
        && slot == slot_index
    {
        gap_at = Some(y);
        y += height(ordered[d], &heights) + GAP;
    }

    // Reserve the list's space in the parent layout.
    ui.allocate_rect(Rect::from_min_size(egui::pos2(left, top), vec2(width, (y - GAP).max(0.0))), Sense::hover());

    if let Some((d, float_y, _)) = lifted {
        let group = ordered[d];
        if let Some(gap_y) = gap_at {
            let gap = Rect::from_min_size(egui::pos2(left, top + gap_y), vec2(width, height(group, &heights)));
            ui.painter().rect(
                gap,
                CornerRadius::same(theme::CARD_RADIUS),
                theme::ACCENT.gamma_multiply(0.05),
                egui::Stroke::new(1.5, theme::ACCENT.gamma_multiply(0.45)),
                egui::StrokeKind::Inside,
            );
        }
        // Keep the card's slide animation at the floating position, so on release it
        // glides from there into the gap.
        ctx.animate_value_with_time(anim_id(group), float_y, 0.0);
        egui::Area::new(base.with("floating"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(left, top + float_y))
            .show(&ctx, |ui| {
                ui.set_width(width);
                let drawn = settings_card(ui, group, settings, handle_id(group), true);
                heights.insert(group.id.clone(), drawn.height());
            });

        // Scroll when dragging near the top or bottom edge of the view.
        if let Some(p) = pointer {
            let view = ui.clip_rect();
            let edge = 48.0;
            let delta = if p.y < view.top() + edge {
                12.0
            } else if p.y > view.bottom() - edge {
                -12.0
            } else {
                0.0
            };
            if delta != 0.0 {
                ui.scroll_with_delta(vec2(0.0, delta));
                ctx.request_repaint();
            }
        }
    }

    ctx.data_mut(|d| {
        d.insert_temp(heights_id, heights);
        d.insert_temp(positions_id, positions);
    });
}

/// One component card on the settings page; `lifted` styles it as being dragged.
fn settings_card(ui: &mut Ui, group: &Group, settings: &mut Settings, handle_id: egui::Id, lifted: bool) -> Rect {
    let visible = settings.group_visible(group);
    let color = theme::kind_color(group.kind);

    let mut frame = Frame::new()
        .fill(if lifted { theme::RAISED } else { theme::SURFACE })
        .stroke(egui::Stroke::new(1.0, if lifted { theme::ACCENT } else { theme::BORDER }))
        .corner_radius(CornerRadius::same(theme::CARD_RADIUS))
        .inner_margin(Margin::symmetric(20, 16));
    if lifted {
        frame = frame.shadow(egui::Shadow {
            offset: [0, 12],
            blur: 32,
            spread: 0,
            color: egui::Color32::from_black_alpha(150),
        });
    }

    frame
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                widgets::drag_handle(ui, handle_id);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(
                        egui::RichText::new(group.kind.label().to_uppercase())
                            .font(theme::semibold(10.5))
                            .color(if visible { color } else { theme::TEXT_FAINT }),
                    );
                    ui.label(egui::RichText::new(&group.title).font(theme::semibold(15.0)).color(if visible {
                        theme::TEXT
                    } else {
                        theme::TEXT_DIM
                    }));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::toggle(ui, visible).clicked() {
                        settings.toggle_group(group);
                    }
                });
            });

            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                for r in &group.readings {
                    let on = settings.reading_visible(&group.id, r);
                    if widgets::chip(ui, r.label, on, visible, color).clicked() {
                        settings.toggle_reading(&group.id, r);
                    }
                }
            });
        })
        .response
        .rect
}

// ---------------------------------------------------------------------------
// Cards shared by the Components page

fn subsection(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.label(egui::RichText::new(title).font(theme::semibold(16.0)).color(theme::TEXT));
    if !subtitle.is_empty() {
        ui.add_space(-8.0);
        ui.label(egui::RichText::new(subtitle).font(theme::regular(12.0)).color(theme::TEXT_FAINT));
    }
    ui.add_space(4.0);
}

fn card_frame() -> Frame {
    Frame::new()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(theme::CARD_RADIUS))
        .inner_margin(Margin::symmetric(20, 14))
}

/// Label/value pairs flowing in as many columns as fit, filled column by column so
/// related facts stay together.
fn details_card(ui: &mut Ui, facts: &[crate::sensors::Info]) {
    const MIN_COLUMN: f32 = 300.0;
    const ROW: f32 = 26.0;
    card_frame().show(ui, |ui| {
        let width = ui.available_width();
        let columns = ((width / MIN_COLUMN) as usize).clamp(1, 3);
        let rows = facts.len().div_ceil(columns);
        let column = width / columns as f32;
        let (rect, _) = ui.allocate_exact_size(vec2(width, rows as f32 * ROW), Sense::hover());
        let painter = ui.painter_at(rect);
        let label_width = (column * 0.38).min(150.0);
        for (i, fact) in facts.iter().enumerate() {
            let (col, row) = (i / rows, i % rows);
            let origin = rect.min + vec2(col as f32 * column, row as f32 * ROW + ROW / 2.0);
            let label =
                widgets::elided(&painter, &fact.label, theme::regular(12.5), theme::TEXT_DIM, label_width - 10.0);
            painter.galley(origin - vec2(0.0, label.size().y / 2.0), label, theme::TEXT_DIM);
            let value =
                widgets::elided(&painter, &fact.value, theme::regular(13.0), theme::TEXT, column - label_width - 16.0);
            painter.galley(origin + vec2(label_width, -value.size().y / 2.0), value, theme::TEXT);
        }
    });
}

/// Ranked list with a bar per program, relative to the busiest one (or to 100 % for
/// percentages, so a lone 3 % process does not look maxed out).
fn processes_card(ui: &mut Ui, processes: &crate::sensors::Processes, color: egui::Color32) {
    const ROW: f32 = 30.0;
    card_frame().show(ui, |ui| {
        let width = ui.available_width();
        if processes.items.is_empty() {
            ui.label(
                egui::RichText::new("Nothing measurable right now").font(theme::regular(12.5)).color(theme::TEXT_FAINT),
            );
            return;
        }
        let (rect, _) = ui.allocate_exact_size(vec2(width, processes.items.len() as f32 * ROW), Sense::hover());
        let painter = ui.painter_at(rect);
        let max = match processes.unit {
            crate::sensors::Unit::Percent => 100.0,
            _ => processes.items.iter().map(|p| p.value).fold(0.0, f64::max).max(f64::EPSILON),
        };
        let value_width = 96.0;
        let bar_width = (width * 0.28).clamp(60.0, 220.0);
        let name_width = width - value_width - bar_width - 24.0;
        for (i, p) in processes.items.iter().enumerate() {
            let y = rect.top() + i as f32 * ROW + ROW / 2.0;
            let name = if p.count > 1 { format!("{}  ×{}", p.name, p.count) } else { p.name.clone() };
            let galley = widgets::elided(&painter, &name, theme::regular(13.0), theme::TEXT, name_width);
            painter.galley(egui::pos2(rect.left(), y - galley.size().y / 2.0), galley, theme::TEXT);

            let track = Rect::from_min_size(
                egui::pos2(rect.right() - value_width - bar_width - 12.0, y - 3.0),
                vec2(bar_width, 6.0),
            );
            painter.rect_filled(track, CornerRadius::same(3), theme::TRACK);
            let fill = (p.value / max).clamp(0.0, 1.0) as f32;
            painter.rect_filled(
                Rect::from_min_size(track.min, vec2((bar_width * fill).max(3.0), 6.0)),
                CornerRadius::same(3),
                color,
            );
            painter.text(
                egui::pos2(rect.right(), y),
                Align2::RIGHT_CENTER,
                widgets::format_value(p.value, processes.unit),
                theme::semibold(13.0),
                theme::TEXT,
            );
        }
    });
}

/// As many columns as fit (min. cell width), preferring a count that divides the
/// items evenly so rows end up full (16 threads → 8 × 2 or 4 × 4).
fn grid_columns(width: f32, items: usize) -> usize {
    const MIN_CELL: f32 = 128.0;
    let max = ((width / MIN_CELL) as usize).clamp(2, 8).min(items.max(1));
    (2..=max).rev().find(|c| items.is_multiple_of(*c)).unwrap_or(max)
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

/// Sits right below the content, or at the bottom of the window when the content
/// is shorter than the window. `remaining` is the unused viewport height.
fn footer(ui: &mut Ui, settings: &mut Settings, remaining: f32) {
    const HEIGHT: f32 = 22.0;
    ui.add_space((remaining - HEIGHT).max(2.0));
    let interval = poller::interval().as_secs_f32();
    let history = (poller::HISTORY_LEN as f32 * interval).round() as u32;
    let history = if history >= 60 { format!("{} min", history / 60) } else { format!("{history} s") };
    let text = if poller::paused() {
        format!("Paused · {history} history")
    } else {
        format!("Polling every {interval} s · {history} history")
    };
    ui.vertical_centered(|ui| {
        ui.horizontal(|ui| {
            let link_width = 100.0;
            let galley = ui.painter().layout_no_wrap(text.clone(), theme::regular(11.5), theme::TEXT_FAINT);
            ui.add_space(((ui.available_width() - galley.size().x - link_width) / 2.0).max(0.0));
            ui.label(egui::RichText::new(text).font(theme::regular(11.5)).color(theme::TEXT_FAINT));
            if link(ui, "Compact view", theme::regular(11.5), theme::TEXT_DIM, false).clicked() {
                settings.compact = true;
            }
        });
    });
}
