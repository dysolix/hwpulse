//! Dark-only palette, fonts and global egui style.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, TextStyle,
};

use crate::icon;
use crate::sensors::{Kind, Unit};

pub const BG: Color32 = Color32::from_rgb(0x0B, 0x0D, 0x12);
/// Shared with the app icon, see [`crate::icon`].
pub const SURFACE: Color32 = rgb(icon::SURFACE);
pub const BORDER: Color32 = Color32::from_rgb(0x21, 0x25, 0x32);
pub const TRACK: Color32 = rgb(icon::TRACK);
/// Hovered cards, active tab.
pub const RAISED: Color32 = Color32::from_rgb(0x1C, 0x20, 0x2C);
pub const BORDER_HOVER: Color32 = Color32::from_rgb(0x34, 0x3A, 0x4E);
pub const ACCENT: Color32 = Color32::from_rgb(0x9A, 0xA6, 0xFF);
pub const TEXT: Color32 = Color32::from_rgb(0xE8, 0xEA, 0xF0);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8B, 0x91, 0xA3);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x5A, 0x60, 0x72);
pub const LIVE: Color32 = Color32::from_rgb(0x3D, 0xD6, 0x8C);
pub const ERROR: Color32 = Color32::from_rgb(0xF2, 0x54, 0x5B);

pub const CARD_RADIUS: u8 = 14;

/// Accent colour for a reading. Temperatures shift from cool to hot with the value.
pub fn accent(unit: Unit, value: f64) -> Color32 {
    match unit {
        Unit::Celsius => heat(value),
        Unit::Percent => Color32::from_rgb(0x7C, 0x8C, 0xFF),
        Unit::Watts => Color32::from_rgb(0xF5, 0xB8, 0x3D),
        Unit::Megahertz => Color32::from_rgb(0x4F, 0xD1, 0xC5),
        Unit::Mebibytes => Color32::from_rgb(0xC0, 0x84, 0xFC),
        Unit::BytesPerSec => Color32::from_rgb(0x5C, 0xC8, 0xFF),
        Unit::BitsPerSec => Color32::from_rgb(0xF4, 0x72, 0xB6),
        Unit::Milliseconds => Color32::from_rgb(0xFF, 0x9F, 0x6B),
        Unit::PerSecond | Unit::Count => Color32::from_rgb(0x94, 0xA3, 0xB8),
    }
}

pub fn kind_color(kind: Kind) -> Color32 {
    match kind {
        Kind::Cpu => Color32::from_rgb(0x7C, 0x8C, 0xFF),
        Kind::Gpu => Color32::from_rgb(0x3D, 0xD6, 0x8C),
        Kind::IntegratedGpu => Color32::from_rgb(0x2D, 0xB8, 0x9A),
        Kind::Memory => Color32::from_rgb(0xC0, 0x84, 0xFC),
        Kind::Disk => Color32::from_rgb(0x5C, 0xC8, 0xFF),
        Kind::ExternalDisk => Color32::from_rgb(0x38, 0xBD, 0xC8),
        Kind::Network => Color32::from_rgb(0xF4, 0x72, 0xB6),
        Kind::Battery => Color32::from_rgb(0xA3, 0xE6, 0x35),
    }
}

/// Cool to hot, the same gradient as the icon's gauge.
pub fn heat(celsius: f64) -> Color32 {
    rgb(icon::heat(celsius))
}

const fn rgb([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::Theme::Dark);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, regular(11.0)),
            (TextStyle::Body, regular(14.0)),
            (TextStyle::Button, regular(14.0)),
            (TextStyle::Heading, semibold(20.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(12.0, 12.0);
        style.spacing.window_margin = Margin::same(20);

        let v = &mut style.visuals;
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.extreme_bg_color = BG;
        v.override_text_color = Some(TEXT);
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(CARD_RADIUS);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        v.selection.bg_fill = Color32::from_rgb(0x2E, 0x36, 0x5C);
    });
}

/// Uses Segoe UI from the Windows font folder when available; falls back to egui's
/// built-in fonts otherwise. Nothing is bundled into the binary.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::path::Path::new(r"C:\Windows\Fonts");
    let load = |file: &str| std::fs::read(dir.join(file)).ok().map(|b| Arc::new(FontData::from_owned(b)));

    let regular = load("segoeui.ttf");
    let semibold = load("seguisb.ttf");

    if let Some(data) = regular {
        fonts.font_data.insert("segoe".into(), data);
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "segoe".into());
    }

    // The "semibold" family falls back to whatever proportional fonts exist.
    let mut semibold_chain = fonts.families[&FontFamily::Proportional].clone();
    if let Some(data) = semibold {
        fonts.font_data.insert("segoe-semibold".into(), data);
        semibold_chain.insert(0, "segoe-semibold".into());
    }
    fonts.families.insert(FontFamily::Name("semibold".into()), semibold_chain);

    ctx.set_fonts(fonts);
}
