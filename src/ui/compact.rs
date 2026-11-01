//! Compact view: a small window with the dashboard's values as one list, Task
//! Manager's "Fewer details". Pairs well with "Always on top".

use eframe::egui::{self, Align2, CornerRadius, ScrollArea, Sense, Ui, pos2, vec2};

use super::{theme, widgets};
use crate::poller::Model;
use crate::settings::Settings;

pub const SIZE: egui::Vec2 = vec2(330.0, 520.0);
pub const MIN_SIZE: egui::Vec2 = vec2(240.0, 160.0);
const ROW: f32 = 26.0;

pub fn page(ui: &mut Ui, model: &Model, settings: &mut Settings) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("HWPulse").font(theme::semibold(14.0)).color(theme::TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if super::link(ui, "Full view", theme::regular(12.5), theme::TEXT_DIM, true).clicked() {
                settings.compact = false;
            }
        });
    });
    ui.add_space(-4.0);

    let groups = settings.dashboard_groups(&model.groups);
    if groups.is_empty() {
        let text = if model.groups.is_empty() { "Waiting for sensors…" } else { "Everything is hidden" };
        ui.label(egui::RichText::new(text).font(theme::regular(12.5)).color(theme::TEXT_FAINT));
        return;
    }
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for group in &groups {
            ui.add_space(10.0);
            let color = theme::kind_color(group.kind);
            ui.label(egui::RichText::new(&group.title).font(theme::semibold(12.5)).color(color));
            ui.add_space(2.0);
            for reading in &group.readings {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
                let painter = ui.painter_at(rect);
                let value = widgets::format_value(reading.value, reading.unit);
                let value_width = painter.layout_no_wrap(value.clone(), theme::semibold(13.0), theme::TEXT).size().x;
                let label = widgets::elided(
                    &painter,
                    reading.label,
                    theme::regular(12.5),
                    theme::TEXT_DIM,
                    rect.width() - value_width - 12.0,
                );
                painter.galley(pos2(rect.left(), rect.center().y - label.size().y / 2.0 - 2.0), label, theme::TEXT_DIM);
                painter.text(
                    rect.right_center() - vec2(0.0, 2.0),
                    Align2::RIGHT_CENTER,
                    value,
                    theme::semibold(13.0),
                    theme::TEXT,
                );
                // A thin bar for values with a known range (loads, temperatures …).
                if let Some((lo, hi)) = reading.range.filter(|(lo, hi)| hi > lo) {
                    let fill = ((reading.value - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
                    let track =
                        egui::Rect::from_min_size(pos2(rect.left(), rect.bottom() - 4.0), vec2(rect.width(), 2.0));
                    painter.rect_filled(track, CornerRadius::same(1), theme::TRACK);
                    let bar = egui::Rect::from_min_size(track.min, vec2(track.width() * fill, 2.0));
                    painter.rect_filled(bar, CornerRadius::same(1), theme::accent(reading.unit, reading.value));
                }
            }
        }
    });
}
