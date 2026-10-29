//! Native window chrome: tints the Windows 11 title bar and border to match the app
//! (keeps snap layouts, resize edges and shadows), and generates the app icon.

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
    let _ = unsafe { DwmSetWindowAttribute(hwnd, attribute, value as *const T as *const c_void, size_of::<T>() as u32) };
}

fn colorref(c: Color32) -> COLORREF {
    COLORREF(c.r() as u32 | (c.g() as u32) << 8 | (c.b() as u32) << 16)
}

/// The window icon. The same drawing, at several sizes, is embedded in the exe as
/// `assets/hwmon.ico` (see the ignored test below).
pub fn app_icon() -> IconData {
    const SIZE: u32 = 64;
    IconData { rgba: render_icon(SIZE), width: SIZE, height: SIZE }
}

/// Dark rounded tile with a cool-to-hot gauge arc as RGBA, drawn procedurally so no
/// image file or decoder is needed.
fn render_icon(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);

    let (start, sweep) = (0.75 * std::f32::consts::PI, 1.5 * std::f32::consts::PI);
    let (radius, half_width) = (s * 0.30, s * 0.075);
    let filled = 0.72;

    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);

            // Rounded tile.
            let corner = s * 0.22;
            let (qx, qy) = ((px - s / 2.0).abs() - (s / 2.0 - corner), (py - s / 2.0).abs() - (s / 2.0 - corner));
            let tile_dist = (qx.max(0.0).hypot(qy.max(0.0))) - corner;
            let tile_alpha = coverage(tile_dist);
            let mut color = blend(Color32::TRANSPARENT, theme::SURFACE, tile_alpha);

            // Gauge ring: track first, then the coloured value part.
            let (dx, dy) = (px - s / 2.0, py - s / 2.0 + s * 0.03);
            let ring_dist = (dx.hypot(dy) - radius).abs() - half_width;
            let angle = (dy.atan2(dx) - start).rem_euclid(std::f32::consts::TAU);
            if angle <= sweep + 0.15 {
                let t = (angle / sweep).min(1.0);
                let end_fade = coverage((angle - sweep) * radius);
                let ring = if t <= filled {
                    theme::heat(35.0 + 55.0 * (t / filled) as f64)
                } else {
                    theme::TRACK
                };
                color = blend(color, ring, coverage(ring_dist) * end_fade * tile_alpha);
            }

            rgba.extend_from_slice(&color.to_srgba_unmultiplied());
        }
    }

    rgba
}

/// Anti-aliased coverage for a signed distance (negative = inside).
fn coverage(dist: f32) -> f32 {
    (0.5 - dist).clamp(0.0, 1.0)
}

fn blend(under: Color32, over: Color32, alpha: f32) -> Color32 {
    let a = alpha.clamp(0.0, 1.0);
    let mix = |u: u8, o: u8| (u as f32 + (o as f32 - u as f32) * a).round() as u8;
    let out_a = under.a() as f32 / 255.0 + a * (1.0 - under.a() as f32 / 255.0);
    let [ur, ug, ub, _] = under.to_srgba_unmultiplied();
    let [or, og, ob, _] = over.to_srgba_unmultiplied();
    let base = if under.a() == 0 { [or, og, ob] } else { [mix(ur, or), mix(ug, og), mix(ub, ob)] };
    Color32::from_rgba_unmultiplied(base[0], base[1], base[2], (out_a * 255.0).round() as u8)
}

#[cfg(test)]
mod tests {
    /// Regenerates `assets/hwmon.ico`, which `build.rs` embeds into the exe:
    /// `cargo test write_icon_file -- --ignored`
    #[test]
    #[ignore]
    fn write_icon_file() {
        const SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];
        let images: Vec<Vec<u8>> = SIZES.iter().map(|&size| bmp_entry(size, &super::render_icon(size))).collect();
        let mut ico = Vec::new();
        ico.extend_from_slice(&[0, 0, 1, 0]); // reserved, type = icon
        ico.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());
        let mut offset = 6 + 16 * SIZES.len();
        for (&size, image) in SIZES.iter().zip(&images) {
            let dim = if size >= 256 { 0 } else { size as u8 };
            ico.extend_from_slice(&[dim, dim, 0, 0]);
            ico.extend_from_slice(&1u16.to_le_bytes()); // planes
            ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
            ico.extend_from_slice(&(image.len() as u32).to_le_bytes());
            ico.extend_from_slice(&(offset as u32).to_le_bytes());
            offset += image.len();
        }
        images.iter().for_each(|image| ico.extend_from_slice(image));
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("hwmon.ico");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, ico).unwrap();
    }

    /// One icon image as a 32-bit DIB: header, BGRA rows bottom-up, empty AND mask.
    fn bmp_entry(size: u32, rgba: &[u8]) -> Vec<u8> {
        let mask_row = size.div_ceil(32) * 4;
        let mut out = Vec::new();
        for v in [40, size, size * 2] {
            out.extend_from_slice(&v.to_le_bytes()); // header size, width, height (image + mask)
        }
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&[0; 24]); // no compression, sizes and palette left at 0
        for y in (0..size).rev() {
            for x in 0..size {
                let i = ((y * size + x) * 4) as usize;
                out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        out.resize(out.len() + (mask_row * size) as usize, 0);
        out
    }
}