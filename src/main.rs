// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod poller;
mod report;
mod sensors;
mod services;
mod settings;
mod shell;
mod startup;
mod tasks;
mod ui;
mod users;

use eframe::egui;

fn main() -> eframe::Result {
    let model = poller::spawn();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Hardware Monitor")
            .with_inner_size([1180.0, 940.0])
            .with_min_inner_size([520.0, 420.0])
            .with_icon(std::sync::Arc::new(ui::window::app_icon())),
        ..Default::default()
    };
    eframe::run_native("hwmon", options, Box::new(|cc| Ok(Box::new(ui::App::new(cc, model)))))
}
