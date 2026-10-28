pub mod theme;
pub mod widgets;

use eframe::egui::{self, Align2, CentralPanel, Frame, Margin, ScrollArea, Sense, Ui, vec2};

use crate::poller::{self, Model, Shared};
use crate::sensors::Group;

pub struct App {
    model: Shared,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        Self { model: poller::spawn(cc.egui_ctx.clone()) }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let model = self.model.lock().unwrap();

        CentralPanel::default()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(22)))
            .show(ui, |ui| {
                header(ui, &model);
                ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    for error in &model.errors {
                        ui.label(egui::RichText::new(error).color(theme::ERROR).font(theme::regular(13.0)));
                    }
                    if model.groups.is_empty() && model.errors.is_empty() {
                        ui.label(egui::RichText::new("Waiting for sensors…").color(theme::TEXT_DIM));
                    }
                    for group in &model.groups {
                        group_section(ui, group, &model);
                        ui.add_space(14.0);
                    }
                    footer(ui);
                });
            });
    }
}

fn header(ui: &mut Ui, model: &Model) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    let p = ui.painter();
    p.text(rect.left_center(), Align2::LEFT_CENTER, "Hardware Monitor", theme::semibold(22.0), theme::TEXT);

    // "Live" pill on the right.
    let live = !model.groups.is_empty();
    let (dot, label) = if live { (theme::LIVE, "Live") } else { (theme::TEXT_FAINT, "Idle") };
    let galley = p.layout_no_wrap(label.to_owned(), theme::semibold(12.0), theme::TEXT_DIM);
    let pill_w = galley.size().x + 32.0;
    let pill = egui::Rect::from_center_size(rect.right_center() - vec2(pill_w / 2.0, 0.0), vec2(pill_w, 24.0));
    p.rect_filled(pill, egui::CornerRadius::same(12), theme::SURFACE);
    p.circle_filled(pill.left_center() + vec2(13.0, 0.0), 3.5, dot);
    p.circle_filled(pill.left_center() + vec2(13.0, 0.0), 6.5, dot.gamma_multiply(0.18));
    p.galley(pill.left_center() + vec2(22.0, -galley.size().y / 2.0), galley, theme::TEXT_DIM);

    ui.add_space(10.0);
}

fn group_section(ui: &mut Ui, group: &Group, model: &Model) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(&group.title).font(theme::semibold(16.0)).color(theme::TEXT));
    ui.add_space(-8.0);
    ui.label(egui::RichText::new(&group.subtitle).font(theme::regular(12.0)).color(theme::TEXT_FAINT));
    ui.add_space(4.0);

    let history = |id: &str| model.history.get(&format!("{}/{}", group.id, id));

    let featured: Vec<_> = group.readings.iter().filter(|r| r.featured).collect();
    if !featured.is_empty() {
        ui.columns(featured.len(), |cols| {
            for (col, r) in cols.iter_mut().zip(&featured) {
                widgets::gauge_card(col, r, history(r.id));
            }
        });
    }

    const PER_ROW: usize = 3;
    let stats: Vec<_> = group.readings.iter().filter(|r| !r.featured).collect();
    for row in stats.chunks(PER_ROW) {
        ui.columns(PER_ROW, |cols| {
            for (col, r) in cols.iter_mut().zip(row) {
                widgets::stat_card(col, r, history(r.id));
            }
        });
    }
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
