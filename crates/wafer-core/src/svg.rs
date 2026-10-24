//! Draws a laid-out schematic as SVG, colored by live signal values.

use std::fmt::Write as _;

use crate::gate::GateKind;
use crate::logic::{Logic, format_bits};
use crate::schematic::{
    CHAR_W, CHIP_HEADER, GATE_W, MARGIN, NodeKind, PORT_H, SMALL_CHAR_W, Schematic, TagKind, port_label,
};

pub struct Theme {
    pub background: &'static str,
    pub grid: &'static str,
    pub gate_fill: &'static str,
    pub gate_stroke: &'static str,
    pub text: &'static str,
    pub muted: &'static str,
    pub zero: &'static str,
    pub one: &'static str,
    pub unknown: &'static str,
    pub floating: &'static str,
    pub bus: &'static str,
}

/// The Terminal green look picked in round 3.
pub const TERMINAL_GREEN: Theme = Theme {
    background: "#0f1411",
    grid: "#1e2a22",
    gate_fill: "#16201a",
    gate_stroke: "#5f8f6c",
    text: "#c8e6cf",
    muted: "#6f8f78",
    zero: "#25523a",
    one: "#4dff88",
    unknown: "#ff5f56",
    floating: "#5d6b62",
    bus: "#e6d36a",
};

/// How a wire carrying `value` is stroked.
struct Stroke {
    color: &'static str,
    width: f32,
    dashed: bool,
}

fn stroke(theme: &Theme, width: usize, value: Option<&[Logic]>) -> Stroke {
    let s = |color, width, dashed| Stroke { color, width, dashed };
    match value {
        None if width > 1 => s(theme.zero, 5.0, false),
        None => s(theme.zero, 2.5, false),
        Some([bit]) => match bit {
            Logic::One => s(theme.one, 3.5, false),
            Logic::Zero => s(theme.zero, 2.5, false),
            Logic::X => s(theme.unknown, 3.0, false),
            Logic::Z => s(theme.floating, 2.5, true),
        },
        Some(bits) if bits.contains(&Logic::X) => s(theme.unknown, 5.0, false),
        Some(bits) if bits.iter().all(|&b| b == Logic::Z) => s(theme.floating, 4.0, true),
        Some(_) => s(theme.bus, 5.0, false),
    }
}

/// Fill and text color for a port showing `value`.
fn port_colors(theme: &Theme, value: Option<&[Logic]>) -> (&'static str, &'static str) {
    match value {
        Some([Logic::One]) => (theme.one, theme.background),
        Some([Logic::X]) => (theme.unknown, theme.background),
        _ => (theme.gate_fill, theme.text),
    }
}

/// Renders the schematic. `values` looks up a signal's current value by
/// path; `subtitle` goes under the chip name.
pub fn render(
    schematic: &Schematic,
    theme: &Theme,
    values: &dyn Fn(&str) -> Option<Vec<Logic>>,
    subtitle: &str,
) -> String {
    let (w, h) = (schematic.width, schematic.height);
    let mut out = String::new();
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}" font-family="Menlo, Monaco, 'DejaVu Sans Mono', monospace">"#
    );
    let _ = writeln!(
        out,
        r#"<defs><pattern id="grid" width="20" height="20" patternUnits="userSpaceOnUse"><circle cx="10" cy="10" r="1" fill="{}"/></pattern></defs>"#,
        theme.grid
    );
    let _ = writeln!(out, r#"<rect width="100%" height="100%" fill="{}"/>"#, theme.background);
    let _ = writeln!(out, r#"<rect width="100%" height="100%" fill="url(#grid)"/>"#);
    text(&mut out, MARGIN, 30.0, 16.0, theme.text, "start", &schematic.chip);
    text(&mut out, MARGIN, 48.0, 11.0, theme.muted, "start", subtitle);

    // Wires first, so boxes sit on top of their ends.
    // Floating (Z) wires first, so a live value drawn over the same spot wins.
    let floating = |w: &&crate::schematic::Wire| values(&w.signal).is_some_and(|v| v.iter().all(|&b| b == Logic::Z));
    let mut order: Vec<&crate::schematic::Wire> = schematic.wires.iter().collect();
    order.sort_by_key(|w| !floating(w));
    for wire in order {
        let value = values(&wire.signal);
        let s = stroke(theme, wire.width, value.as_deref());
        let mut d = String::new();
        for seg in &wire.segments {
            let _ = write!(d, "M{:.1},{:.1}L{:.1},{:.1}", seg[0], seg[1], seg[2], seg[3]);
        }
        let dash = if s.dashed { r#" stroke-dasharray="6 5""# } else { "" };
        let _ = writeln!(
            out,
            r#"<path d="{d}" fill="none" stroke="{}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round"{dash}/>"#,
            s.color, s.width
        );
        for &(x, y) in &wire.dots {
            let _ = writeln!(out, r#"<circle cx="{x:.1}" cy="{y:.1}" r="{}" fill="{}"/>"#, s.width + 1.5, s.color);
        }
    }

    for node in &schematic.nodes {
        let (x, y) = (node.x, node.y);
        match &node.kind {
            NodeKind::Input { name, width } => {
                let value = values(name);
                port_box(&mut out, theme, x, y, node.w, *width, value.as_deref());
                text(&mut out, x - 8.0, y + PORT_H / 2.0, 12.0, theme.text, "end", &port_label(name, *width));
            }
            NodeKind::Output { name, width } => {
                let value = values(name);
                if *width == 1 {
                    let (fill, _) = port_colors(theme, value.as_deref());
                    let _ = writeln!(
                        out,
                        r#"<circle cx="{:.1}" cy="{:.1}" r="11" fill="{fill}" stroke="{}" stroke-width="1.5"/>"#,
                        x + node.w / 2.0,
                        y + PORT_H / 2.0,
                        theme.gate_stroke
                    );
                } else {
                    port_box(&mut out, theme, x, y, node.w, *width, value.as_deref());
                }
                text(
                    &mut out,
                    x + node.w + 8.0,
                    y + PORT_H / 2.0,
                    12.0,
                    theme.text,
                    "start",
                    &port_label(name, *width),
                );
            }
            NodeKind::Gate { name, kind, label } if kind.is_sequential() => {
                block(&mut out, theme, node, label, name, true);
            }
            NodeKind::Gate { name, kind, .. } => {
                gate(&mut out, theme, *kind, x, y);
                text(&mut out, x + GATE_W / 2.0, y - 9.0, 11.0, theme.muted, "middle", name);
            }
            NodeKind::Chip { name, chip } => block(&mut out, theme, node, chip, name, false),
        }
    }

    for tag in &schematic.tags {
        match tag.kind {
            TagKind::Constant => {
                let tw = tag.text.chars().count() as f32 * SMALL_CHAR_W + 10.0;
                let _ = writeln!(
                    out,
                    r#"<rect x="{:.1}" y="{:.1}" width="{tw:.1}" height="16" rx="3" fill="{}" stroke="{}" stroke-width="1"/>"#,
                    tag.x - tw,
                    tag.y - 8.0,
                    theme.gate_fill,
                    theme.gate_stroke
                );
                text(&mut out, tag.x - 5.0, tag.y, 11.0, theme.text, "end", &tag.text);
            }
            TagKind::Bits => text(&mut out, tag.x, tag.y, 10.0, theme.muted, "end", &tag.text),
        }
    }

    // Bus values next to where each bus starts.
    for wire in &schematic.wires {
        let Some((lx, ly)) = wire.label_at.filter(|_| wire.width > 1) else { continue };
        if let Some(value) = values(&wire.signal) {
            let s = stroke(theme, wire.width, Some(&value));
            let label = format_bits(&value);
            let tw = label.chars().count() as f32 * SMALL_CHAR_W + 6.0;
            let _ = writeln!(
                out,
                r#"<rect x="{:.1}" y="{:.1}" width="{tw:.1}" height="14" rx="3" fill="{}"/>"#,
                lx - 3.0,
                ly - 7.0,
                theme.background
            );
            text(&mut out, lx, ly, 11.0, s.color, "start", &label);
        }
    }

    out.push_str("</svg>\n");
    out
}

fn port_box(out: &mut String, theme: &Theme, x: f32, y: f32, w: f32, width: usize, value: Option<&[Logic]>) {
    let (fill, text_color) = port_colors(theme, value);
    let border = if width > 1 { theme.bus } else { theme.gate_stroke };
    let _ = writeln!(
        out,
        r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{PORT_H}" rx="5" fill="{fill}" stroke="{border}" stroke-width="1.5"/>"#
    );
    let shown = value.map(format_bits).unwrap_or_default();
    text(out, x + w / 2.0, y + PORT_H / 2.0, 12.0, text_color, "middle", &shown);
}

/// A chip (or a box-shaped built-in like Dff and Ram): a labeled box with
/// its pin names inside, and a clock triangle if it ticks.
fn block(out: &mut String, theme: &Theme, node: &crate::schematic::Node, title: &str, name: &str, clocked: bool) {
    let (x, y) = (node.x, node.y);
    let _ = writeln!(
        out,
        r#"<rect x="{x:.1}" y="{y:.1}" width="{:.1}" height="{:.1}" rx="6" fill="{}" stroke="{}" stroke-width="1.5"/>"#,
        node.w, node.h, theme.gate_fill, theme.gate_stroke
    );
    text(out, x + node.w / 2.0, y + 16.0, 12.0, theme.text, "middle", title);
    let _ = writeln!(
        out,
        r#"<line x1="{x:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{}" stroke-width="1"/>"#,
        y + CHIP_HEADER - 2.0,
        x + node.w,
        y + CHIP_HEADER - 2.0,
        theme.grid
    );
    for pin in &node.inputs {
        text(out, pin.x + 8.0, pin.y, 11.0, theme.muted, "start", &pin.name);
    }
    for pin in &node.outputs {
        text(out, pin.x - 8.0, pin.y, 11.0, theme.muted, "end", &pin.name);
    }
    if clocked {
        let b = y + node.h;
        let _ = writeln!(
            out,
            r#"<path d="M{x:.1},{:.1} L{:.1},{:.1} L{x:.1},{:.1}" fill="none" stroke="{}" stroke-width="1.5"/>"#,
            b - 16.0,
            x + 9.0,
            b - 10.0,
            b - 4.0,
            theme.gate_stroke
        );
    }
    text(out, x + node.w / 2.0, y - 9.0, 11.0, theme.muted, "middle", name);
}

/// Classic gate symbols in a 66×50 box: inputs on the left, output at the
/// middle of the right edge.
fn gate(out: &mut String, theme: &Theme, kind: GateKind, x: f32, y: f32) {
    let stroke = theme.gate_stroke;
    match kind {
        GateKind::Mux => {
            let _ = writeln!(
                out,
                r#"<g transform="translate({x:.1},{y:.1})" stroke="{stroke}" stroke-width="2" fill="none">"#
            );
            for iy in [12.0, 25.0, 38.0] {
                let _ = writeln!(out, r#"<line x1="0" y1="{iy}" x2="6" y2="{iy}"/>"#);
            }
            let _ = writeln!(out, r#"<path d="M6,0 L54,10 L54,40 L6,50 Z" fill="{}"/>"#, theme.gate_fill);
            for (iy, label) in [(12.0, "0"), (25.0, "1"), (38.0, "s")] {
                let _ = writeln!(
                    out,
                    r#"<text x="10" y="{:.1}" font-size="9" fill="{}" stroke="none">{label}</text>"#,
                    iy + 3.0,
                    theme.muted
                );
            }
            let _ = writeln!(out, r#"<line x1="54" y1="25" x2="{GATE_W}" y2="25"/>"#);
            out.push_str("</g>\n");
            return;
        }
        GateKind::TriBuf => {
            let _ = writeln!(
                out,
                r#"<g transform="translate({x:.1},{y:.1})" stroke="{stroke}" stroke-width="2" fill="none">"#
            );
            let _ = writeln!(out, r#"<line x1="0" y1="25" x2="6" y2="25"/>"#);
            let _ = writeln!(out, r#"<path d="M0,45 H28 V35"/>"#);
            let _ = writeln!(out, r#"<path d="M6,5 L50,25 L6,45 Z" fill="{}"/>"#, theme.gate_fill);
            let _ = writeln!(out, r#"<line x1="50" y1="25" x2="{GATE_W}" y2="25"/>"#);
            out.push_str("</g>\n");
            return;
        }
        _ => {}
    }
    let (body, extra, back_x, tip): (&str, Option<&str>, f32, f32) = match kind {
        GateKind::And | GateKind::Nand => ("M0,0 H30 A25,25 0 0 1 30,50 H0 Z", None, 0.0, 55.0),
        GateKind::Or | GateKind::Nor => ("M0,0 Q14,25 0,50 Q36,50 55,25 Q36,0 0,0 Z", None, 6.0, 55.0),
        GateKind::Xor | GateKind::Xnor => {
            ("M6,0 Q19,25 6,50 Q38,50 56,25 Q38,0 6,0 Z", Some("M0,0 Q13,25 0,50"), 11.5, 56.0)
        }
        GateKind::Not => ("M4,7 L50,25 L4,43 Z", None, 4.0, 50.0),
        GateKind::Mux | GateKind::TriBuf | GateKind::Dff | GateKind::Ram => unreachable!("drawn separately"),
    };
    let bubble = matches!(kind, GateKind::Nand | GateKind::Nor | GateKind::Xnor | GateKind::Not);
    let _ = writeln!(out, r#"<g transform="translate({x:.1},{y:.1})" stroke="{stroke}" stroke-width="2" fill="none">"#);
    let ins: &[f32] = if kind == GateKind::Not { &[25.0] } else { &[15.0, 35.0] };
    for &iy in ins {
        if back_x > 0.0 {
            let _ = writeln!(out, r#"<line x1="0" y1="{iy}" x2="{back_x}" y2="{iy}"/>"#);
        }
    }
    if let Some(extra) = extra {
        let _ = writeln!(out, r#"<path d="{extra}"/>"#);
    }
    let _ = writeln!(out, r#"<path d="{body}" fill="{}"/>"#, theme.gate_fill);
    let stub_start = if bubble {
        let _ = writeln!(out, r#"<circle cx="{}" cy="25" r="4.5" fill="{}"/>"#, tip + 4.5, theme.gate_fill);
        tip + 9.0
    } else {
        tip
    };
    let _ = writeln!(out, r#"<line x1="{stub_start}" y1="25" x2="{GATE_W}" y2="25"/>"#);
    out.push_str("</g>\n");
}

/// Text vertically centered on `y`.
fn text(out: &mut String, x: f32, y: f32, size: f32, color: &str, anchor: &str, content: &str) {
    if content.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        r#"<text x="{x:.1}" y="{:.1}" font-size="{size}" fill="{color}" text-anchor="{anchor}">{}</text>"#,
        y + size * 0.35,
        escape(content)
    );
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Rough width of a text label, for layout.
pub fn text_width(s: &str) -> f32 {
    s.chars().count() as f32 * CHAR_W
}
