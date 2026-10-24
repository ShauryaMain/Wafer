//! The waveform timeline: one lane per signal, time running left to right
//! in gate delays. Click or drag to scrub back in time (the canvas shows
//! the values at that step); double-click to go back to live.

use egui::{Align2, FontId, Rect, Sense, Shape, Stroke, pos2, vec2};
use wafer_core::logic::{Logic, format_bits};
use wafer_core::trace::Trace;

use crate::simrun::SimRunner;
use crate::theme;

const LABEL_W: f32 = 170.0;
const RULER_H: f32 = 22.0;
const LANE_H: f32 = 26.0;

pub struct TimelineView {
    /// Pixels per gate delay.
    pub scale: f32,
    /// The step at the left edge of the waves.
    left: f64,
    /// Keep the latest step in view.
    follow: bool,
    /// Vertical scroll of the lanes, in pixels.
    scroll: f32,
}

impl Default for TimelineView {
    fn default() -> TimelineView {
        TimelineView { scale: 18.0, left: 0.0, follow: true, scroll: 0.0 }
    }
}

impl TimelineView {
    /// Zooms so the whole recorded history fits.
    fn fit(&mut self, trace: &Trace, now: u64, width: f32) {
        let span = (now.max(trace.end) + 2).saturating_sub(trace.start).max(4) as f32;
        self.scale = (width / span).clamp(0.5, 60.0);
        self.left = trace.start as f64;
        self.follow = true;
    }
}

/// `scope` is the signal prefix of the chip on screen (`""` at the top).
pub fn ui(ui: &mut egui::Ui, view: &mut TimelineView, sim: &mut SimRunner, scope: &str) {
    let now = sim.now();
    let mut fit = false;
    ui.horizontal(|ui| {
        match sim.cursor {
            Some(t) => {
                ui.label(egui::RichText::new(format!("viewing step {t}")).color(theme::SELECTED).strong());
                if ui.button("Back to live").clicked() {
                    sim.cursor = None;
                    view.follow = true;
                }
            }
            None => {
                ui.label(egui::RichText::new(format!("live · step {now}")).color(theme::MUTED));
            }
        }
        ui.separator();
        if ui.button("Fit").on_hover_text("Show the whole history").clicked() {
            fit = true;
        }
        if ui.button("−").on_hover_text("Zoom out (or pinch)").clicked() {
            view.scale = (view.scale / 1.5).max(0.5);
        }
        if ui.button("+").on_hover_text("Zoom in (or pinch)").clicked() {
            view.scale = (view.scale * 1.5).min(60.0);
        }
        if ui.button("Reset").on_hover_text("Start the chip over: every gate back to unknown (X)").clicked() {
            sim.reset();
        }
        ui.label(egui::RichText::new("click or drag to scrub · double-click for live").color(theme::MUTED).small());
    });

    let Some(trace) = &sim.trace else {
        ui.add_space(8.0);
        let why = sim.error.as_deref().unwrap_or("nothing to simulate yet");
        ui.label(egui::RichText::new(format!("No timeline: {why}.")).color(theme::MUTED));
        return;
    };

    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    let wave = Rect::from_min_max(pos2(rect.min.x + LABEL_W, rect.min.y + RULER_H), rect.max);
    if fit {
        view.fit(trace, now, wave.width());
    }
    let visible = (wave.width() / view.scale) as f64;
    if view.follow && sim.cursor.is_none() {
        view.left = (now as f64 + 3.0 - visible).max(trace.start as f64);
    }

    // One lane per signal at the level on screen, in the order the chip
    // names them: inputs, outputs, then everything inside.
    let mut rows: Vec<usize> = Vec::new();
    if let Some(live) = &sim.sim {
        for signal in live.netlist().signals_at(scope) {
            if let Some(i) = trace.index_of(&signal.path)
                && !rows.contains(&i)
            {
                rows.push(i);
            }
        }
    }
    let content_h = rows.len() as f32 * LANE_H;
    let max_scroll = (content_h - wave.height()).max(0.0);
    if response.hovered() {
        let (scroll, zoom, pointer) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
        if zoom != 1.0
            && let Some(p) = pointer
        {
            let t = view.left + ((p.x - wave.min.x) / view.scale) as f64;
            view.scale = (view.scale * zoom).clamp(0.5, 60.0);
            view.left = t - ((p.x - wave.min.x) / view.scale) as f64;
            view.follow = false;
        }
        if scroll.x != 0.0 {
            view.left -= (scroll.x / view.scale) as f64;
            view.follow = false;
        }
        view.scroll = (view.scroll - scroll.y).clamp(0.0, max_scroll);
    }
    view.left = view.left.max(trace.start as f64 - 2.0);
    view.scroll = view.scroll.min(max_scroll);

    let x_of = |t: f64| wave.min.x + ((t - view.left) as f32) * view.scale;
    let t_of = |x: f32| view.left + ((x - wave.min.x) / view.scale) as f64;

    if let Some(p) = response.interact_pointer_pos()
        && (response.clicked() || response.dragged())
        && p.x >= wave.min.x
    {
        let t = t_of(p.x).round().clamp(trace.start as f64, now as f64);
        sim.cursor = if t as u64 >= now { None } else { Some(t as u64) };
    }
    if response.double_clicked() {
        sim.cursor = None;
        view.follow = true;
    }
    let trace = sim.trace.as_ref().expect("checked above");

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::BACKGROUND);
    painter.rect_filled(Rect::from_min_max(rect.min, pos2(rect.min.x + LABEL_W, rect.max.y)), 0.0, theme::PANEL);

    // Ruler with step numbers.
    let step = [1u64, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10_000]
        .into_iter()
        .find(|&s| s as f32 * view.scale >= 46.0)
        .unwrap_or(10_000);
    let first = (view.left.max(0.0) as u64).div_ceil(step) * step;
    let waves = painter.with_clip_rect(Rect::from_min_max(pos2(wave.min.x, rect.min.y), rect.max));
    let mut t = first;
    while (t as f64) <= view.left + visible {
        let x = x_of(t as f64);
        waves.line_segment([pos2(x, rect.min.y + RULER_H - 5.0), pos2(x, rect.max.y)], Stroke::new(1.0, theme::GRID));
        waves.text(
            pos2(x + 3.0, rect.min.y + 3.0),
            Align2::LEFT_TOP,
            t.to_string(),
            FontId::monospace(10.0),
            theme::MUTED,
        );
        t += step;
    }
    for &mark in &trace.marks {
        let x = x_of(mark as f64);
        let tip = pos2(x, rect.min.y + RULER_H - 2.0);
        waves.add(Shape::convex_polygon(
            vec![tip, tip + vec2(-4.0, -6.0), tip + vec2(4.0, -6.0)],
            theme::BUS,
            Stroke::NONE,
        ));
    }

    // Lanes.
    let shown = sim.cursor.unwrap_or(now);
    let lanes = painter.with_clip_rect(Rect::from_min_max(pos2(rect.min.x, wave.min.y), rect.max));
    let lane_waves = lanes.with_clip_rect(wave);
    for (row, &i) in rows.iter().enumerate() {
        let signal = &trace.signals[i];
        let top = wave.min.y + row as f32 * LANE_H - view.scroll;
        if top + LANE_H < wave.min.y || top > wave.max.y {
            continue;
        }
        let lane = Rect::from_min_size(pos2(wave.min.x, top), vec2(wave.width(), LANE_H));
        lanes.line_segment([pos2(rect.min.x, lane.max.y), pos2(rect.max.x, lane.max.y)], Stroke::new(1.0, theme::GRID));
        let value = trace.value_at(i, shown).map(format_bits).unwrap_or_default();
        lanes.text(
            pos2(rect.min.x + 8.0, lane.center().y),
            Align2::LEFT_CENTER,
            signal.path.strip_prefix(scope).unwrap_or(&signal.path),
            FontId::monospace(12.0),
            theme::TEXT,
        );
        lanes.text(
            pos2(rect.min.x + LABEL_W - 8.0, lane.center().y),
            Align2::RIGHT_CENTER,
            value,
            FontId::monospace(12.0),
            theme::MUTED,
        );

        let history = &trace.changes[i];
        let from = history.partition_point(|(t, _)| (*t as f64) <= view.left).saturating_sub(1);
        let mut previous: Option<&[Logic]> = None;
        for k in from..history.len() {
            let (start, bits) = (&history[k].0, history[k].1.as_slice());
            if *start as f64 > view.left + visible {
                break;
            }
            let end = history.get(k + 1).map_or(now.max(*start) + 1, |(t, _)| *t);
            let (x0, x1) = (x_of(*start as f64).max(wave.min.x - 4.0), x_of(end as f64).min(wave.max.x + 4.0));
            if signal.nets.len() == 1 {
                bit_segment(&lane_waves, lane, x0, x1, bits[0], previous.map(|p| p[0]));
            } else {
                bus_segment(&lane_waves, lane, x0, x1, bits);
            }
            previous = Some(bits);
        }
    }
    painter.line_segment(
        [pos2(rect.min.x + LABEL_W, rect.min.y), pos2(rect.min.x + LABEL_W, rect.max.y)],
        Stroke::new(1.0, theme::GRID),
    );

    // Where "now" is, and the scrub cursor.
    let now_x = x_of(now as f64);
    waves.line_segment(
        [pos2(now_x, wave.min.y), pos2(now_x, rect.max.y)],
        Stroke::new(1.0, theme::GATE_STROKE.gamma_multiply(0.6)),
    );
    if let Some(t) = sim.cursor {
        let x = x_of(t as f64);
        waves.line_segment([pos2(x, rect.min.y), pos2(x, rect.max.y)], Stroke::new(1.5, theme::SELECTED));
        let label = Rect::from_min_size(pos2(x - 18.0, rect.min.y + 1.0), vec2(36.0, 15.0));
        waves.rect_filled(label, 3.0, theme::SELECTED);
        waves.text(label.center(), Align2::CENTER_CENTER, t.to_string(), FontId::monospace(10.0), theme::BACKGROUND);
    }
}

/// One stretch of a single-bit signal: high for 1, low for 0, a red band
/// for X and a dashed middle line for Z.
fn bit_segment(painter: &egui::Painter, lane: Rect, x0: f32, x1: f32, bit: Logic, previous: Option<Logic>) {
    let (high, low, mid) = (lane.min.y + 6.0, lane.max.y - 6.0, lane.center().y);
    let level = |b: Logic| match b {
        Logic::One => Some(high),
        Logic::Zero => Some(low),
        _ => None,
    };
    match bit {
        Logic::One => {
            painter.rect_filled(
                Rect::from_min_max(pos2(x0, high), pos2(x1, low)),
                0.0,
                theme::ONE.gamma_multiply(0.12),
            );
            painter.line_segment([pos2(x0, high), pos2(x1, high)], Stroke::new(2.0, theme::ONE));
        }
        Logic::Zero => {
            painter.line_segment([pos2(x0, low), pos2(x1, low)], Stroke::new(2.0, theme::GATE_STROKE));
        }
        Logic::X => {
            painter.rect_filled(
                Rect::from_min_max(pos2(x0, high), pos2(x1, low)),
                0.0,
                theme::UNKNOWN.gamma_multiply(0.25),
            );
            painter.line_segment([pos2(x0, high), pos2(x1, high)], Stroke::new(1.0, theme::UNKNOWN));
            painter.line_segment([pos2(x0, low), pos2(x1, low)], Stroke::new(1.0, theme::UNKNOWN));
        }
        Logic::Z => painter.extend(Shape::dashed_line(
            &[pos2(x0, mid), pos2(x1, mid)],
            Stroke::new(1.5, theme::FLOATING),
            4.0,
            3.0,
        )),
    }
    if let (Some(from), Some(to)) = (previous.and_then(level), level(bit)) {
        let color = if bit == Logic::One { theme::ONE } else { theme::GATE_STROKE };
        painter.line_segment([pos2(x0, from), pos2(x0, to)], Stroke::new(2.0, color));
    }
}

/// One stretch of a bus: a long hexagon with the value inside, like a
/// waveform viewer draws it.
fn bus_segment(painter: &egui::Painter, lane: Rect, x0: f32, x1: f32, bits: &[Logic]) {
    let (top, bottom, mid) = (lane.min.y + 5.0, lane.max.y - 5.0, lane.center().y);
    let slant = ((x1 - x0) / 2.0).min(4.0);
    let color = if bits.contains(&Logic::X) {
        theme::UNKNOWN
    } else if bits.iter().all(|&b| b == Logic::Z) {
        theme::FLOATING
    } else {
        theme::BUS
    };
    let outline = vec![
        pos2(x0, mid),
        pos2(x0 + slant, top),
        pos2(x1 - slant, top),
        pos2(x1, mid),
        pos2(x1 - slant, bottom),
        pos2(x0 + slant, bottom),
    ];
    painter.add(Shape::convex_polygon(outline, color.gamma_multiply(0.12), Stroke::new(1.5, color)));
    let text = format_bits(bits);
    let fits = (x1 - x0) > text.len() as f32 * 7.0 + 10.0;
    if fits {
        let center = pos2(((x0 + x1) / 2.0).max(lane.min.x + text.len() as f32 * 3.6 + 6.0), mid);
        painter.text(center, Align2::CENTER_CENTER, text, FontId::monospace(11.0), color);
    }
}
