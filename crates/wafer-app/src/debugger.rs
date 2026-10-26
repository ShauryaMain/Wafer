//! The Debugger tab: the program the console is playing, as its `.asm`
//! source, with the line about to run highlighted and breakpoints set by
//! clicking lines; Crumb's registers and stacks, read from the gates; and
//! the program's variables, read from RAM. Step runs one instruction, Step
//! frame runs to the next frame, and Continue plays until a breakpoint.

use std::collections::HashMap;

use crumb::asm::SymbolKind;
use egui::{Align, RichText, Sense};

use crate::console::Player;
use crate::simrun::SimRunner;
use crate::theme;

/// A program's source, and which instruction came from which line.
#[derive(Clone, Debug)]
pub struct Source {
    pub name: String,
    pub text: String,
    pub program: crumb::Program,
    /// The line each ROM word came from (`None` for data, or the jump to
    /// `main` the assembler adds).
    line_of: Vec<Option<usize>>,
    /// The first instruction each line made: where a breakpoint on it goes.
    first_on: HashMap<usize, u16>,
}

impl Source {
    pub fn new(name: String, text: String, program: crumb::Program) -> Source {
        let line_at = |offset: usize| text[..offset.min(text.len())].matches('\n').count();
        let line_of: Vec<Option<usize>> = (0..program.words.len())
            .map(|addr| {
                let (start, end) = program.origins.get(addr).copied().unwrap_or((0, 0));
                (end > start && !program.is_data(addr as u16)).then(|| line_at(start))
            })
            .collect();
        let mut first_on = HashMap::new();
        for (addr, line) in line_of.iter().enumerate() {
            if let Some(line) = line {
                first_on.entry(*line).or_insert(addr as u16);
            }
        }
        Source { name, text, program, line_of, first_on }
    }

    /// The line (counting from 0) the instruction at `addr` came from.
    pub fn line_of(&self, addr: u16) -> Option<usize> {
        self.line_of.get(usize::from(addr)).copied().flatten()
    }

    /// Where a breakpoint on `line` goes: its first instruction.
    pub fn address_of(&self, line: usize) -> Option<u16> {
        self.first_on.get(&line).copied()
    }

    /// The word (`: name`) that `addr` is inside.
    pub fn word_at(&self, addr: u16) -> Option<&str> {
        (self.program.symbols.iter())
            .filter(|s| s.kind == SymbolKind::Word && s.value <= addr)
            .max_by_key(|s| s.value)
            .map(|s| s.name.as_str())
    }

    /// The program's variables: (name, RAM address, how many words).
    pub fn variables(&self) -> Vec<(String, u16, usize)> {
        let mut vars: Vec<(String, u16)> = (self.program.symbols.iter())
            .filter(|s| s.kind == SymbolKind::Var)
            .map(|s| (s.name.clone(), s.value))
            .collect();
        vars.sort_by_key(|(_, at)| *at);
        let end = crumb::isa::RAM_START + self.program.ram_used as u16;
        (0..vars.len())
            .map(|i| {
                let next = vars.get(i + 1).map_or(end, |(_, at)| *at);
                (vars[i].0.clone(), vars[i].1, usize::from(next.saturating_sub(vars[i].1)).max(1))
            })
            .collect()
    }
}

/// Shown for a word: hex, or ???? while any bit is unknown.
fn hex(word: Option<u16>) -> String {
    word.map_or_else(|| "????".into(), |w| format!("{w:04X}"))
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::MUTED).small().strong());
}

/// The tab.
pub fn ui(ui: &mut egui::Ui, state: &mut crate::app::AppState) {
    let crate::app::AppState { console: player, sim, .. } = state;
    if !player.can_play() {
        ui.add_space(8.0);
        ui.label(
            RichText::new("Open a game console, like the Console chip, to debug what it's playing.")
                .color(theme::MUTED),
        );
        return;
    }
    toolbar(ui, player, sim);
    ui.separator();
    let height = ui.available_height();
    let width = ui.available_width();
    // Three columns, each laid out top to bottom.
    let column = egui::Layout::top_down(Align::Min);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(width * 0.55, height), column, |ui| source_view(ui, player, sim));
        ui.separator();
        ui.allocate_ui_with_layout(egui::vec2(width * 0.2, height), column, |ui| registers_view(ui, player, sim));
        ui.separator();
        let rest = egui::vec2(ui.available_width(), height);
        ui.allocate_ui_with_layout(rest, column, |ui| variables_view(ui, player, sim));
    });
}

/// Where it's stopped, said in terms of the source.
fn whereabouts(player: &Player, sim: &SimRunner) -> String {
    let pc = player.registers(sim).and_then(|r| r.pc);
    let Some(pc) = pc.filter(|_| player.between_instructions(sim)) else {
        return "At the start: Step to fetch the first instruction.".into();
    };
    let place = match &player.source {
        Some(source) => {
            let code = source.program.words.get(usize::from(pc)).copied().unwrap_or(0);
            let what =
                format!("`{}`", crumb::isa::disasm_with(code, |a| source.program.name_at(a).map(str::to_string)));
            let line = source.line_of(pc).map_or_else(String::new, |l| format!("line {}, ", l + 1));
            let inside = source.word_at(pc).map_or_else(String::new, |w| format!(" in {w}"));
            format!("{line}{what}{inside}")
        }
        None => "the next instruction".to_string(),
    };
    let why = if player.hit { "Stopped at a breakpoint" } else { "Stopped" };
    format!("{why}: about to run {place} (PC {pc:04X}).")
}

fn toolbar(ui: &mut egui::Ui, player: &mut Player, sim: &mut SimRunner) {
    ui.horizontal(|ui| {
        let label = if player.running { "Pause" } else { "Continue" };
        if ui.button(label).on_hover_text("Play until a breakpoint").clicked() {
            let running = !player.running;
            player.set_running(running, sim);
        }
        if ui.add_enabled(!player.running, egui::Button::new("Step")).on_hover_text("Run one instruction").clicked() {
            player.step(sim);
        }
        if ui
            .add_enabled(!player.running, egui::Button::new("Step frame"))
            .on_hover_text("Run to the next frame")
            .clicked()
        {
            player.step_frame(sim);
        }
        let any = !player.breakpoints.is_empty();
        if ui.add_enabled(any, egui::Button::new("Clear breakpoints")).clicked() {
            player.breakpoints.clear();
        }
        ui.separator();
        let status = if player.running {
            let mode = if player.in_turbo() { " in turbo" } else { "" };
            RichText::new(format!("Running{mode}…")).color(theme::ONE)
        } else {
            RichText::new(whereabouts(player, sim)).color(if player.hit { theme::WARNING } else { theme::TEXT })
        };
        ui.label(status);
    });
}

fn source_view(ui: &mut egui::Ui, player: &mut Player, sim: &SimRunner) {
    let Some(source) = player.source.clone() else {
        heading(ui, "PROGRAM");
        ui.label(RichText::new("This program has no source to show (a .hex file).").color(theme::MUTED));
        return;
    };
    heading(ui, &source.name.to_uppercase());
    let pc = player.registers(sim).and_then(|r| r.pc);
    let current = pc.filter(|_| !player.running && player.between_instructions(sim)).and_then(|pc| source.line_of(pc));
    // Scroll to the line once each time it changes.
    let seen = egui::Id::new("debugger-scrolled-to");
    let scroll = ui.data(|d| d.get_temp::<Option<usize>>(seen)) != Some(current);
    egui::ScrollArea::vertical().id_salt("debugger-source").auto_shrink([false, false]).show(ui, |ui| {
        for (i, text) in source.text.lines().enumerate() {
            let address = source.address_of(i);
            let row = ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (rect, response) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::click());
                if let Some(address) = address {
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            format!("breakpoint on line {}", i + 1),
                        )
                    });
                    let on = player.breakpoints.contains(&address);
                    if on {
                        ui.painter().circle_filled(rect.center(), 5.0, theme::UNKNOWN);
                    } else if response.hovered() {
                        ui.painter().circle_stroke(rect.center(), 5.0, egui::Stroke::new(1.0, theme::UNKNOWN));
                    }
                    // A toggle: only in a frame's first pass, since egui
                    // may run the frame again with the same click.
                    let first_pass = ui.ctx().current_pass_index() == 0;
                    if response.clicked() && first_pass && !player.breakpoints.remove(&address) {
                        player.breakpoints.insert(address);
                    }
                }
                ui.label(RichText::new(format!("{:>3}", i + 1)).monospace().color(theme::MUTED));
                let line = RichText::new(if text.is_empty() { " " } else { text }).monospace();
                let line = if current == Some(i) {
                    line.color(theme::BACKGROUND).background_color(theme::ONE)
                } else if address.is_some() {
                    line.color(theme::TEXT)
                } else {
                    line.color(theme::MUTED)
                };
                ui.label(line);
            });
            if scroll && current == Some(i) {
                ui.scroll_to_rect(row.response.rect, Some(Align::Center));
            }
        }
    });
    if scroll {
        ui.data_mut(|d| d.insert_temp(seen, current));
    }
}

fn registers_view(ui: &mut egui::Ui, player: &Player, sim: &SimRunner) {
    heading(ui, "REGISTERS");
    let Some(registers) = player.registers(sim) else { return };
    egui::Grid::new("debugger-registers").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
        for (name, value) in [("PC", registers.pc), ("T", registers.t), ("N", registers.n), ("R", registers.r)] {
            ui.label(RichText::new(name).monospace().color(theme::MUTED));
            let decimal = value.map_or_else(String::new, |v| format!(" ({v})"));
            ui.label(RichText::new(format!("{}{decimal}", hex(value))).monospace().color(theme::BUS));
            ui.end_row();
        }
    });
    ui.add_space(6.0);
    heading(ui, "DATA STACK (TOP LAST)");
    let words = |items: &[Option<u16>]| {
        if items.is_empty() { "empty".to_string() } else { items.iter().map(|&w| hex(w)).collect::<Vec<_>>().join(" ") }
    };
    ui.label(RichText::new(words(&registers.stack)).monospace().color(theme::TEXT));
    ui.add_space(4.0);
    heading(ui, "RETURN STACK (TOP LAST)");
    ui.label(RichText::new(words(&registers.returns)).monospace().color(theme::TEXT));
}

fn variables_view(ui: &mut egui::Ui, player: &Player, sim: &SimRunner) {
    heading(ui, "VARIABLES");
    let Some(source) = &player.source else { return };
    let vars = source.variables();
    if vars.is_empty() {
        ui.label(RichText::new("This program has no vars.").color(theme::MUTED));
        return;
    }
    egui::ScrollArea::vertical().id_salt("debugger-vars").show(ui, |ui| {
        egui::Grid::new("debugger-vars-grid").num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
            for (name, at, count) in vars {
                ui.label(RichText::new(name).monospace().color(theme::MUTED));
                let words = player.ram_words(sim, at, count.min(8));
                let mut text = words.iter().map(|&w| hex(w)).collect::<Vec<_>>().join(" ");
                if let [Some(w)] = words[..] {
                    text = format!("{text} ({})", w as i16);
                }
                if count > 8 {
                    text.push_str(&format!(" … ({count} words)"));
                }
                ui.label(RichText::new(text).monospace().color(theme::BUS));
                ui.end_row();
            }
        });
    });
}
