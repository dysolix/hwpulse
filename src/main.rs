// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod icon;
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
            .with_title("HWPulse")
            .with_inner_size(first_size())
            .with_min_inner_size([520.0, 420.0])
            .with_icon(std::sync::Arc::new(ui::window::app_icon())),
        ..Default::default()
    };
    eframe::run_native("hwpulse", options, Box::new(|cc| Ok(Box::new(ui::App::new(cc, model)))))
}

/// 1180 × 940, or less on small screens (a 1366 × 768 laptop): within the primary
/// screen's work area, minus room for the title bar. Later starts reopen at the last size.
fn first_size() -> [f32; 2] {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut area = RECT::default();
    // hwpulse is not DPI-aware yet at this point, so Windows answers in scaled units,
    // which match egui's points.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut area as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    let (width, height) = match ok {
        Ok(()) => ((area.right - area.left) as f32 - 40.0, (area.bottom - area.top) as f32 - 60.0),
        Err(_) => (f32::MAX, f32::MAX),
    };
    [1180f32.min(width).max(520.0), 940f32.min(height).max(420.0)]
}
