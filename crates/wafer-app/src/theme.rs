//! The Terminal green look: canvas colors, wire styles, and egui visuals.

use egui::Color32;
use wafer_core::Logic;

pub const BACKGROUND: Color32 = Color32::from_rgb(0x0f, 0x14, 0x11);
pub const PANEL: Color32 = Color32::from_rgb(0x12, 0x18, 0x14);
pub const GRID: Color32 = Color32::from_rgb(0x1e, 0x2a, 0x22);
pub const GATE_FILL: Color32 = Color32::from_rgb(0x16, 0x20, 0x1a);
pub const GATE_STROKE: Color32 = Color32::from_rgb(0x5f, 0x8f, 0x6c);
pub const TEXT: Color32 = Color32::from_rgb(0xc8, 0xe6, 0xcf);
pub const MUTED: Color32 = Color32::from_rgb(0x6f, 0x8f, 0x78);
pub const ZERO: Color32 = Color32::from_rgb(0x25, 0x52, 0x3a);
pub const ONE: Color32 = Color32::from_rgb(0x4d, 0xff, 0x88);
pub const UNKNOWN: Color32 = Color32::from_rgb(0xff, 0x5f, 0x56);
pub const FLOATING: Color32 = Color32::from_rgb(0x5d, 0x6b, 0x62);
pub const BUS: Color32 = Color32::from_rgb(0xe6, 0xd3, 0x6a);
pub const SELECTED: Color32 = Color32::from_rgb(0xa6, 0xff, 0xcb);
pub const WARNING: Color32 = Color32::from_rgb(0xff, 0xc8, 0x57);

pub struct WireStyle {
    pub color: Color32,
    /// In world units; the canvas scales it with zoom.
    pub width: f32,
    pub dashed: bool,
}

/// Same rules as the SVG renderer, so the app and `wafer render` agree.
pub fn wire_style(bits: usize, value: Option<&[Logic]>) -> WireStyle {
    let s = |color, width, dashed| WireStyle { color, width, dashed };
    match value {
        None if bits > 1 => s(ZERO, 5.0, false),
        None => s(ZERO, 2.5, false),
        Some([bit]) => match bit {
            Logic::One => s(ONE, 3.5, false),
            Logic::Zero => s(ZERO, 2.5, false),
            Logic::X => s(UNKNOWN, 3.0, false),
            Logic::Z => s(FLOATING, 2.5, true),
        },
        Some(bits) if bits.contains(&Logic::X) => s(UNKNOWN, 5.0, false),
        Some(bits) if bits.iter().all(|&b| b == Logic::Z) => s(FLOATING, 4.0, true),
        Some(_) => s(BUS, 5.0, false),
    }
}

/// Fill and text color for a port box or lamp showing `value`.
pub fn port_colors(value: Option<&[Logic]>) -> (Color32, Color32) {
    match value {
        Some([Logic::One]) => (ONE, BACKGROUND),
        Some([Logic::X]) => (UNKNOWN, BACKGROUND),
        _ => (GATE_FILL, TEXT),
    }
}

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BACKGROUND;
    v.faint_bg_color = Color32::from_rgb(0x16, 0x1e, 0x19);
    v.code_bg_color = GATE_FILL;
    v.hyperlink_color = ONE;
    v.warn_fg_color = WARNING;
    v.error_fg_color = UNKNOWN;
    v.selection.bg_fill = Color32::from_rgb(0x1f, 0x5a, 0x38);
    v.selection.stroke.color = SELECTED;
    v.window_stroke.color = Color32::from_rgb(0x24, 0x33, 0x29);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke.color = Color32::from_rgb(0x24, 0x33, 0x29);
    w.noninteractive.fg_stroke.color = TEXT;
    w.inactive.bg_fill = GATE_FILL;
    w.inactive.weak_bg_fill = GATE_FILL;
    w.inactive.fg_stroke.color = TEXT;
    w.hovered.bg_fill = Color32::from_rgb(0x1e, 0x2e, 0x24);
    w.hovered.weak_bg_fill = Color32::from_rgb(0x1e, 0x2e, 0x24);
    w.hovered.bg_stroke.color = GATE_STROKE;
    w.active.bg_fill = Color32::from_rgb(0x24, 0x40, 0x2e);
    w.active.weak_bg_fill = Color32::from_rgb(0x24, 0x40, 0x2e);
    w.open.weak_bg_fill = Color32::from_rgb(0x1e, 0x2e, 0x24);

    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, v);
}
