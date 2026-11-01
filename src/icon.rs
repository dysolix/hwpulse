//! The app icon, drawn procedurally: a dark rounded tile with a cool-to-hot gauge arc.
//! Plain Rust without dependencies, because `build.rs` includes this file too: it
//! renders the `.ico` that is embedded into the exe (Explorer, Start Menu, taskbar),
//! while the app uses [`render`] for its window icon. One drawing, never out of date.
//!
//! The colours the icon shares with the UI live here; `ui::theme` takes them from here.

/// Card background.
pub const SURFACE: [u8; 3] = [0x13, 0x16, 0x1E];
/// Empty part of gauges and bars.
pub const TRACK: [u8; 3] = [0x1F, 0x23, 0x30];

/// Temperature colours from cool to hot, as (°C, colour) stops.
const HEAT: [(f64, [u8; 3]); 4] =
    [(35.0, [0x3D, 0xD6, 0xA6]), (60.0, [0x9B, 0xD8, 0x4A]), (72.0, [0xF5, 0xC5, 0x42]), (85.0, [0xF2, 0x54, 0x5B])];

/// The colour for a temperature, blended between the [`HEAT`] stops.
pub fn heat(celsius: f64) -> [u8; 3] {
    if celsius <= HEAT[0].0 {
        return HEAT[0].1;
    }
    for w in HEAT.windows(2) {
        let ((t0, c0), (t1, c1)) = (w[0], w[1]);
        if celsius <= t1 {
            let t = ((celsius - t0) / (t1 - t0)) as f32;
            let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
            return [lerp(c0[0], c1[0]), lerp(c0[1], c1[1]), lerp(c0[2], c1[2])];
        }
    }
    HEAT[3].1
}

/// The icon at `size` × `size` pixels as RGBA, rows top to bottom.
pub fn render(size: u32) -> Vec<u8> {
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
            let mut color = blend([0; 4], SURFACE, tile_alpha);

            // Gauge ring: track first, then the coloured value part.
            let (dx, dy) = (px - s / 2.0, py - s / 2.0 + s * 0.03);
            let ring_dist = (dx.hypot(dy) - radius).abs() - half_width;
            let angle = (dy.atan2(dx) - start).rem_euclid(std::f32::consts::TAU);
            if angle <= sweep + 0.15 {
                let t = (angle / sweep).min(1.0);
                let end_fade = coverage((angle - sweep) * radius);
                let ring = if t <= filled { heat(35.0 + 55.0 * (t / filled) as f64) } else { TRACK };
                color = blend(color, ring, coverage(ring_dist) * end_fade * tile_alpha);
            }

            rgba.extend_from_slice(&color);
        }
    }

    rgba
}

/// Anti-aliased coverage for a signed distance (negative = inside).
fn coverage(dist: f32) -> f32 {
    (0.5 - dist).clamp(0.0, 1.0)
}

/// `over` painted with `alpha` onto the unpremultiplied RGBA colour `under`.
fn blend(under: [u8; 4], over: [u8; 3], alpha: f32) -> [u8; 4] {
    let a = alpha.clamp(0.0, 1.0);
    let mix = |u: u8, o: u8| (u as f32 + (o as f32 - u as f32) * a).round() as u8;
    let out_a = under[3] as f32 / 255.0 + a * (1.0 - under[3] as f32 / 255.0);
    let [r, g, b] =
        if under[3] == 0 { over } else { [mix(under[0], over[0]), mix(under[1], over[1]), mix(under[2], over[2])] };
    [r, g, b, (out_a * 255.0).round() as u8]
}

/// A Windows `.ico` file with the icon at the usual sizes, 16 to 256 pixels. Only
/// `build.rs` uses it.
#[allow(dead_code)]
pub fn ico() -> Vec<u8> {
    const SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];
    let images: Vec<Vec<u8>> = SIZES.iter().map(|&size| dib(size, &render(size))).collect();
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
    ico
}

/// One icon image as a 32-bit DIB: header, BGRA rows bottom-up, empty AND mask.
#[allow(dead_code)]
fn dib(size: u32, rgba: &[u8]) -> Vec<u8> {
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
