//! Native window chrome: tints the Windows 11 title bar and border to match the app
//! (keeps snap layouts, resize edges and shadows), and sets the window icon.

use std::ffi::c_void;

use eframe::egui::{Color32, IconData};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWINDOWATTRIBUTE,
    DwmSetWindowAttribute,
};

use super::theme;

/// Best effort: older Windows versions ignore the colour attributes.
pub fn style_title_bar(window: &impl HasWindowHandle) {
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else { return };
    let hwnd = HWND(win32.hwnd.get() as *mut c_void);

    set(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &1i32);
    set(hwnd, DWMWA_CAPTION_COLOR, &colorref(theme::BG));
    set(hwnd, DWMWA_BORDER_COLOR, &colorref(theme::BORDER));
    set(hwnd, DWMWA_TEXT_COLOR, &colorref(theme::TEXT_DIM));
}

fn set<T>(hwnd: HWND, attribute: DWMWINDOWATTRIBUTE, value: &T) {
    let _ =
        unsafe { DwmSetWindowAttribute(hwnd, attribute, value as *const T as *const c_void, size_of::<T>() as u32) };
}

fn colorref(c: Color32) -> COLORREF {
    COLORREF(c.r() as u32 | (c.g() as u32) << 8 | (c.b() as u32) << 16)
}

/// The window icon: the same drawing `build.rs` embeds into the exe (see `crate::icon`).
pub fn app_icon() -> IconData {
    const SIZE: u32 = 64;
    IconData { rgba: crate::icon::render(SIZE), width: SIZE, height: SIZE }
}
