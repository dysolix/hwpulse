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
    // DLLs loaded at run time (OpenGL, NVML …) come from System32 only, not from the
    // exe's folder or the working directory; see build.rs for the imported ones.
    let _ = unsafe {
        windows::Win32::System::LibraryLoader::SetDefaultDllDirectories(
            windows::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
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
