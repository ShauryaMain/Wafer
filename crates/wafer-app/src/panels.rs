//! The side and bottom panels: chips, inspector, tests, problems and the
//! timeline placeholder.

use egui::{Color32, RichText};
use wafer_core::check::Callee;
use wafer_core::edit::Edit;
use wafer_core::logic::{Logic, bits_from_u64, bits_to_u64, format_bits};
use wafer_core::schematic::NodeKind;

use crate::app::AppState;
use crate::canvas::sink_of;
use crate::simrun::TestResults;
use crate::theme;

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::MUTED).small().strong());
}

fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).monospace()
}

pub fn chips(ui: &mut egui::Ui, state: &mut AppState) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        heading(ui, "CHIPS");
        let current = state.canvas.root.clone();
        for name in state.ws.chip_names() {
            let ok = state.ws.design.is_buildable(&name);
            let mut text = RichText::new(&name).monospace();
            if !ok {
                text = text.color(theme::UNKNOWN);
            }
            let response = ui.selectable_label(name == current, text);
            if response.clicked() {
                state.open_chip(&name);
            }
            if !ok {
                response.on_hover_text("This chip (or one inside it) has errors. See Problems.");
            }
        }

        // The open chip's insides, as a tree: click a level to look inside it.
        let root = state.canvas.root.clone();
        if has_chips_inside(&state.ws.design, &root) {
            ui.add_space(12.0);
            heading(ui, &format!("INSIDE {}", root.to_uppercase()));
            let mut go: Option<Vec<(String, String)>> = None;
            if ui.selectable_label(state.canvas.path.is_empty(), mono(&root)).clicked() {
                go = Some(Vec::new());
            }
            tree(ui, &state.ws.design, &root, &mut Vec::new(), &state.canvas.path, &mut go);
            if let Some(path) = go {
                state.canvas.set_path(path);
            }
        }

        ui.add_space(12.0);
        heading(ui, "NEW CHIP");
        ui.horizontal(|ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut state.new_chip).hint_text("Name").desired_width(110.0));
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Create").clicked() || enter) && !state.new_chip.trim().is_empty() {
                let name = state.new_chip.trim().to_string();
                if state.ws.new_chip(&name) {
                    state.new_chip.clear();
                    state.open_chip(&name);
                }
            }
        });
    });
}

fn has_chips_inside(design: &wafer_core::Design, chip: &str) -> bool {
    design.chips.get(chip).is_some_and(|info| info.instances.iter().any(|i| matches!(i.callee, Callee::Chip { .. })))
}

/// One level of the hierarchy tree. Gates are left out; chips can be
/// expanded and clicked.
fn tree(
    ui: &mut egui::Ui,
    design: &wafer_core::Design,
    chip: &str,
    path: &mut Vec<(String, String)>,
    current: &[(String, String)],
    go: &mut Option<Vec<(String, String)>>,
) {
    let Some(info) = design.chips.get(chip) else { return };
    if path.len() > 12 {
        return;
    }
    for inst in &info.instances {
        let Callee::Chip { name: child, .. } = &inst.callee else { continue };
        path.push((inst.name.clone(), child.clone()));
        let here = path.as_slice() == current;
        let label = mono(format!("{} · {child}", inst.name));
        if has_chips_inside(design, child) {
            let id = ui.make_persistent_id(path.iter().map(|(i, _)| i.as_str()).collect::<Vec<_>>().join("."));
            let open = current.starts_with(path);
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, open)
                .show_header(ui, |ui| {
                    if ui.selectable_label(here, label).clicked() {
                        *go = Some(path.clone());
                    }
                })
                .body(|ui| tree(ui, design, child, path, current, go));
        } else {
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                if ui.selectable_label(here, label).clicked() {
                    *go = Some(path.clone());
                }
            });
        }
        path.pop();
    }
}

pub fn inspector(ui: &mut egui::Ui, state: &mut AppState) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        let Some(s) = state.canvas.schematic(&state.ws).cloned() else {
            ui.label("No chip open.");
            return;
        };
        let selected: Vec<usize> = s
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| state.canvas.selection.contains(&n.key))
            .map(|(i, _)| i)
            .collect();

        if let [i] = selected.as_slice() {
            node_inspector(ui, state, &s, *i);
        } else if selected.len() > 1 {
            heading(ui, "SELECTION");
            ui.label(format!("{} boxes selected", selected.len()));
            ui.label(RichText::new("G to move, X to delete").color(theme::MUTED));
        } else if let Some(id) = state.canvas.selected_wire.clone()
            && let Some(wire) = s.wires.iter().find(|w| s.nodes[w.source.0].key == id.node && w.source.1 == id.pin)
        {
            heading(ui, "WIRE");
            ui.label(mono(&wire.signal));
            let value = state.value(&wire.signal).map(|v| format_bits(&v)).unwrap_or_else(|| "?".into());
            ui.label(format!("value {value} · {} bit{}", wire.width, if wire.width == 1 { "" } else { "s" }));
            ui.add_space(6.0);
            heading(ui, "FEEDS");
            for &(n, p) in &wire.sinks {
                let node = &s.nodes[n];
                let pin = node.inputs.get(p).map(|p| p.name.as_str()).unwrap_or("");
                let label = match &node.kind {
                    NodeKind::Output { name, .. } => format!("output {name}"),
                    NodeKind::Gate { name, .. } | NodeKind::Chip { name, .. } => format!("{name}.{pin}"),
                    NodeKind::Input { .. } => continue,
                };
                ui.label(mono(label));
            }
            ui.add_space(6.0);
            if ui.button("Delete wire").clicked() {
                state.canvas.delete_selection(&s, &mut state.ws);
            }
        } else {
            chip_overview(ui, state);
        }
    });
}

fn node_inspector(ui: &mut egui::Ui, state: &mut AppState, s: &wafer_core::schematic::Schematic, i: usize) {
    let node = &s.nodes[i];
    let (kind, name) = match &node.kind {
        NodeKind::Input { name, width } => {
            (if *width == 1 { "Input".to_string() } else { format!("Input · {width} bits") }, name.clone())
        }
        NodeKind::Output { name, width } => {
            (if *width == 1 { "Output".to_string() } else { format!("Output · {width} bits") }, name.clone())
        }
        NodeKind::Gate { name, kind, .. } => (format!("{} · {}", kind.name(), kind.describe()), name.clone()),
        NodeKind::Chip { name, chip, copies: 1 } => (format!("{chip} chip"), name.clone()),
        NodeKind::Chip { name, chip, copies } => {
            (format!("{chip} chip · {copies} side by side, one per bit"), name.clone())
        }
    };
    heading(ui, &kind.to_uppercase());

    // Rename: edits the name everywhere in the chip.
    if state.rename.0 != node.key {
        state.rename = (node.key.clone(), name.clone());
    }
    ui.horizontal(|ui| {
        ui.label("Name");
        let field = ui
            .add(egui::TextEdit::singleline(&mut state.rename.1).font(egui::TextStyle::Monospace).desired_width(130.0));
        if field.lost_focus() && state.rename.1 != name {
            let new = state.rename.1.trim().to_string();
            let chip = state.canvas.chip.clone();
            if state.ws.edit(&chip, &[Edit::Rename { old: name.clone(), new: new.clone() }], &[]) {
                let key = match &node.kind {
                    NodeKind::Input { .. } => format!("in:{new}"),
                    NodeKind::Output { .. } => format!("out:{new}"),
                    _ => new.clone(),
                };
                state.canvas.selection = [key.clone()].into();
                state.rename = (key, new);
            } else {
                state.rename.1 = name.clone();
            }
        }
    });

    if let NodeKind::Gate { name, kind: wafer_core::GateKind::Ram | wafer_core::GateKind::Rom, .. } = &node.kind {
        ui.add_space(6.0);
        memory_view(ui, state, name);
    }

    if let NodeKind::Input { name, width } = &node.kind {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Value");
            input_editor(ui, state, name, *width);
        });
    }

    ui.add_space(8.0);
    if !node.inputs.is_empty() && !matches!(node.kind, NodeKind::Output { .. }) {
        heading(ui, "INPUTS");
        for (p, pin) in node.inputs.iter().enumerate() {
            let feed = s.wires.iter().find(|w| w.sinks.contains(&(i, p)));
            let text = match feed {
                Some(w) => {
                    let value = state.value(&w.signal).map(|v| format_bits(&v)).unwrap_or_else(|| "?".into());
                    format!("{:<6} ← {} = {value}", pin.name, w.signal)
                }
                None if s.tags.iter().any(|t| (t.x + 2.0 - pin.x).abs() < 0.5 && (t.y - pin.y).abs() < 0.5) => {
                    format!("{:<6} ← constant", pin.name)
                }
                None => format!("{:<6} ← nothing (reads Z)", pin.name),
            };
            ui.horizontal(|ui| {
                ui.label(mono(text));
                if feed.is_some()
                    && ui.small_button("✕").on_hover_text("Disconnect").clicked()
                    && let Some(sink) = sink_of(s, i, p)
                {
                    let chip = state.canvas.chip.clone();
                    state.ws.edit(&chip, &[Edit::Disconnect { sink }], &[]);
                }
            });
        }
    }
    if let NodeKind::Output { name, .. } = &node.kind {
        let value = state.value(name).map(|v| format_bits(&v)).unwrap_or_else(|| "?".into());
        ui.label(mono(format!("value {value}")));
    }
    if !node.outputs.is_empty() && !matches!(node.kind, NodeKind::Input { .. }) {
        heading(ui, "OUTPUTS");
        for (p, pin) in node.outputs.iter().enumerate() {
            let wire = s.wires.iter().find(|w| w.source == (i, p));
            let value =
                wire.and_then(|w| state.value(&w.signal)).map(|v| format_bits(&v)).unwrap_or_else(|| "?".into());
            ui.label(mono(format!("{:<6} → {value}", pin.name)));
        }
    }
    ui.add_space(10.0);
    if ui.button("Delete").clicked() {
        state.canvas.delete_selection(s, &mut state.ws);
    }
}

/// A Ram's contents, live, with the word at its current address highlighted.
fn memory_view(ui: &mut egui::Ui, state: &AppState, name: &str) {
    heading(ui, "MEMORY");
    let path = format!("{}{name}", state.canvas.prefix);
    let Some(sim) = &state.sim.sim else { return };
    let Some(index) = sim.netlist().rams.iter().position(|r| r.path == path) else { return };
    let ram = &sim.netlist().rams[index];
    let width = ram.data.len();
    let current = bits_to_u64(&sim.values(&ram.addr));
    let words: Vec<&[Logic]> = sim.memory(index).chunks(width).collect();
    let digits = format!("{:X}", words.len().saturating_sub(1)).len();
    if ram.rom.is_some() {
        rom_view(ui, state, name, words.len(), current);
        return;
    }
    ui.label(RichText::new(format!("{} words of {width} bits (live)", words.len())).color(theme::MUTED).small());
    egui::ScrollArea::vertical().id_salt("memory").max_height(240.0).show(ui, |ui| {
        for (row, chunk) in words.chunks(4).enumerate().take(1024) {
            ui.horizontal(|ui| {
                ui.label(mono(format!("{:0digits$X}:", row * 4)).color(theme::MUTED));
                for (i, word) in chunk.iter().enumerate() {
                    let here = current == Some((row * 4 + i) as u64);
                    let text = mono(format_bits(word));
                    ui.label(if here {
                        text.color(theme::BACKGROUND).background_color(theme::BUS)
                    } else {
                        text.color(theme::TEXT)
                    });
                }
            });
        }
    });
}

/// A Rom's program, one word per line, disassembled when it came from a
/// `.asm` file.
fn rom_view(ui: &mut egui::Ui, state: &AppState, name: &str, size: usize, current: Option<u64>) {
    let design = &state.ws.design;
    let program = design
        .chips
        .get(&state.canvas.chip)
        .and_then(|chip| chip.instances.iter().find(|i| i.name == name))
        .and_then(|inst| inst.program.as_ref())
        .and_then(|p| Some((p.name.clone(), design.programs.get(&p.path)?)));
    let Some((file, program)) = program else { return };
    let used = program.words.len();
    ui.label(RichText::new(format!("{file}: {used} of {size} words (read-only)")).color(theme::MUTED).small());
    let assembled = program.assembled.as_ref();
    egui::ScrollArea::vertical().id_salt("rom").max_height(240.0).show(ui, |ui| {
        for (addr, &word) in program.words.iter().enumerate().take(4096) {
            if let Some(label) = assembled.and_then(|p| p.name_at(addr as u16)) {
                ui.label(mono(format!("{label}:")).color(theme::BUS));
            }
            let text = assembled.map(|p| p.describe(addr as u16)).unwrap_or_default();
            let line = mono(format!("{addr:04X}  {word:04X}  {text}"));
            ui.label(if current == Some(addr as u64) {
                line.color(theme::BACKGROUND).background_color(theme::BUS)
            } else {
                line.color(theme::TEXT)
            });
        }
    });
}

fn input_editor(ui: &mut egui::Ui, state: &mut AppState, name: &str, width: usize) {
    if !state.canvas.prefix.is_empty() {
        let value = state.value(name).map(|v| format_bits(&v)).unwrap_or_else(|| "?".into());
        ui.label(mono(value)).on_hover_text(format!("Driven from outside; change it on {}", state.canvas.root));
        return;
    }
    let current = state.sim.read(name);
    ui.horizontal(|ui| {
        if width == 1 {
            let on = current.as_deref() == Some(&[Logic::One]);
            if ui
                .selectable_label(on, if on { " 1 " } else { " 0 " })
                .on_hover_text("Click to flip (or click the input on the canvas)")
                .clicked()
            {
                state.sim.toggle(name);
            }
        } else {
            let mut value = current.as_deref().and_then(bits_to_u64).unwrap_or(0);
            let max = if width >= 64 { u64::MAX } else { (1u64 << width) - 1 };
            let digits = width.div_ceil(4);
            let drag =
                ui.add(egui::DragValue::new(&mut value).range(0..=max).hexadecimal(digits, false, true).prefix("0x"));
            if drag.changed() {
                state.sim.set_input(name, bits_from_u64(value, width));
            }
            ui.label(
                mono(format!("= {}", current.as_deref().and_then(bits_to_u64).map_or("?".into(), |v| v.to_string())))
                    .color(theme::MUTED),
            );
        }
        if ui.small_button("X").on_hover_text("Set to unknown").clicked() {
            state.sim.set_input(name, vec![Logic::X; width]);
        }
    });
}

fn chip_overview(ui: &mut egui::Ui, state: &mut AppState) {
    let chip = state.canvas.chip.clone();
    let Some(info) = state.ws.design.chips.get(&chip).cloned() else { return };
    heading(ui, "CHIP");
    ui.label(RichText::new(&chip).monospace().size(16.0).color(theme::TEXT));
    if let Some(path) = state.ws.chip_file(&chip) {
        let shown = path.strip_prefix(&state.ws.dir).unwrap_or(&path).display().to_string();
        ui.label(RichText::new(shown).color(theme::MUTED).small());
    }
    ui.label(format!("{} box{}", info.instances.len(), if info.instances.len() == 1 { "" } else { "es" }));
    ui.add_space(8.0);
    heading(ui, "INPUTS");
    for port in &info.inputs {
        ui.horizontal(|ui| {
            ui.label(mono(format!("{:<6}", port.name)));
            input_editor(ui, state, &port.name, port.width);
        });
    }
    ui.add_space(4.0);
    heading(ui, "OUTPUTS");
    for port in &info.outputs {
        let value = state.value(&port.name).map(|v| format_bits(&v)).unwrap_or_else(|| "?".into());
        ui.label(mono(format!("{:<6} {value}", port.name)));
    }
    ui.add_space(12.0);
    heading(ui, "KEYS");
    for (key, what) in [
        ("Shift+A", "add a gate, chip or port"),
        ("drag pin → pin", "draw a wire"),
        ("click input", "flip it"),
        ("G", "move selection"),
        ("Shift+D", "duplicate"),
        ("X", "delete"),
        ("F", "frame everything"),
        ("Space / →", "play-pause / step"),
        ("Cmd+Z", "undo"),
    ] {
        ui.horizontal(|ui| {
            ui.label(mono(format!("{key:<15}")).color(theme::TEXT));
            ui.label(RichText::new(what).color(theme::MUTED));
        });
    }
}

pub fn tests(ui: &mut egui::Ui, state: &mut AppState) {
    let chip = state.canvas.chip.clone();
    ui.horizontal(|ui| {
        if ui.button("Run tests").clicked() {
            state.tests = Some(TestResults::run(&state.ws, &chip));
        }
        if let Some(results) = &state.tests {
            if !state.ws.design.is_buildable(&chip) {
                ui.label(
                    RichText::new("This chip has errors, so it can't be tested. See Problems.").color(theme::UNKNOWN),
                );
            } else if results.is_empty() {
                ui.label(RichText::new("No tests yet").color(theme::MUTED));
            } else {
                let total: usize = results.outcomes.iter().map(|o| o.rows.len()).sum();
                let passed: usize = results.outcomes.iter().map(|o| o.passed_rows()).sum();
                let (mark, color) = if results.passed() { ("✔", theme::ONE) } else { ("✖", theme::UNKNOWN) };
                let mut summary =
                    if total > 0 { format!("{mark} {passed}/{total} checks pass") } else { mark.to_string() };
                if let Some(check) = &results.builtin {
                    summary += &if check.passed() {
                        format!(" · matches built-in {}", check.gate.name())
                    } else {
                        format!(" · differs from built-in {}", check.gate.name())
                    };
                }
                ui.label(RichText::new(summary).color(color).strong());
            }
        }
    });
    ui.separator();
    let Some(results) = &state.tests else { return };
    egui::ScrollArea::vertical().show(ui, |ui| {
        for outcome in &results.outcomes {
            if let Some(error) = &outcome.error {
                ui.label(RichText::new(format!("can't build it: {error}")).color(theme::UNKNOWN));
            }
            for row in &outcome.rows {
                let mut inputs: Vec<String> = row.inputs.iter().map(|(n, v)| format!("{n}={v}")).collect();
                if row.ticks > 0 {
                    inputs.insert(0, format!("tick {}:", row.ticks));
                }
                let line = state.ws.project.sources.line(row.span);
                if row.passed() {
                    ui.label(mono(format!("✔ line {line:<3} {}", inputs.join(" "))).color(theme::ONE));
                } else {
                    let problem = if row.unsettled {
                        "still changing (is something oscillating?)".to_string()
                    } else {
                        row.failures
                            .iter()
                            .map(|f| format!("expected {}={}, got {}", f.output, f.expected, f.actual))
                            .collect::<Vec<_>>()
                            .join("; ")
                    };
                    ui.label(mono(format!("✖ line {line:<3} {} → {problem}", inputs.join(" "))).color(theme::UNKNOWN));
                }
            }
        }
        if let Some(check) = &results.builtin {
            for m in &check.mismatches {
                let inputs: Vec<String> = m.inputs.iter().map(Logic::to_string).collect();
                ui.label(
                    mono(format!(
                        "✖ inputs {} → built-in gives {}, yours gives {}",
                        inputs.join(" "),
                        m.expected,
                        m.actual
                    ))
                    .color(theme::UNKNOWN),
                );
            }
        }
        if results.is_empty()
            && let Some(info) = state.ws.design.chips.get(&chip)
        {
            ui.label(RichText::new("Add a test block to the chip's file, like this:").color(theme::MUTED));
            let ins: Vec<&str> = info.inputs.iter().map(|p| p.name.as_str()).collect();
            let outs: Vec<&str> = info.outputs.iter().map(|p| p.name.as_str()).collect();
            let zeros = |n: usize| vec!["0"; n].join(" ");
            let template = format!(
                "test {chip} {{\n  // {} | {}\n     {} | {}\n}}",
                ins.join(" "),
                outs.join(" "),
                zeros(ins.len()),
                zeros(outs.len())
            );
            ui.label(mono(template).color(theme::TEXT));
        }
    });
}

pub fn problems(ui: &mut egui::Ui, state: &mut AppState) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        if let Some(stale) = &state.ws.stale {
            ui.label(
                RichText::new(format!("⚠ {stale}. Showing the last version that read cleanly.")).color(theme::WARNING),
            );
            ui.separator();
        }
        if state.ws.problems.is_empty() {
            ui.label(RichText::new("No problems.").color(theme::MUTED));
        }
        for problem in &state.ws.problems {
            let (mark, color): (&str, Color32) =
                if problem.error { ("✖", theme::UNKNOWN) } else { ("⚠", theme::WARNING) };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(mark).color(color));
                ui.label(mono(&problem.location).color(theme::MUTED));
                ui.label(&problem.message);
            })
            .response
            .on_hover_text(mono(&problem.detail));
        }
    });
}
