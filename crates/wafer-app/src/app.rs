//! The window: a top bar with simulation controls, Blender-style docked
//! panels around the canvas, and a status bar.

use std::path::PathBuf;
use std::time::Duration;

use egui::{Key, KeyboardShortcut, Modifiers, RichText};
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};

use crate::canvas::Canvas;
use crate::code::CodePanel;
use crate::simrun::{SimRunner, TestResults};
use crate::timeline::{self, TimelineView};
use crate::workspace::Workspace;
use crate::{panels, theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Chips,
    Canvas,
    Inspector,
    Tests,
    Problems,
    Timeline,
    Code,
}

/// Everything except the dock layout, so panels can borrow it freely.
pub struct AppState {
    pub ws: Workspace,
    pub canvas: Canvas,
    pub sim: SimRunner,
    pub tests: Option<TestResults>,
    pub new_chip: String,
    /// The inspector's name field: (node key it belongs to, text).
    pub rename: (String, String),
    pub timeline: TimelineView,
    pub code: CodePanel,
}

pub struct WaferApp {
    pub state: AppState,
    dock: DockState<Tab>,
}

impl WaferApp {
    pub fn new(cc: &eframe::CreationContext<'_>, dir: PathBuf) -> WaferApp {
        theme::apply(&cc.egui_ctx);
        WaferApp::open(dir)
    }

    pub fn open(dir: PathBuf) -> WaferApp {
        let ws = Workspace::open(dir);
        // Start on the most built-up chip.
        let chip = ws.chip_names().last().cloned().unwrap_or_default();
        let state = AppState {
            canvas: Canvas::new(chip),
            sim: SimRunner::new(),
            tests: None,
            new_chip: String::new(),
            rename: (String::new(), String::new()),
            timeline: TimelineView::default(),
            code: CodePanel::default(),
            ws,
        };
        WaferApp { state, dock: default_dock() }
    }
}

impl WaferApp {
    /// Brings a panel's tab to the front.
    pub fn show_tab(&mut self, tab: Tab) {
        if let Some(path) = self.dock.find_tab(&tab) {
            let _ = self.dock.set_active_tab(path);
        }
    }
}

fn default_dock() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Canvas]);
    let surface = dock.main_surface_mut();
    let [center, _] = surface.split_left(NodeIndex::root(), 0.16, vec![Tab::Chips]);
    let [center, _] = surface.split_right(center, 0.72, vec![Tab::Inspector, Tab::Code]);
    surface.split_below(center, 0.68, vec![Tab::Timeline, Tab::Tests, Tab::Problems]);
    dock
}

impl AppState {
    pub fn open_chip(&mut self, chip: &str) {
        self.code.flush(&mut self.ws);
        self.canvas.open(chip);
        self.tests = None;
    }

    /// Per-frame housekeeping: notice file changes, keep the simulation and
    /// tests in step with them, and handle app-wide shortcuts.
    pub fn tick(&mut self, ctx: &egui::Context) {
        self.ws.poll();
        if !self.ws.design.chips.contains_key(&self.canvas.root)
            && let Some(first) = self.ws.chip_names().last().cloned()
        {
            self.open_chip(&first);
        }
        self.canvas.validate(&self.ws);
        // The top-level chip is simulated; diving in just looks inside it.
        let root = self.canvas.root.clone();
        self.sim.sync(&self.ws, &root);
        let chip = self.canvas.chip.clone();
        let stale_tests = self.tests.as_ref().is_none_or(|t| t.chip != chip || t.generation != self.ws.generation);
        if stale_tests {
            self.tests = Some(TestResults::run(&self.ws, &chip));
        }

        // App-wide shortcuts, unless you're typing in a text field. (The
        // canvas holding keyboard focus doesn't count as typing.)
        let typing = ctx.memory(|m| m.focused()).is_some_and(|f| Some(f) != self.canvas.focus_id);
        if !typing {
            let redo = ctx.input_mut(|i| {
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z))
            });
            let undo = ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z)));
            let (space, right) = ctx.input(|i| {
                (i.key_pressed(Key::Space) && !i.modifiers.any(), i.key_pressed(Key::ArrowRight) && !i.modifiers.any())
            });
            if redo {
                self.ws.redo();
            } else if undo {
                self.ws.undo();
            }
            if space {
                self.sim.playing = !self.sim.playing;
            }
            if right {
                self.sim.step();
            }
        }

        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        if self.sim.advance(dt) {
            ctx.request_repaint();
        }
        // Keep checking for edits made outside the app.
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Wafer").strong().color(theme::ONE));
            ui.label(RichText::new(&self.canvas.chip).monospace().color(theme::TEXT));
            ui.separator();
            let label = if self.sim.playing { "Pause" } else { "Play" };
            if ui.button(label).on_hover_text("Space").clicked() {
                self.sim.playing = !self.sim.playing;
            }
            if ui.button("Step").on_hover_text("One gate delay (right arrow key)").clicked() {
                self.sim.step();
            }
            ui.add_enabled(
                !self.sim.instant,
                egui::Slider::new(&mut self.sim.speed, 1.0..=1000.0).logarithmic(true).suffix(" delays/s"),
            );
            ui.checkbox(&mut self.sim.instant, "Instant");
            let color = if self.sim.error.is_some() {
                theme::UNKNOWN
            } else if self.sim.is_settled() {
                theme::MUTED
            } else {
                theme::ONE
            };
            ui.label(RichText::new(self.sim.status()).color(color));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(self.ws.can_redo(), egui::Button::new("Redo")).on_hover_text("Cmd+Shift+Z").clicked()
                {
                    self.ws.redo();
                }
                if ui.add_enabled(self.ws.can_undo(), egui::Button::new("Undo")).on_hover_text("Cmd+Z").clicked() {
                    self.ws.undo();
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| match (&self.ws.status, &self.ws.stale) {
            (Some((message, true)), _) => {
                ui.label(RichText::new(message).color(theme::UNKNOWN));
            }
            (_, Some(stale)) => {
                ui.label(RichText::new(format!("{stale}. Editing is paused until it's fixed.")).color(theme::WARNING));
            }
            (Some((message, false)), _) => {
                ui.label(RichText::new(message).color(theme::MUTED));
            }
            _ => {
                let errors = self.ws.problems.iter().filter(|p| p.error).count();
                let hint = "Shift+A add · drag from a pin to wire · click an input to flip it · X delete · F frame";
                if errors > 0 {
                    ui.label(
                        RichText::new(format!("{errors} error{} in Problems", if errors == 1 { "" } else { "s" }))
                            .color(theme::UNKNOWN),
                    );
                    ui.separator();
                }
                ui.label(RichText::new(hint).color(theme::MUTED));
            }
        });
    }
}

struct Tabs<'a> {
    state: &'a mut AppState,
}

impl TabViewer for Tabs<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(*tab)
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        let errors = self.state.ws.problems.iter().filter(|p| p.error).count();
        match tab {
            Tab::Chips => "Chips".into(),
            Tab::Canvas => "Canvas".into(),
            Tab::Inspector => "Inspector".into(),
            Tab::Tests => match &self.state.tests {
                Some(t) if !t.is_empty() && !t.passed() => RichText::new("Tests ✖").color(theme::UNKNOWN).into(),
                Some(t) if !t.is_empty() => "Tests ✔".into(),
                _ => "Tests".into(),
            },
            Tab::Problems if errors > 0 => RichText::new(format!("Problems ({errors})")).color(theme::UNKNOWN).into(),
            Tab::Problems => "Problems".into(),
            Tab::Timeline => "Timeline".into(),
            Tab::Code => "Code".into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        let state = &mut *self.state;
        match tab {
            Tab::Chips => panels::chips(ui, state),
            Tab::Canvas => state.canvas.ui(ui, &mut state.ws, &mut state.sim),
            Tab::Inspector => panels::inspector(ui, state),
            Tab::Tests => panels::tests(ui, state),
            Tab::Problems => panels::problems(ui, state),
            Tab::Timeline => timeline::ui(ui, &mut state.timeline, &mut state.sim, &state.canvas.prefix),
            Tab::Code => state.code.ui(ui, &mut state.ws, &mut state.canvas),
        }
    }

    fn is_closeable(&self, _tab: &Tab) -> bool {
        false
    }

    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        [false, false]
    }
}

impl eframe::App for WaferApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.state.tick(&ctx);
        egui::Panel::top("top_bar").show(ui, |ui| self.state.top_bar(ui));
        egui::Panel::bottom("status_bar").show(ui, |ui| self.state.status_bar(ui));
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(theme::PANEL)).show(ui, |ui| {
            let style = Style::from_egui(ui.style());
            DockArea::new(&mut self.dock)
                .style(style)
                .show_close_buttons(false)
                .show_add_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_leaf_collapse_buttons(false)
                .show_inside(ui, &mut Tabs { state: &mut self.state });
        });
    }
}

impl AppState {
    /// A signal's value in the chip on screen (inside the box you've dived
    /// into, if you have).
    pub fn value(&self, name: &str) -> Option<Vec<wafer_core::Logic>> {
        self.sim.value(&format!("{}{name}", self.canvas.prefix))
    }

    /// Keys of every box on the open chip's canvas (for tests and tools).
    pub fn canvas_keys(&self) -> Vec<String> {
        wafer_core::schematic::layout(&self.ws.design, &self.ws.project.sources, &self.canvas.chip)
            .map(|s| s.nodes.into_iter().map(|n| n.key).collect())
            .unwrap_or_default()
    }
}
