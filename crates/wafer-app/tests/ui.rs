//! Drives the real app with simulated mouse and keyboard input. Each test
//! works on a fresh copy of a project in a temporary folder, and saves a
//! screenshot to `target/tmp/shots/` for a human (or Claude) to look at.

use std::path::{Path, PathBuf};

use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use wafer_app::app::{Tab, WaferApp};
use wafer_core::Logic;
use wafer_core::logic::bits_from_u64;

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("projects").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        std::fs::write(dir.join(file), text).unwrap();
    }
    dir
}

fn example_project(name: &str) -> PathBuf {
    let dir = project(name, &[]);
    let chips = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../chips");
    for entry in std::fs::read_dir(chips).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "chip" || e == "asm" || e == "hex") {
            std::fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
        }
    }
    dir
}

fn open(dir: &Path) -> Harness<'static, WaferApp> {
    open_sized(dir, egui::vec2(1400.0, 860.0))
}

/// A bigger window, for chips with a lot of boxes.
fn open_sized(dir: &Path, size: egui::Vec2) -> Harness<'static, WaferApp> {
    let dir = dir.to_path_buf();
    let mut harness = Harness::builder()
        .with_size(size)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(move |cc| WaferApp::new(cc, dir));
    harness.state_mut().state.sim.instant = true;
    harness.run_steps(4);
    harness
}

fn shot(harness: &mut Harness<'static, WaferApp>, name: &str) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("shots");
    std::fs::create_dir_all(&dir).unwrap();
    let image = harness.render().expect("rendering works");
    image.save(dir.join(format!("{name}.png"))).unwrap();
}

#[test]
fn shows_the_example_project() {
    let dir = example_project("overview");
    let mut h = open(&dir);
    // The most built-up chip: the one with the most levels of chips inside.
    let st = &h.state().state;
    let deepest = st.ws.chip_names().iter().map(|c| st.ws.design.depth(c)).max().unwrap();
    assert_eq!(st.ws.design.depth(&st.canvas.chip), deepest, "opens {} first", st.canvas.chip);
    h.state_mut().state.open_chip("HalfAdder");
    h.run_steps(4);
    shot(&mut h, "overview");
    let tests = h.state().state.tests.as_ref().unwrap();
    assert!(tests.passed() && !tests.is_empty());
}

fn press(h: &Harness<'static, WaferApp>, pos: Pos2, pressed: bool) {
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
}

fn click(h: &mut Harness<'static, WaferApp>, pos: Pos2) {
    h.hover_at(pos);
    h.run_steps(1);
    press(h, pos, true);
    h.run_steps(1);
    press(h, pos, false);
    h.run_steps(2);
}

fn drag(h: &mut Harness<'static, WaferApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    press(h, from, true);
    h.run_steps(1);
    for t in [0.25, 0.5, 0.75, 1.0] {
        h.hover_at(from.lerp(to, t));
        h.run_steps(1);
    }
    press(h, to, false);
    h.run_steps(3);
}

/// Opens the Shift+A menu at `at`, searches, and picks the first match.
fn add(h: &mut Harness<'static, WaferApp>, at: Pos2, search: &str) {
    h.hover_at(at);
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::SHIFT, Key::A);
    h.run_steps(2);
    h.event(Event::Text(search.into()));
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(3);
}

/// Screen position of a pin: `out` picks an output pin, otherwise an input.
fn pin(h: &mut Harness<'static, WaferApp>, key: &str, out: bool, index: usize) -> Pos2 {
    let st = &mut h.state_mut().state;
    let s = st.canvas.schematic(&st.ws).cloned().expect("a chip is open");
    let node = s.nodes.iter().find(|n| n.key == key).unwrap_or_else(|| panic!("no node {key}"));
    let p = if out { &node.outputs[index] } else { &node.inputs[index] };
    st.canvas.screen_pos((p.x, p.y))
}

fn node_center(h: &mut Harness<'static, WaferApp>, key: &str) -> Pos2 {
    let st = &mut h.state_mut().state;
    let s = st.canvas.schematic(&st.ws).cloned().expect("a chip is open");
    let node = s.nodes.iter().find(|n| n.key == key).unwrap_or_else(|| panic!("no node {key}"));
    st.canvas.screen_pos((node.x + node.w / 2.0, node.y + node.h / 2.0))
}

fn rename(h: &mut Harness<'static, WaferApp>, key: &str, from: &str, to: &str) {
    let at = node_center(h, key);
    click(h, at);
    h.get(by().value(from).role(egui::accesskit::Role::TextInput)).focus();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    h.event(Event::Text(to.into()));
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(3);
}

fn chip_text(dir: &Path, file: &str) -> String {
    std::fs::read_to_string(dir.join(file)).unwrap()
}

const MINE_TESTS: &str =
    "\ntest Mine {\n  // a b | sum carry\n     0 0 | 0   0\n     0 1 | 1   0\n     1 0 | 1   0\n     1 1 | 0   1\n}\n";

#[test]
fn builds_a_half_adder_on_the_canvas() {
    let dir = project("build", &[("Mine.chip", &format!("chip Mine {{\n}}\n{MINE_TESTS}"))]);
    let mut h = open(&dir);
    assert_eq!(h.state().state.canvas.chip, "Mine");
    let c = h.state().state.canvas.rect().center();

    add(&mut h, c + vec2(-300.0, -100.0), "Input");
    add(&mut h, c + vec2(-300.0, 60.0), "Input");
    add(&mut h, c + vec2(-60.0, -140.0), "Xor");
    add(&mut h, c + vec2(-60.0, 40.0), "And");
    add(&mut h, c + vec2(220.0, -120.0), "Output");
    add(&mut h, c + vec2(220.0, 60.0), "Output");

    let wires = [
        ("in:a", 0, "xor1", 0),
        ("in:b", 0, "xor1", 1),
        ("in:a", 0, "and1", 0),
        ("in:b", 0, "and1", 1),
        ("xor1", 0, "out:out", 0),
        ("and1", 0, "out:y", 0),
    ];
    for (from, fp, to, tp) in wires {
        let a = pin(&mut h, from, true, fp);
        let b = pin(&mut h, to, false, tp);
        drag(&mut h, a, b);
    }
    rename(&mut h, "out:out", "out", "sum");
    rename(&mut h, "out:y", "y", "carry");

    let text = chip_text(&dir, "Mine.chip");
    assert_eq!(
        text,
        format!(
            "chip Mine {{\n  in  a, b\n  out sum, carry\n  xor1 = Xor(a, b)\n  and1 = And(a, b)\n  sum = xor1\n  carry = and1\n}}\n{MINE_TESTS}"
        ),
        "{text}"
    );
    assert!(dir.join("Mine.layout.json").exists(), "positions are saved");

    // The tests in the file now pass.
    h.run_steps(2);
    let tests = h.state().state.tests.as_ref().unwrap();
    assert!(!tests.is_empty() && tests.passed(), "tests should pass");

    // Clicking an input flips it, and the simulation follows.
    let a = node_center(&mut h, "in:a");
    click(&mut h, a);
    h.run_steps(3);
    assert_eq!(h.state().state.sim.read("a"), Some(vec![Logic::One]));
    assert_eq!(h.state().state.sim.read("sum"), Some(vec![Logic::One]));
    assert_eq!(h.state().state.sim.read("carry"), Some(vec![Logic::Zero]));
    shot(&mut h, "built_half_adder");

    // Undo takes back the last rename; redo brings it back.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert!(chip_text(&dir, "Mine.chip").contains("out sum, y"));
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(2);
    assert!(chip_text(&dir, "Mine.chip").contains("out sum, carry"));
}

fn keys(h: &Harness<'static, WaferApp>) -> Vec<String> {
    let mut keys: Vec<String> = h.state().state.canvas_keys();
    keys.sort();
    keys
}

/// Lets the workspace's file polling notice a change made on disk.
fn wait_for_reload(h: &mut Harness<'static, WaferApp>) {
    std::thread::sleep(std::time::Duration::from_millis(450));
    h.run_steps(3);
}

#[test]
fn edits_from_outside_show_up_live() {
    let dir = example_project("live");
    let mut h = open(&dir);
    h.state_mut().state.open_chip("HalfAdder");
    h.run_steps(3);
    assert!(!keys(&h).contains(&"spare".to_string()));

    // Claude (or a text editor) adds a gate to the file.
    let path = dir.join("HalfAdder.chip");
    let text = chip_text(&dir, "HalfAdder.chip");
    std::fs::write(&path, text.replace("  carry = And(a, b)\n", "  carry = And(a, b)\n  spare = Not(a)\n")).unwrap();
    wait_for_reload(&mut h);
    assert!(keys(&h).contains(&"spare".to_string()), "the new gate appears");

    // A syntax error keeps the last good version on screen and pauses editing.
    std::fs::write(&path, "chip HalfAdder {\n  in a, b\n  out sum carry\n").unwrap();
    wait_for_reload(&mut h);
    assert!(h.state().state.ws.stale.is_some());
    assert!(keys(&h).contains(&"spare".to_string()), "still showing the last good version");
    let c = h.state().state.canvas.rect().center();
    add(&mut h, c, "Not");
    assert!(chip_text(&dir, "HalfAdder.chip").ends_with("out sum carry\n"), "editing is paused");
    shot(&mut h, "syntax_error");

    // Fixing the file brings everything back.
    std::fs::write(&path, &text).unwrap();
    wait_for_reload(&mut h);
    assert!(h.state().state.ws.stale.is_none());
    assert!(!keys(&h).contains(&"spare".to_string()));
}

#[test]
fn moves_deletes_and_undoes() {
    let dir = example_project("move");
    let mut h = open(&dir);
    h.state_mut().state.open_chip("HalfAdder");
    h.run_steps(3);
    let original = chip_text(&dir, "HalfAdder.chip");

    // Dragging a gate saves every position, so nothing else jumps.
    let from = node_center(&mut h, "sum");
    drag(&mut h, from, from + vec2(0.0, -80.0));
    let layout = chip_text(&dir, "HalfAdder.layout.json");
    for key in ["in:a", "in:b", "sum", "carry", "out:sum", "out:carry"] {
        assert!(layout.contains(&format!("\"{key}\"")), "{key} missing from {layout}");
    }
    assert_eq!(chip_text(&dir, "HalfAdder.chip"), original, "moving doesn't touch the chip text");

    // X deletes the selected gate. Its output is left undriven (a warning, not an error).
    let and_gate = node_center(&mut h, "carry");
    click(&mut h, and_gate);
    h.key_press(Key::X);
    h.run_steps(3);
    let text = chip_text(&dir, "HalfAdder.chip");
    assert!(!text.contains("And("), "{text}");
    assert!(h.state().state.ws.problems.iter().all(|p| !p.error));

    // Undo brings it back, exactly.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(chip_text(&dir, "HalfAdder.chip"), original);
}

#[test]
fn duplicates_and_deletes_wires() {
    let dir = example_project("duplicate");
    let mut h = open(&dir);
    h.state_mut().state.open_chip("HalfAdder");
    h.run_steps(3);

    // Shift+D copies the selected gate and picks it up; a click drops it.
    let xor = node_center(&mut h, "sum");
    click(&mut h, xor);
    h.key_press_modifiers(Modifiers::SHIFT, Key::D);
    h.run_steps(2);
    let drop = xor + vec2(0.0, 220.0);
    h.hover_at(drop);
    h.run_steps(2);
    click(&mut h, drop);
    let text = chip_text(&dir, "HalfAdder.chip");
    assert!(text.contains("xor1 = Xor()"), "{text}");
    assert!(chip_text(&dir, "HalfAdder.layout.json").contains("\"xor1\""));

    // Clicking a wire selects it; X disconnects everything it feeds.
    let st = &mut h.state_mut().state;
    let s = st.canvas.schematic(&st.ws).cloned().unwrap();
    let wire = s.wires.iter().find(|w| w.signal == "b").expect("b has a wire");
    let seg = wire.segments.iter().max_by(|a, b| (a[2] - a[0]).abs().total_cmp(&(b[2] - b[0]).abs())).unwrap();
    let on_wire = st.canvas.screen_pos(((seg[0] + seg[2]) / 2.0, seg[1]));
    click(&mut h, on_wire);
    assert!(h.state().state.canvas.selected_wire.is_some(), "the wire is selected");
    h.key_press(Key::X);
    h.run_steps(3);
    let text = chip_text(&dir, "HalfAdder.chip");
    assert!(text.contains("sum   = Xor(a)") && text.contains("carry = And(a)"), "{text}");
}

#[test]
fn makes_a_new_chip() {
    let dir = example_project("new_chip");
    let mut h = open(&dir);
    h.get(by().role(egui::accesskit::Role::TextInput)).focus();
    h.run_steps(1);
    h.event(Event::Text("Counter".into()));
    h.run_steps(1);
    h.get_by_label("Create").click();
    h.run_steps(3);
    assert_eq!(chip_text(&dir, "Counter.chip"), "chip Counter {\n}\n");
    assert_eq!(h.state().state.canvas.chip, "Counter");
}

/// When a signal first became 1, from the timeline's history.
fn rose_at(h: &Harness<'static, WaferApp>, path: &str) -> u64 {
    let trace = h.state().state.sim.trace.as_ref().expect("Add4 simulates");
    let i = trace.index_of(path).unwrap_or_else(|| panic!("{path} isn't on the timeline"));
    trace.changes[i].iter().skip(1).find(|(_, v)| v.as_slice() == [Logic::One]).map(|(t, _)| *t).expect("it rises")
}

#[test]
fn watches_a_carry_ripple_in_the_timeline() {
    let dir = example_project("ripple");
    let mut h = open(&dir);
    h.state_mut().state.open_chip("Add4");
    h.state_mut().show_tab(Tab::Timeline);
    h.run_steps(2);
    let sim = &mut h.state_mut().state.sim;
    sim.set_input("a", bits_from_u64(0xF, 4));
    sim.set_input("b", bits_from_u64(0x1, 4));
    h.run_steps(4);

    // 0xF + 0x1 carries out of every bit, one full adder after another.
    let carries: Vec<u64> = ["fa0.cout", "fa1.cout", "fa2.cout", "fa3.cout"].iter().map(|p| rose_at(&h, p)).collect();
    assert!(carries.windows(2).all(|w| w[0] < w[1]), "carries should ripple in order: {carries:?}");
    assert_eq!(h.state().state.sim.read("cout"), Some(vec![Logic::One]));
    shot(&mut h, "timeline_ripple");

    // Scrubbing back to before the last carry shows the old value on the canvas.
    let sim = &mut h.state_mut().state.sim;
    sim.cursor = Some(carries[3] - 1);
    assert_eq!(sim.value("fa3.cout"), Some(vec![Logic::Zero]));
    assert_eq!(sim.value("fa2.cout"), Some(vec![Logic::One]));
    h.run_steps(2);
    shot(&mut h, "timeline_scrubbed");

    // Clicking on the timeline scrubs; double-clicking goes back to live.
    h.state_mut().state.sim.cursor = None;
    let canvas = h.state().state.canvas.rect();
    let on_waves = egui::pos2(canvas.center().x - 120.0, canvas.max.y + 110.0);
    click(&mut h, on_waves);
    assert!(h.state().state.sim.cursor.is_some(), "clicking the waves sets the cursor");
    h.run_steps(60); // A second later, so the next two clicks are a fresh double-click.
    for pressed in [true, false, true, false] {
        press(&h, on_waves, pressed);
        h.run_steps(1);
    }
    h.run_steps(1);
    assert!(h.state().state.sim.cursor.is_none(), "double-click goes back to live");
}

#[test]
fn code_panel_follows_the_canvas_and_saves_typing() {
    let dir = example_project("code");
    let mut h = open(&dir);
    h.state_mut().state.open_chip("HalfAdder");
    h.state_mut().show_tab(Tab::Code);
    h.run_steps(3);
    assert!(h.state().state.code.text().contains("sum   = Xor(a, b)"));

    // A gate added on the canvas shows up in the code.
    let c = h.state().state.canvas.rect().center();
    add(&mut h, c + vec2(0.0, 120.0), "Nand");
    h.run_steps(2);
    assert!(h.state().state.code.text().contains("nand1 = Nand()"), "{}", h.state().state.code.text());

    // Typing in the code saves to the file and updates the canvas.
    let edited =
        chip_text(&dir, "HalfAdder.chip").replace("nand1 = Nand()", "nand1 = Nand(a, b)\n  spare = Not(nand1)");
    h.get(by().role(egui::accesskit::Role::MultilineTextInput)).focus();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    h.event(Event::Text(edited.clone()));
    h.run_steps(1);
    std::thread::sleep(wafer_app::code::SAVE_DELAY + std::time::Duration::from_millis(50));
    h.run_steps(3);
    assert_eq!(chip_text(&dir, "HalfAdder.chip"), edited);
    assert!(keys(&h).contains(&"spare".to_string()), "the typed gate is on the canvas");
    shot(&mut h, "code_panel");

    // If the file changes outside the app mid-typing, the app asks instead of overwriting it.
    h.event(Event::Text("// typing".into()));
    h.run_steps(1);
    let outside = edited.replace("spare = Not(nand1)", "spare = Not(a)");
    std::fs::write(dir.join("HalfAdder.chip"), &outside).unwrap();
    wait_for_reload(&mut h);
    assert!(h.state().state.code.has_conflict());
    assert_eq!(chip_text(&dir, "HalfAdder.chip"), outside, "the outside change wasn't overwritten");
    h.get_by_label("Use the file's version").click();
    h.run_steps(2);
    assert_eq!(h.state().state.code.text(), outside);
}

#[test]
fn code_panel_formats_and_completes() {
    let dir = project("complete", &[("Mine.chip", "chip Mine {\nin a,b\n  out y\n  y=Nand( a,b )\n}\n")]);
    let mut h = open(&dir);
    h.state_mut().show_tab(Tab::Code);
    h.run_steps(3);

    // Format tidies the file and saves it.
    h.get_by_label("Format").click();
    h.run_steps(2);
    let tidy = "chip Mine {\n  in  a, b\n  out y\n  y = Nand(a, b)\n}\n";
    assert_eq!(h.state().state.code.text(), tidy);
    std::thread::sleep(wafer_app::code::SAVE_DELAY + std::time::Duration::from_millis(50));
    h.run_steps(3);
    assert_eq!(chip_text(&dir, "Mine.chip"), tidy);

    // Typing suggests what fits; Tab accepts and lands inside the parentheses.
    h.get(by().role(egui::accesskit::Role::MultilineTextInput)).focus();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    h.event(Event::Text("chip Mine {\n  in  a, b\n  out y\n  y = Na".into()));
    h.run_steps(2);
    assert_eq!(h.state().state.code.suggestions(), vec!["Nand".to_string()]);
    shot(&mut h, "code_completion");
    h.key_press(Key::Tab);
    h.run_steps(2);
    h.event(Event::Text("a, b".into()));
    h.run_steps(2);
    assert!(h.state().state.code.text().ends_with("y = Nand(a, b)"), "{}", h.state().state.code.text());
    h.key_press(Key::Escape);
    h.run_steps(1);
    assert!(h.state().state.code.suggestions().is_empty());
}

#[test]
fn makes_a_chip_and_looks_inside_it() {
    let dir = example_project("hierarchy");
    let original = chip_text(&dir, "Xor.chip");
    let mut h = open(&dir);
    h.state_mut().state.open_chip("Xor");
    h.run_steps(3);
    h.state_mut().state.sim.set_input("a", vec![Logic::One]);
    h.run_steps(3);

    // Select two gates and press Cmd+G; the dialog suggests a name.
    h.state_mut().state.canvas.selection = ["n2".to_string(), "n3".to_string()].into();
    let c = h.state().state.canvas.rect().center();
    h.hover_at(c);
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::G);
    h.run_steps(2);
    assert!(h.state().state.canvas.make_chip_open());
    h.key_press(Key::Enter);
    h.run_steps(3);
    let xor = chip_text(&dir, "Xor.chip");
    assert!(xor.contains("part1 = Part(a, n1, b)"), "{xor}");
    assert!(chip_text(&dir, "Part.chip").contains("n2 = Nand(a, n1)"));
    let tests = h.state().state.tests.as_ref().unwrap();
    assert!(!tests.is_empty() && tests.passed(), "Xor still works");
    assert_eq!(h.state().state.canvas.selection.iter().next().map(String::as_str), Some("part1"));

    // Tab looks inside; the values come from the outer chip's simulation.
    h.hover_at(c);
    h.run_steps(1);
    h.key_press(Key::Tab);
    h.run_steps(3);
    let st = &h.state().state;
    assert_eq!(st.canvas.chip, "Part");
    assert_eq!(st.canvas.prefix, "part1.");
    assert_eq!(st.value("n1"), Some(vec![Logic::One]), "Nand(1, 0) outside");
    assert_eq!(st.value("n2"), Some(vec![Logic::Zero]), "Nand(a=1, n1=1)");
    assert_eq!(st.value("n3"), Some(vec![Logic::One]), "Nand(b=0, n1=1)");
    shot(&mut h, "inside_part");

    // Shift+Tab goes back out; double-click (or the tree) goes back in.
    h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
    h.run_steps(2);
    assert_eq!(h.state().state.canvas.chip, "Xor");
    let part = node_center(&mut h, "part1");
    h.hover_at(part);
    h.run_steps(1);
    for pressed in [true, false, true, false] {
        press(&h, part, pressed);
        h.run_steps(1);
    }
    h.run_steps(2);
    assert_eq!(h.state().state.canvas.chip, "Part", "double-click dives in");
    h.hover_at(c);
    h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
    h.run_steps(2);
    assert_eq!(h.state().state.canvas.chip, "Xor");
    h.get_by_label("part1 · Part").click();
    h.run_steps(2);
    assert_eq!(h.state().state.canvas.chip, "Part", "the tree dives in");

    // Undo takes the whole thing back, new file and all.
    h.state_mut().state.canvas.set_path(Vec::new());
    h.run_steps(1);
    h.hover_at(c);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(chip_text(&dir, "Xor.chip"), original);
    assert!(!dir.join("Part.chip").exists());
}

fn count(h: &Harness<'static, WaferApp>) -> Option<u64> {
    h.state().state.sim.read("count").and_then(|bits| wafer_core::logic::bits_to_u64(&bits))
}

#[test]
fn counts_with_the_clock() {
    let dir = example_project("counter");
    let mut h = open(&dir);
    h.state_mut().state.open_chip("Counter8");
    h.state_mut().show_tab(Tab::Timeline);
    h.run_steps(3);
    assert!(h.state().state.sim.has_clock());
    assert_eq!(count(&h), None, "flip-flops start unknown");

    // Click reset on, then press T for one tick.
    let reset = node_center(&mut h, "in:reset");
    click(&mut h, reset);
    h.key_press(Key::T);
    h.run_steps(2);
    assert_eq!(count(&h), Some(0));

    // Reset off, en on, and five ticks with the T key.
    click(&mut h, reset);
    let en = node_center(&mut h, "in:en");
    click(&mut h, en);
    for _ in 0..5 {
        h.key_press(Key::T);
        h.run_steps(1);
    }
    h.run_steps(1);
    assert_eq!(count(&h), Some(5));

    // The Tick button does the same.
    h.get_by_label("Tick").click();
    h.run_steps(2);
    assert_eq!(count(&h), Some(6));
    assert_eq!(h.state().state.sim.ticks(), 7);
    shot(&mut h, "counter");

    // Auto ticks the clock by itself: about 20 ticks in a second at 20 Hz.
    h.state_mut().state.sim.clock_hz = 20.0;
    h.get_by_label("Auto").click();
    h.run_steps(60);
    let counted = count(&h).unwrap();
    assert!((20..=30).contains(&counted), "counted to {counted}");
    let ticks = h.state().state.sim.trace.as_ref().unwrap().ticks.len();
    assert!(ticks >= 20, "the timeline saw {ticks} ticks");
}

#[test]
fn shows_a_roms_program() {
    let dir = example_project("rom");
    let mut h = open(&dir);
    h.state_mut().state.open_chip("Squares");
    h.run_steps(3);
    let rom = node_center(&mut h, "square");
    click(&mut h, rom);
    h.run_steps(3);
    // 18 words: the jump to main, 16 squares, and main's halt.
    let _ = h.get_by_label("squares.asm: 18 of 32 words (read-only)");
    let _ = h.get_by_label("0000  0011  jump main");
    let _ = h.get_by_label("0004  0009  data 0x0009");
    shot(&mut h, "rom_program");

    // Editing the program reloads the Rom.
    let path = dir.join("squares.asm");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("225", "225 0xBEEF")).unwrap();
    wait_for_reload(&mut h);
    let _ = h.get_by_label("squares.asm: 19 of 32 words (read-only)");
}

#[test]
fn wires_slices_and_joins_of_buses() {
    let dir = example_project("bits");
    let halves = "chip Halves {\n  in  a[16]\n  out sum[16]\n\n  lo = Add8()\n  hi = Add8()\n}\n";
    std::fs::write(dir.join("Halves.chip"), halves).unwrap();
    let mut h = open(&dir);
    h.state_mut().state.open_chip("Halves");
    h.run_steps(3);

    // 16-bit `a` into an 8-bit pin: pick which 8 bits.
    let (from, to) = (pin(&mut h, "in:a", true, 0), pin(&mut h, "lo", false, 0));
    drag(&mut h, from, to);
    assert!(h.state().state.canvas.bit_picker_open());
    shot(&mut h, "bit_picker");
    h.get_by_label("a[7:0]").click();
    h.run_steps(3);
    assert!(chip_text(&dir, "Halves.chip").contains("lo = Add8(a[7:0])"));
    let (from, to) = (pin(&mut h, "in:a", true, 0), pin(&mut h, "hi", false, 0));
    drag(&mut h, from, to);
    h.get_by_label("a[15:8]").click();
    h.run_steps(3);
    assert!(chip_text(&dir, "Halves.chip").contains("hi = Add8(a[15:8])"));

    // An 8-bit sum into the 16-bit output: pick which bits it fills.
    let (from, to) = (pin(&mut h, "lo", true, 0), pin(&mut h, "out:sum", false, 0));
    drag(&mut h, from, to);
    h.get_by_label("sum[7:0]").click();
    h.run_steps(3);
    assert!(chip_text(&dir, "Halves.chip").contains("sum[7:0] = lo.sum"));
    let (from, to) = (pin(&mut h, "hi", true, 0), pin(&mut h, "out:sum", false, 0));
    drag(&mut h, from, to);
    h.get_by_label("sum[15:8]").click();
    h.run_steps(3);
    let text = chip_text(&dir, "Halves.chip");
    assert!(text.contains("sum = {hi.sum, lo.sum}") && !text.contains("sum[7:0]"), "{text}");
    shot(&mut h, "joined_bus");

    // Escape cancels the picker without changing anything.
    let (from, to) = (pin(&mut h, "in:a", true, 0), pin(&mut h, "lo", false, 1));
    drag(&mut h, from, to);
    assert!(h.state().state.canvas.bit_picker_open());
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!h.state().state.canvas.bit_picker_open());
    assert_eq!(chip_text(&dir, "Halves.chip"), text);

    // A fresh built-in gate takes the whole bus, no questions asked. (Our
    // own Not here is a 1-bit chip, so it would ask which bit.)
    let c = h.state().state.canvas.rect().center();
    add(&mut h, c + vec2(0.0, 160.0), "Nand");
    let nand = keys(&h).into_iter().find(|k| k.starts_with("nand")).expect("a Nand was added");
    let (from, to) = (pin(&mut h, "in:a", true, 0), pin(&mut h, &nand, false, 0));
    drag(&mut h, from, to);
    assert!(!h.state().state.canvas.bit_picker_open());
    assert!(chip_text(&dir, "Halves.chip").contains(&format!("{nand} = Nand(a)")));
}

#[test]
fn spreads_our_1_bit_xor_across_a_bus() {
    let dir = example_project("spread");
    std::fs::write(dir.join("Wide.chip"), "chip Wide {\n  in  a[16], b[16]\n  out y[16]\n}\n").unwrap();
    let mut h = open(&dir);
    h.state_mut().state.open_chip("Wide");
    h.run_steps(3);

    // Our Xor is a 1-bit chip, but a fresh one takes whatever bus it's given.
    let c = h.state().state.canvas.rect().center();
    add(&mut h, c, "Xor");
    let xor = keys(&h).into_iter().find(|k| k.starts_with("xor")).expect("an Xor was added");
    for (port, pin_index) in [("a", 0), ("b", 1)] {
        let (from, to) = (pin(&mut h, &format!("in:{port}"), true, 0), pin(&mut h, &xor, false, pin_index));
        drag(&mut h, from, to);
        assert!(!h.state().state.canvas.bit_picker_open(), "no picker for {port}");
    }
    let (from, to) = (pin(&mut h, &xor, true, 0), pin(&mut h, "out:y", false, 0));
    drag(&mut h, from, to);
    let text = chip_text(&dir, "Wide.chip");
    assert!(text.contains(&format!("{xor} = Xor(a, b)")) && text.contains(&format!("y = {xor}")), "{text}");
    let errors: Vec<&str> =
        h.state().state.ws.problems.iter().filter(|p| p.error).map(|p| p.message.as_str()).collect();
    assert!(errors.is_empty(), "{errors:?}");

    let st = &mut h.state_mut().state;
    st.sim.set_input("a", bits_from_u64(0xF0F0, 16));
    st.sim.set_input("b", bits_from_u64(0xFF00, 16));
    h.run_steps(30);
    assert_eq!(h.state().state.sim.read("y"), Some(bits_from_u64(0x0FF0, 16)));
    shot(&mut h, "xor_x16");

    // Inside, it's our 4-Nand Xor with 16-bit wires.
    let at = node_center(&mut h, &xor);
    click(&mut h, at);
    h.key_press(Key::Tab);
    h.run_steps(3);
    let st = &mut h.state_mut().state;
    assert_eq!(st.canvas.chip, "Xor");
    assert_eq!(st.canvas.copies(&st.ws), 16);
    let s = st.canvas.schematic(&st.ws).cloned().unwrap();
    assert!(s.wires.iter().all(|w| w.width == 16), "every wire inside is 16 bits");
    shot(&mut h, "inside_xor_x16");
}

/// Builds Register16 on the canvas the way the guide in the chat says to,
/// starting from two Register8s with only the first one half wired.
#[test]
fn builds_register16_from_two_register8s() {
    let dir = example_project("register16");
    let tests =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../chips/Register16.chip")).unwrap();
    let tests = &tests[tests.find("test Register16").unwrap()..];
    let start = "chip Register16 {\n  in  d[16], load, reset\n  out out[16]\n  Lo = Register8(d[7:0], load)\n  Hi = Register8()\n}\n\n";
    std::fs::write(dir.join("Register16.chip"), format!("{start}{tests}")).unwrap();
    let _ = std::fs::remove_file(dir.join("Register16.layout.json"));
    let mut h = open(&dir);
    h.state_mut().state.open_chip("Register16");
    h.run_steps(3);

    // Step 1: the parts. (Lo and Hi are already there.)
    let c = h.state().state.canvas.rect().center();
    add(&mut h, c + vec2(-150.0, 170.0), "Not");
    add(&mut h, c + vec2(40.0, 200.0), "And");
    add(&mut h, c + vec2(40.0, -200.0), "Or");
    let (not, and, or) = ("not1", "and1", "or1");
    for key in [not, and, or] {
        assert!(keys(&h).contains(&key.to_string()), "{key} in {:?}", keys(&h));
    }
    // F frames everything, so every pin is on screen.
    h.hover_at(c);
    h.run_steps(1);
    h.key_press(Key::F);
    h.run_steps(3);

    let wire = |h: &mut Harness<'static, WaferApp>, from: &str, to: &str, pin_index: usize| {
        let (a, b) = (pin(h, from, true, 0), pin(h, to, false, pin_index));
        drag(h, a, b);
    };
    // Step 2: zeros when reset is 1. The 1-bit Not feeds every copy of the And.
    wire(&mut h, "in:reset", not, 0);
    wire(&mut h, "in:d", and, 0);
    wire(&mut h, not, and, 1);
    assert!(!h.state().state.canvas.bit_picker_open(), "a 1-bit wire into And ×16 needs no picking");
    // Step 3: load on load or reset.
    wire(&mut h, "in:load", or, 0);
    wire(&mut h, "in:reset", or, 1);
    // Step 4: each half loads from its half of the And.
    wire(&mut h, and, "Lo", 0);
    h.get_by_label("and1[7:0]").click();
    h.run_steps(3);
    wire(&mut h, and, "Hi", 0);
    h.get_by_label("and1[15:8]").click();
    h.run_steps(3);
    wire(&mut h, or, "Lo", 1);
    wire(&mut h, or, "Hi", 1);
    // Step 5: the two halves make the output.
    let (a, b) = (pin(&mut h, "Lo", true, 0), pin(&mut h, "out:out", false, 0));
    drag(&mut h, a, b);
    h.get_by_label("out[7:0]").click();
    h.run_steps(3);
    let (a, b) = (pin(&mut h, "Hi", true, 0), pin(&mut h, "out:out", false, 0));
    drag(&mut h, a, b);
    h.get_by_label("out[15:8]").click();
    h.run_steps(3);
    shot(&mut h, "register16");

    let text = chip_text(&dir, "Register16.chip");
    for line in [
        "Lo = Register8(and1[7:0], or1)",
        "Hi = Register8(and1[15:8], or1)",
        "not1 = Not(reset)",
        "and1 = And(d, not1)",
        "or1 = Or(load, reset)",
        "out = {Hi, Lo}",
    ] {
        assert!(text.contains(line), "missing `{line}`:\n{text}");
    }
    let results = wafer_app::simrun::TestResults::run(&h.state().state.ws, "Register16");
    assert!(results.passed(), "Register16's tests fail:\n{text}");
}

/// Builds Shift16 on the canvas the way the guide in the chat says to.
#[test]
fn builds_shift16_with_option_drags() {
    let dir = example_project("shift16");
    // Start from the empty chip, whatever chips/ holds now.
    let real = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../chips/Shift16.chip")).unwrap();
    let tests = &real[real.find("test Shift16").unwrap()..];
    let start = "chip Shift16 {\n  in  a[16], by[4]\n  out left[16], right[16]\n}\n\n";
    std::fs::write(dir.join("Shift16.chip"), format!("{start}{tests}")).unwrap();
    let _ = std::fs::remove_file(dir.join("Shift16.layout.json"));
    let mut h = open(&dir);
    h.state_mut().state.open_chip("Shift16");
    h.run_steps(3);

    // Step 1: eight Muxes, left chain on top and right chain below.
    let c = h.state().state.canvas.rect().center();
    for (i, x) in [-240.0, -80.0, 80.0, 240.0].into_iter().enumerate() {
        add(&mut h, c + vec2(x, -120.0), "Mux");
        add(&mut h, c + vec2(x, 120.0), "Mux");
        assert!(keys(&h).contains(&format!("mux{}", 2 * i + 2)));
    }
    h.hover_at(c);
    h.run_steps(1);
    h.key_press(Key::F);
    h.run_steps(3);

    let wire = |h: &mut Harness<'static, WaferApp>, from: &str, to: &str, pin_index: usize, option: bool| {
        let (a, b) = (pin(h, from, true, 0), pin(h, to, false, pin_index));
        if option {
            h.event(Event::ModifiersChanged(Modifiers::ALT));
        }
        drag(h, a, b);
        h.event(Event::ModifiersChanged(Modifiers::NONE));
        h.run_steps(1);
    };
    let click = |h: &mut Harness<'static, WaferApp>, label: &str| {
        h.get_by_label(label).click();
        h.run_steps(3);
    };
    // Steps 2 and 3: each stage passes its input, or shifts it, picked by one bit of `by`.
    // Adding alternated top and bottom: mux1, 3, 5, 7 are the left chain.
    let left = [("in:a", "mux1", 1, 0), ("mux1", "mux3", 2, 1), ("mux3", "mux5", 4, 2), ("mux5", "mux7", 8, 3)];
    let right = [("in:a", "mux2", 1, 0), ("mux2", "mux4", 2, 1), ("mux4", "mux6", 4, 2), ("mux6", "mux8", 8, 3)];
    for (stages, arrow) in [(left, "<<"), (right, ">>")] {
        for (input, mux, shift, bit) in stages {
            wire(&mut h, input, mux, 0, false);
            wire(&mut h, input, mux, 1, true);
            assert!(h.state().state.canvas.bit_picker_open(), "Option-drop asks which bits");
            if mux == "mux1" {
                shot(&mut h, "free_bit_picker");
            }
            click(&mut h, &format!("{arrow} {shift}"));
            wire(&mut h, "in:by", mux, 2, false);
            click(&mut h, &format!("by[{bit}]"));
        }
    }
    // Step 4: the ends of the chains are the outputs.
    wire(&mut h, "mux7", "out:left", 0, false);
    wire(&mut h, "mux8", "out:right", 0, false);
    shot(&mut h, "shift16");

    let text = chip_text(&dir, "Shift16.chip");
    for line in [
        "mux1 = Mux(a, {a[14:0], 0b0}, by[0])",
        "mux3 = Mux(mux1, {mux1[13:0], 0b00}, by[1])",
        "mux5 = Mux(mux3, {mux3[11:0], 0x0}, by[2])",
        "mux7 = Mux(mux5, {mux5[7:0], 0x00}, by[3])",
        "mux2 = Mux(a, {0b0, a[15:1]}, by[0])",
        "mux8 = Mux(mux6, {0x00, mux6[15:8]}, by[3])",
        "left = mux7",
        "right = mux8",
    ] {
        assert!(text.contains(line), "missing `{line}`:\n{text}");
    }
    let results = wafer_app::simrun::TestResults::run(&h.state().state.ws, "Shift16");
    assert!(results.passed(), "Shift16's tests fail:\n{text}");
}

fn right_click(h: &mut Harness<'static, WaferApp>, pos: Pos2) {
    h.hover_at(pos);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(2);
}

/// Builds Compare16 on the canvas the way the guide in the chat says to.
#[test]
fn builds_compare16_with_constants_and_bit_picks() {
    let dir = example_project("compare16");
    let real =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../chips/Compare16.chip")).unwrap();
    let tests = &real[real.find("test Compare16").unwrap()..];
    let start = "chip Compare16 {\n  in  a[16], b[16]\n  out eq, lt, ult\n}\n\n";
    std::fs::write(dir.join("Compare16.chip"), format!("{start}{tests}")).unwrap();
    let _ = std::fs::remove_file(dir.join("Compare16.layout.json"));
    let mut h = open(&dir);
    h.state_mut().state.open_chip("Compare16");
    h.run_steps(3);

    // Step 1: the parts, in three rows.
    let c = h.state().state.canvas.rect().center();
    // Spread out, so no pin sits on top of another.
    let parts = [
        ("Not", vec2(-300.0, -200.0)),
        ("Add16", vec2(-50.0, -200.0)),
        ("Not", vec2(200.0, -200.0)),
        ("Or", vec2(-340.0, 0.0)),
        ("Or", vec2(-170.0, 0.0)),
        ("Or", vec2(0.0, 0.0)),
        ("Or", vec2(170.0, 0.0)),
        ("Not", vec2(340.0, 0.0)),
        ("Xor", vec2(-50.0, 200.0)),
        ("Mux", vec2(200.0, 200.0)),
    ];
    for (part, at) in parts {
        add(&mut h, c + at, part);
    }
    rename(&mut h, "add16_1", "add16_1", "diff");
    h.hover_at(c);
    h.run_steps(1);
    h.key_press(Key::F);
    h.run_steps(3);

    let wire = |h: &mut Harness<'static, WaferApp>, from: (&str, usize), to: (&str, usize), pick: Option<&str>| {
        let (a, b) = (pin(h, from.0, true, from.1), pin(h, to.0, false, to.1));
        drag(h, a, b);
        if let Some(label) = pick {
            h.get_by_label(label).click();
            h.run_steps(3);
        }
    };
    let option_wire = |h: &mut Harness<'static, WaferApp>, from: (&str, usize), to: (&str, usize), pick: &str| {
        let (a, b) = (pin(h, from.0, true, from.1), pin(h, to.0, false, to.1));
        h.event(Event::ModifiersChanged(Modifiers::ALT));
        drag(h, a, b);
        h.event(Event::ModifiersChanged(Modifiers::NONE));
        h.run_steps(1);
        h.get_by_label(pick).click();
        h.run_steps(3);
    };

    // Step 2: a - b, as a + (not b) + 1.
    wire(&mut h, ("in:b", 0), ("not1", 0), None);
    wire(&mut h, ("in:a", 0), ("diff", 0), None);
    wire(&mut h, ("not1", 0), ("diff", 1), None);
    let cin = pin(&mut h, "diff", false, 2);
    right_click(&mut h, cin);
    shot(&mut h, "pin_menu");
    h.get_by_label("Set to 1").click();
    h.run_steps(3);
    // Step 3: no carry means a < b.
    wire(&mut h, ("diff", 1), ("not2", 0), None);
    wire(&mut h, ("not2", 0), ("out:ult", 0), None);
    // Step 4: equal when every bit of the difference is 0.
    option_wire(&mut h, ("diff", 0), ("or1", 0), "diff.sum[15:8]");
    shot(&mut h, "half_picker");
    option_wire(&mut h, ("diff", 0), ("or1", 1), "diff.sum[7:0]");
    option_wire(&mut h, ("or1", 0), ("or2", 0), "or1[7:4]");
    option_wire(&mut h, ("or1", 0), ("or2", 1), "or1[3:0]");
    option_wire(&mut h, ("or2", 0), ("or3", 0), "or2[3:2]");
    option_wire(&mut h, ("or2", 0), ("or3", 1), "or2[1:0]");
    option_wire(&mut h, ("or3", 0), ("or4", 0), "or3[1]");
    option_wire(&mut h, ("or3", 0), ("or4", 1), "or3[0]");
    wire(&mut h, ("or4", 0), ("not3", 0), None);
    wire(&mut h, ("not3", 0), ("out:eq", 0), None);
    // Step 5: signed: when the signs differ, a is smaller if it's the negative one.
    option_wire(&mut h, ("in:a", 0), ("xor1", 0), "a[15]");
    option_wire(&mut h, ("in:b", 0), ("xor1", 1), "b[15]");
    wire(&mut h, ("not2", 0), ("mux1", 0), None);
    wire(&mut h, ("in:a", 0), ("mux1", 1), Some("a[15]"));
    wire(&mut h, ("xor1", 0), ("mux1", 2), None);
    wire(&mut h, ("mux1", 0), ("out:lt", 0), None);
    shot(&mut h, "compare16");

    let text = chip_text(&dir, "Compare16.chip");
    for line in [
        "not1 = Not(b)",
        "diff = Add16(a, not1, 1)",
        "not2 = Not(diff.cout)",
        "or1 = Or(diff.sum[15:8], diff.sum[7:0])",
        "or2 = Or(or1[7:4], or1[3:0])",
        "or3 = Or(or2[3:2], or2[1:0])",
        "or4 = Or(or3[1], or3[0])",
        "not3 = Not(or4)",
        "xor1 = Xor(a[15], b[15])",
        "mux1 = Mux(not2, a[15], xor1)",
        "ult = not2",
        "eq = not3",
        "lt = mux1",
    ] {
        assert!(text.contains(line), "missing `{line}`:\n{text}");
    }
    let results = wafer_app::simrun::TestResults::run(&h.state().state.ws, "Compare16");
    assert!(results.passed(), "Compare16's tests fail:\n{text}");
}

/// Empties a chip in a copied project, keeping its ports and tests.
fn reset_chip(dir: &Path, chip: &str, ports: &str) {
    let real = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../chips/{chip}.chip")))
        .unwrap();
    let tests = &real[real.find(&format!("test {chip}")).unwrap()..];
    std::fs::write(dir.join(format!("{chip}.chip")), format!("chip {chip} {{\n{ports}}}\n\n{tests}")).unwrap();
    let _ = std::fs::remove_file(dir.join(format!("{chip}.layout.json")));
}

/// Builds Mux4 and then ALU16 on the canvas the way the guide in the chat
/// says to.
#[test]
fn builds_mux4_and_alu16() {
    let dir = example_project("alu16");
    reset_chip(&dir, "Mux4", "  in  a, b, c, d, s0, s1\n  out out\n");
    reset_chip(&dir, "ALU16", "  in  t[16], n[16], r[16], mem[16], op[4]\n  out out[16]\n");
    let mut h = open_sized(&dir, egui::vec2(2600.0, 1900.0));

    let wire = |h: &mut Harness<'static, WaferApp>, from: (&str, usize), to: (&str, usize), pick: Option<&str>| {
        let (a, b) = (pin(h, from.0, true, from.1), pin(h, to.0, false, to.1));
        drag(h, a, b);
        if let Some(label) = pick {
            h.get_by_label(label).click();
            h.run_steps(3);
        }
    };
    let set = |h: &mut Harness<'static, WaferApp>, to: (&str, usize), item: &str| {
        let at = pin(h, to.0, false, to.1);
        right_click(h, at);
        h.get_by_label(item).click();
        h.run_steps(3);
    };
    let type_value = |h: &mut Harness<'static, WaferApp>, to: (&str, usize), value: &str| {
        let at = pin(h, to.0, false, to.1);
        right_click(h, at);
        h.get(by().label("Set to").role(egui::accesskit::Role::TextInput)).focus();
        h.run_steps(1);
        h.event(Event::Text(value.into()));
        h.run_steps(1);
        h.key_press(Key::Enter);
        h.run_steps(3);
    };
    // Lays boxes out on a grid, like dragging them into place by hand.
    let arrange = |h: &mut Harness<'static, WaferApp>, chip: &str, grid: &[(&str, (f32, f32))]| {
        let positions = grid.iter().map(|(k, at)| (k.to_string(), *at)).collect();
        h.state_mut().state.ws.move_nodes(chip, positions);
        h.run_steps(2);
        let c = h.state().state.canvas.rect().center();
        h.hover_at(c);
        h.run_steps(1);
        h.key_press(Key::F);
        h.run_steps(3);
    };

    // Part 1: Mux4 from three Muxes.
    h.state_mut().state.open_chip("Mux4");
    h.run_steps(3);
    let c = h.state().state.canvas.rect().center();
    for x in [-150.0, 0.0, 150.0] {
        add(&mut h, c + vec2(x, 0.0), "Mux");
    }
    arrange(
        &mut h,
        "Mux4",
        &[("in:a", (0.0, 0.0)), ("in:b", (0.0, 50.0)), ("in:c", (0.0, 140.0)), ("in:d", (0.0, 190.0))]
            .into_iter()
            .chain([("in:s0", (0.0, 270.0)), ("in:s1", (0.0, 320.0))])
            .chain([("mux1", (160.0, 10.0)), ("mux2", (160.0, 150.0)), ("mux3", (320.0, 80.0))])
            .chain([("out:out", (460.0, 95.0))])
            .collect::<Vec<_>>(),
    );
    for (from, to) in [
        (("in:a", 0), ("mux1", 0)),
        (("in:b", 0), ("mux1", 1)),
        (("in:s0", 0), ("mux1", 2)),
        (("in:c", 0), ("mux2", 0)),
        (("in:d", 0), ("mux2", 1)),
        (("in:s0", 0), ("mux2", 2)),
        (("mux1", 0), ("mux3", 0)),
        (("mux2", 0), ("mux3", 1)),
        (("in:s1", 0), ("mux3", 2)),
        (("mux3", 0), ("out:out", 0)),
    ] {
        wire(&mut h, from, to, None);
    }
    let text = chip_text(&dir, "Mux4.chip");
    for line in ["mux1 = Mux(a, b, s0)", "mux2 = Mux(c, d, s0)", "mux3 = Mux(mux1, mux2, s1)", "out = mux3"] {
        assert!(text.contains(line), "missing `{line}`:\n{text}");
    }
    assert!(wafer_app::simrun::TestResults::run(&h.state().state.ws, "Mux4").passed(), "Mux4 fails:\n{text}");

    // Part 2: ALU16. Step 1: every part, laid out in rows.
    h.state_mut().state.open_chip("ALU16");
    h.run_steps(3);
    let c = h.state().state.canvas.rect().center();
    let parts = [
        "Not",
        "Add16",
        "Add16",
        "Add16",
        "And",
        "Or",
        "Xor",
        "Compare16",
        "Mux",
        "Mux",
        "Mux",
        "Shift16",
        "Mux4",
        "Mux4",
        "Mux4",
        "Mux4",
        "Mux4",
    ];
    for (i, part) in parts.into_iter().enumerate() {
        let (col, row) = ((i % 6) as f32, (i / 6) as f32);
        add(&mut h, c + vec2(-330.0 + col * 130.0, -150.0 + row * 140.0), part);
    }
    let col = |x: usize| 220.0 + x as f32 * 200.0;
    arrange(
        &mut h,
        "ALU16",
        &[
            ("in:t", (0.0, 0.0)),
            ("in:n", (0.0, 120.0)),
            ("in:r", (0.0, 900.0)),
            ("in:mem", (0.0, 1000.0)),
            ("in:op", (0.0, 1150.0)),
            ("not1", (col(0), 0.0)),
            ("add16_1", (col(1), 0.0)),
            ("add16_2", (col(1), 150.0)),
            ("add16_3", (col(1), 300.0)),
            ("and1", (col(0), 450.0)),
            ("or1", (col(0), 560.0)),
            ("xor1", (col(0), 670.0)),
            ("compare16_1", (col(1), 500.0)),
            ("mux1", (col(2), 450.0)),
            ("mux2", (col(2), 550.0)),
            ("mux3", (col(2), 650.0)),
            ("shift16_1", (col(1), 760.0)),
            ("mux4_1", (col(3), 0.0)),
            ("mux4_2", (col(3), 300.0)),
            ("mux4_3", (col(3), 600.0)),
            ("mux4_4", (col(3), 900.0)),
            ("mux4_5", (col(4), 450.0)),
            ("out:out", (col(5), 520.0)),
        ],
    );
    for (key, name) in
        [("add16_1", "add"), ("add16_2", "sub"), ("add16_3", "dec"), ("compare16_1", "cmp"), ("shift16_1", "shift")]
    {
        rename(&mut h, key, key, name);
    }

    // Step 2: add, subtract, minus one.
    wire(&mut h, ("in:t", 0), ("not1", 0), None);
    wire(&mut h, ("in:t", 0), ("add", 0), None);
    wire(&mut h, ("in:n", 0), ("add", 1), None);
    set(&mut h, ("add", 2), "Set to 0");
    wire(&mut h, ("in:n", 0), ("sub", 0), None);
    wire(&mut h, ("not1", 0), ("sub", 1), None);
    set(&mut h, ("sub", 2), "Set to 1");
    wire(&mut h, ("in:t", 0), ("dec", 0), None);
    set(&mut h, ("dec", 1), "Set to all 1s (0xFFFF)");
    set(&mut h, ("dec", 2), "Set to 0");
    // Step 3: and, or, xor.
    for gate in ["and1", "or1", "xor1"] {
        wire(&mut h, ("in:t", 0), (gate, 0), None);
        wire(&mut h, ("in:n", 0), (gate, 1), None);
    }
    // Step 4: comparisons, widened to Forth flags.
    wire(&mut h, ("in:n", 0), ("cmp", 0), None);
    wire(&mut h, ("in:t", 0), ("cmp", 1), None);
    for (mux, flag) in [("mux1", 0), ("mux2", 1), ("mux3", 2)] {
        set(&mut h, (mux, 0), "Set to 0");
        type_value(&mut h, (mux, 1), "0xFFFF");
        wire(&mut h, ("cmp", flag), (mux, 2), None);
    }
    // Step 5: shifts.
    wire(&mut h, ("in:n", 0), ("shift", 0), None);
    wire(&mut h, ("in:t", 0), ("shift", 1), Some("t[3:0]"));
    // Step 6: pick within each group of four, then pick the group.
    let groups = [
        ("mux4_1", [("in:t", 0), ("in:n", 0), ("add", 0), ("sub", 0)]),
        ("mux4_2", [("and1", 0), ("or1", 0), ("xor1", 0), ("not1", 0)]),
        ("mux4_3", [("mux1", 0), ("mux2", 0), ("mux3", 0), ("shift", 0)]),
        ("mux4_4", [("shift", 1), ("in:mem", 0), ("in:r", 0), ("dec", 0)]),
        ("mux4_5", [("mux4_1", 0), ("mux4_2", 0), ("mux4_3", 0), ("mux4_4", 0)]),
    ];
    for (mux4, inputs) in groups {
        for (pin_index, from) in inputs.into_iter().enumerate() {
            wire(&mut h, from, (mux4, pin_index), None);
        }
        let (s0, s1) = if mux4 == "mux4_5" { ("op[2]", "op[3]") } else { ("op[0]", "op[1]") };
        wire(&mut h, ("in:op", 0), (mux4, 4), Some(s0));
        wire(&mut h, ("in:op", 0), (mux4, 5), Some(s1));
    }
    wire(&mut h, ("mux4_5", 0), ("out:out", 0), None);
    shot(&mut h, "alu16");

    let text = chip_text(&dir, "ALU16.chip");
    for line in [
        "not1 = Not(t)",
        "add = Add16(t, n, 0)",
        "sub = Add16(n, not1, 1)",
        "dec = Add16(t, 0xFFFF, 0)",
        "and1 = And(t, n)",
        "or1 = Or(t, n)",
        "xor1 = Xor(t, n)",
        "cmp = Compare16(n, t)",
        "mux1 = Mux(0, 0xFFFF, cmp.eq)",
        "mux2 = Mux(0, 0xFFFF, cmp.lt)",
        "mux3 = Mux(0, 0xFFFF, cmp.ult)",
        "shift = Shift16(n, t[3:0])",
        "mux4_1 = Mux4(t, n, add.sum, sub.sum, op[0], op[1])",
        "mux4_2 = Mux4(and1, or1, xor1, not1, op[0], op[1])",
        "mux4_3 = Mux4(mux1, mux2, mux3, shift.left, op[0], op[1])",
        "mux4_4 = Mux4(shift.right, mem, r, dec.sum, op[0], op[1])",
        "mux4_5 = Mux4(mux4_1, mux4_2, mux4_3, mux4_4, op[2], op[3])",
        "out = mux4_5",
    ] {
        assert!(text.contains(line), "missing `{line}`:\n{text}");
    }
    let results = wafer_app::simrun::TestResults::run(&h.state().state.ws, "ALU16");
    assert!(results.passed(), "ALU16's tests fail:\n{text}");
}

/// Builds Inc13 on the canvas the way the guide in the chat says to.
#[test]
fn builds_inc13_from_add16() {
    let dir = example_project("inc13");
    reset_chip(&dir, "Inc13", "  in  a[13]\n  out out[13]\n");
    let mut h = open(&dir);
    h.state_mut().state.open_chip("Inc13");
    h.run_steps(3);
    let c = h.state().state.canvas.rect().center();
    add(&mut h, c, "Add16");
    h.hover_at(c);
    h.run_steps(1);
    h.key_press(Key::F);
    h.run_steps(3);

    // 13 bits into a 16-bit pin: they fill the low bits.
    let (from, to) = (pin(&mut h, "in:a", true, 0), pin(&mut h, "add16_1", false, 0));
    drag(&mut h, from, to);
    h.get_by_label("add16_1.a[12:0]").click();
    h.run_steps(3);
    for (pin_index, item) in [(1, "Set to 1 (0x0001)"), (2, "Set to 0")] {
        let at = pin(&mut h, "add16_1", false, pin_index);
        right_click(&mut h, at);
        h.get_by_label(item).click();
        h.run_steps(3);
    }
    // The low 13 bits of the sum are the answer.
    let (from, to) = (pin(&mut h, "add16_1", true, 0), pin(&mut h, "out:out", false, 0));
    drag(&mut h, from, to);
    h.get_by_label("add16_1.sum[12:0]").click();
    h.run_steps(3);
    shot(&mut h, "inc13");

    let text = chip_text(&dir, "Inc13.chip");
    for line in ["add16_1 = Add16({0b000, a}, 1, 0)", "out = add16_1.sum[12:0]"] {
        assert!(text.contains(line), "missing `{line}`:\n{text}");
    }
    let results = wafer_app::simrun::TestResults::run(&h.state().state.ws, "Inc13");
    assert!(results.passed(), "Inc13's tests fail:\n{text}");
}

/// Drags from an output pin to an input pin, then clicks `pick` in the
/// bit picker if one is given.
fn connect(h: &mut Harness<'static, WaferApp>, from: (&str, usize), to: (&str, usize), pick: Option<&str>) {
    let (a, b) = (pin(h, from.0, true, from.1), pin(h, to.0, false, to.1));
    drag(h, a, b);
    if let Some(label) = pick {
        h.get_by_label(label).click();
        h.run_steps(3);
    }
}

/// Option-drags from an output pin to an input pin and clicks `pick`.
fn option_connect(h: &mut Harness<'static, WaferApp>, from: (&str, usize), to: (&str, usize), pick: &str) {
    let (a, b) = (pin(h, from.0, true, from.1), pin(h, to.0, false, to.1));
    h.event(Event::ModifiersChanged(Modifiers::ALT));
    drag(h, a, b);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
    h.get_by_label(pick).click();
    h.run_steps(3);
}

/// Right-clicks an input pin and picks `item` from its menu.
fn pin_menu(h: &mut Harness<'static, WaferApp>, to: (&str, usize), item: &str) {
    let at = pin(h, to.0, false, to.1);
    right_click(h, at);
    h.get_by_label(item).click();
    h.run_steps(3);
}

/// Right-clicks an input pin and types a number into its Set to field.
fn pin_number(h: &mut Harness<'static, WaferApp>, to: (&str, usize), value: &str) {
    let at = pin(h, to.0, false, to.1);
    right_click(h, at);
    h.get(by().label("Set to").role(egui::accesskit::Role::TextInput)).focus();
    h.run_steps(1);
    h.event(Event::Text(value.into()));
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(3);
}

/// Builds StackPointer on the canvas the way the guide in the chat says to.
#[test]
fn builds_stack_pointer() {
    let dir = example_project("stack_pointer");
    reset_chip(&dir, "StackPointer", "  in  delta[2], move, reset\n  out sp[5]\n");
    let mut h = open(&dir);
    h.state_mut().state.open_chip("StackPointer");
    h.run_steps(3);

    // Step 1: the parts, left to right.
    let c = h.state().state.canvas.rect().center();
    for (part, x) in [("Mux4", -300.0), ("Add8", -130.0), ("Mux", 30.0), ("Mux", 160.0), ("Dff", 290.0)] {
        add(&mut h, c + vec2(x, 0.0), part);
    }
    h.hover_at(c);
    h.run_steps(1);
    h.key_press(Key::F);
    h.run_steps(3);

    // Step 2: how far to move, picked by delta.
    pin_number(&mut h, ("mux4_1", 0), "0x00");
    pin_menu(&mut h, ("mux4_1", 1), "Set to 1 (0x01)");
    pin_number(&mut h, ("mux4_1", 2), "0xFE");
    pin_menu(&mut h, ("mux4_1", 3), "Set to all 1s (0xFF)");
    connect(&mut h, ("in:delta", 0), ("mux4_1", 4), Some("delta[0]"));
    connect(&mut h, ("in:delta", 0), ("mux4_1", 5), Some("delta[1]"));
    // Step 3: add it to where the pointer is now.
    connect(&mut h, ("dff1", 0), ("add8_1", 0), None);
    connect(&mut h, ("mux4_1", 0), ("add8_1", 1), None);
    pin_menu(&mut h, ("add8_1", 2), "Set to 0");
    // Step 4: move or stay, then reset.
    connect(&mut h, ("add8_1", 0), ("mux1", 1), None);
    connect(&mut h, ("dff1", 0), ("mux1", 0), None);
    connect(&mut h, ("in:move", 0), ("mux1", 2), None);
    connect(&mut h, ("mux1", 0), ("mux2", 0), None);
    pin_menu(&mut h, ("mux2", 1), "Set to 0");
    connect(&mut h, ("in:reset", 0), ("mux2", 2), None);
    // Step 5: remember it, and show the low 5 bits.
    connect(&mut h, ("mux2", 0), ("dff1", 0), None);
    connect(&mut h, ("dff1", 0), ("out:sp", 0), Some("dff1[4:0]"));
    shot(&mut h, "stack_pointer");

    let text = chip_text(&dir, "StackPointer.chip");
    for line in [
        "mux4_1 = Mux4(0x00, 1, 0xFE, 0xFF, delta[0], delta[1])",
        "add8_1 = Add8(dff1, mux4_1, 0)",
        "mux1 = Mux(dff1, add8_1.sum, move)",
        "mux2 = Mux(mux1, 0, reset)",
        "dff1 = Dff(mux2)",
        "sp = dff1[4:0]",
    ] {
        assert!(text.contains(line), "missing `{line}`:\n{text}");
    }
    let results = wafer_app::simrun::TestResults::run(&h.state().state.ws, "StackPointer");
    assert!(results.passed(), "StackPointer's tests fail:\n{text}");
}

/// Builds Decode on the canvas the way the guide in the chat says to.
#[test]
fn builds_decode() {
    let dir = example_project("decode");
    let ports = "  in  insn[16]\n  out lit, value[16], jump, jz, call, addr[13]\n  out ret, op[4], t_to_n, t_to_r, store, rd[2], dd[2]\n";
    reset_chip(&dir, "Decode", ports);
    let mut h = open_sized(&dir, egui::vec2(2000.0, 1400.0));
    h.state_mut().state.open_chip("Decode");
    h.run_steps(3);

    // Step 1: the parts.
    let c = h.state().state.canvas.rect().center();
    for (part, at) in [
        ("Mux4", vec2(-250.0, -220.0)),
        ("Mux", vec2(-50.0, -200.0)),
        ("Mux4", vec2(-250.0, 60.0)),
        ("Mux", vec2(-50.0, 80.0)),
        ("And", vec2(-250.0, 300.0)),
    ] {
        add(&mut h, c + at, part);
    }
    h.hover_at(c);
    h.run_steps(1);
    h.key_press(Key::F);
    h.run_steps(3);

    // Step 2: the control word, as if every instruction were an ALU one.
    connect(&mut h, ("in:insn", 0), ("mux4_1", 3), None);
    pin_number(&mut h, ("mux4_1", 0), "0x0000");
    pin_number(&mut h, ("mux4_1", 1), "0x0103");
    pin_number(&mut h, ("mux4_1", 2), "0x0004");
    option_connect(&mut h, ("in:insn", 0), ("mux4_1", 4), "insn[13]");
    option_connect(&mut h, ("in:insn", 0), ("mux4_1", 5), "insn[14]");
    connect(&mut h, ("mux4_1", 0), ("mux1", 0), None);
    pin_number(&mut h, ("mux1", 1), "0x0081");
    connect(&mut h, ("in:insn", 0), ("mux1", 2), Some("insn[15]"));
    // Step 3: which kind of jump, if any.
    pin_number(&mut h, ("mux4_2", 0), "0b001");
    pin_number(&mut h, ("mux4_2", 1), "0b010");
    pin_number(&mut h, ("mux4_2", 2), "0b101");
    pin_menu(&mut h, ("mux4_2", 3), "Set to 0");
    connect(&mut h, ("in:insn", 0), ("mux4_2", 4), Some("insn[13]"));
    connect(&mut h, ("in:insn", 0), ("mux4_2", 5), Some("insn[14]"));
    connect(&mut h, ("mux4_2", 0), ("mux2", 0), None);
    pin_menu(&mut h, ("mux2", 1), "Set to 0");
    connect(&mut h, ("in:insn", 0), ("mux2", 2), Some("insn[15]"));
    // Step 4: a literal's value is the instruction without its top bit.
    connect(&mut h, ("in:insn", 0), ("and1", 0), None);
    pin_number(&mut h, ("and1", 1), "0x7FFF");
    // Step 5: every output is a slice.
    let outputs = [
        ("in:insn", "out:lit", Some("insn[15]")),
        ("and1", "out:value", None),
        ("in:insn", "out:addr", Some("insn[12:0]")),
        ("mux2", "out:jump", Some("mux2[0]")),
        ("mux2", "out:jz", Some("mux2[1]")),
        ("mux2", "out:call", Some("mux2[2]")),
        ("mux1", "out:ret", Some("mux1[12]")),
        ("mux1", "out:op", Some("mux1[11:8]")),
        ("mux1", "out:t_to_n", Some("mux1[7]")),
        ("mux1", "out:t_to_r", Some("mux1[6]")),
        ("mux1", "out:store", Some("mux1[5]")),
        ("mux1", "out:rd", Some("mux1[3:2]")),
        ("mux1", "out:dd", Some("mux1[1:0]")),
    ];
    for (from, to, pick) in outputs {
        connect(&mut h, (from, 0), (to, 0), pick);
    }
    shot(&mut h, "decode");

    let text = chip_text(&dir, "Decode.chip");
    for line in [
        "mux4_1 = Mux4(0x0000, 0x0103, 0x0004, insn, insn[13], insn[14])",
        "mux1 = Mux(mux4_1, 0x0081, insn[15])",
        "mux4_2 = Mux4(0b001, 0b010, 0b101, 0, insn[13], insn[14])",
        "mux2 = Mux(mux4_2, 0, insn[15])",
        "and1 = And(insn, 0x7FFF)",
        "lit = insn[15]",
        "value = and1",
        "addr = insn[12:0]",
        "jump = mux2[0]",
        "call = mux2[2]",
        "op = mux1[11:8]",
        "dd = mux1[1:0]",
    ] {
        assert!(text.contains(line), "missing `{line}`:\n{text}");
    }
    let results = wafer_app::simrun::TestResults::run(&h.state().state.ws, "Decode");
    assert!(results.passed(), "Decode's tests fail:\n{text}");
}

/// Builds Stack on the canvas the way the guide in the chat says to.
#[test]
fn builds_stack() {
    let dir = example_project("stack");
    reset_chip(&dir, "Stack", "  in  data[16], write, delta[2], execute, reset\n  out top[16]\n");
    let mut h = open(&dir);
    h.state_mut().state.open_chip("Stack");
    h.run_steps(3);

    // Step 1: the parts, in three columns.
    let c = h.state().state.canvas.rect().center();
    for (part, at) in [
        ("StackPointer", vec2(-240.0, -190.0)),
        ("Dff", vec2(-240.0, -40.0)),
        ("Mux", vec2(-240.0, 120.0)),
        ("Dff", vec2(-50.0, 120.0)),
        ("Mux", vec2(140.0, 120.0)),
        ("Ram", vec2(280.0, -60.0)),
    ] {
        add(&mut h, c + at, part);
    }
    h.hover_at(c);
    h.run_steps(1);
    h.key_press(Key::F);
    h.run_steps(3);

    // Step 2: the pointer moves in execute ticks.
    connect(&mut h, ("in:delta", 0), ("stackpointer1", 0), None);
    connect(&mut h, ("in:execute", 0), ("stackpointer1", 1), None);
    connect(&mut h, ("in:reset", 0), ("stackpointer1", 2), None);
    // Step 3: catch what to write...
    connect(&mut h, ("in:data", 0), ("dff1", 0), None);
    // Step 4: ...and whether, cleared by reset...
    connect(&mut h, ("in:write", 0), ("mux1", 0), None);
    pin_menu(&mut h, ("mux1", 1), "Set to 0");
    connect(&mut h, ("in:reset", 0), ("mux1", 2), None);
    connect(&mut h, ("mux1", 0), ("dff2", 0), None);
    // Step 5: ...and only in fetch ticks.
    connect(&mut h, ("dff2", 0), ("mux2", 0), None);
    pin_menu(&mut h, ("mux2", 1), "Set to 0");
    connect(&mut h, ("in:execute", 0), ("mux2", 2), None);
    // Step 6: the memory.
    connect(&mut h, ("stackpointer1", 0), ("ram1", 0), None);
    connect(&mut h, ("dff1", 0), ("ram1", 1), None);
    connect(&mut h, ("mux2", 0), ("ram1", 2), None);
    connect(&mut h, ("ram1", 0), ("out:top", 0), None);
    shot(&mut h, "stack");

    let text = chip_text(&dir, "Stack.chip");
    for line in [
        "stackpointer1 = StackPointer(delta, execute, reset)",
        "dff1 = Dff(data)",
        "mux1 = Mux(write, 0, reset)",
        "dff2 = Dff(mux1)",
        "mux2 = Mux(dff2, 0, execute)",
        "ram1 = Ram(stackpointer1, dff1, mux2)",
        "top = ram1",
    ] {
        assert!(text.contains(line), "missing `{line}`:\n{text}");
    }
    let results = wafer_app::simrun::TestResults::run(&h.state().state.ws, "Stack");
    assert!(results.passed(), "Stack's tests fail:\n{text}");
}

/// Builds Sequencer on the canvas the way the guide in the chat says to.
#[test]
fn builds_sequencer() {
    let dir = example_project("sequencer");
    let ports = "  in  reset, jump, jz, ret, t[16], r[16], addr[13]\n  out execute, pc[13], pc1[13]\n";
    reset_chip(&dir, "Sequencer", ports);
    let mut h = open_sized(&dir, egui::vec2(2000.0, 1400.0));
    h.state_mut().state.open_chip("Sequencer");
    h.run_steps(3);

    // Step 1: the parts, in rows.
    let c = h.state().state.canvas.rect().center();
    for (part, at) in [
        ("Not", vec2(-300.0, -260.0)),
        ("Mux", vec2(-140.0, -260.0)),
        ("Dff", vec2(20.0, -260.0)),
        ("Register16", vec2(-300.0, -60.0)),
        ("Inc13", vec2(-60.0, -60.0)),
        ("Compare16", vec2(-300.0, 160.0)),
        ("And", vec2(-90.0, 160.0)),
        ("Or", vec2(80.0, 160.0)),
        ("Mux", vec2(200.0, -20.0)),
        ("Mux", vec2(360.0, -20.0)),
    ] {
        add(&mut h, c + at, part);
    }
    h.hover_at(c);
    h.run_steps(1);
    h.key_press(Key::F);
    h.run_steps(3);

    // Step 2: the phase flips every tick, and reset makes it fetch.
    connect(&mut h, ("dff1", 0), ("not1", 0), None);
    connect(&mut h, ("not1", 0), ("mux1", 0), None);
    pin_menu(&mut h, ("mux1", 1), "Set to 0");
    connect(&mut h, ("in:reset", 0), ("mux1", 2), None);
    connect(&mut h, ("mux1", 0), ("dff1", 0), None);
    connect(&mut h, ("dff1", 0), ("out:execute", 0), None);
    // Step 3: the PC loads at the end of execute ticks; pc1 is the next one.
    connect(&mut h, ("dff1", 0), ("register16_1", 1), None);
    connect(&mut h, ("in:reset", 0), ("register16_1", 2), None);
    connect(&mut h, ("register16_1", 0), ("inc13_1", 0), Some("register16_1[12:0]"));
    connect(&mut h, ("register16_1", 0), ("out:pc", 0), Some("register16_1[12:0]"));
    connect(&mut h, ("inc13_1", 0), ("out:pc1", 0), None);
    // Step 4: is T zero?
    connect(&mut h, ("in:t", 0), ("compare16_1", 0), None);
    pin_menu(&mut h, ("compare16_1", 1), "Set to 0");
    // Step 5: jump, or jz while T is 0.
    connect(&mut h, ("in:jz", 0), ("and1", 0), None);
    connect(&mut h, ("compare16_1", 0), ("and1", 1), None);
    connect(&mut h, ("in:jump", 0), ("or1", 0), None);
    connect(&mut h, ("and1", 0), ("or1", 1), None);
    // Step 6: the next PC.
    connect(&mut h, ("inc13_1", 0), ("mux2", 0), None);
    connect(&mut h, ("in:addr", 0), ("mux2", 1), None);
    connect(&mut h, ("or1", 0), ("mux2", 2), None);
    connect(&mut h, ("mux2", 0), ("mux3", 0), None);
    connect(&mut h, ("in:r", 0), ("mux3", 1), Some("r[12:0]"));
    connect(&mut h, ("in:ret", 0), ("mux3", 2), None);
    connect(&mut h, ("mux3", 0), ("register16_1", 0), Some("register16_1.d[12:0]"));
    shot(&mut h, "sequencer");

    let text = chip_text(&dir, "Sequencer.chip");
    for line in [
        "not1 = Not(dff1)",
        "mux1 = Mux(not1, 0, reset)",
        "dff1 = Dff(mux1)",
        "register16_1 = Register16({0b000, mux3}, dff1, reset)",
        "inc13_1 = Inc13(register16_1[12:0])",
        "compare16_1 = Compare16(t, 0)",
        "and1 = And(jz, compare16_1.eq)",
        "or1 = Or(jump, and1)",
        "mux2 = Mux(inc13_1, addr, or1)",
        "mux3 = Mux(mux2, r[12:0], ret)",
        "execute = dff1",
        "pc = register16_1[12:0]",
        "pc1 = inc13_1",
    ] {
        assert!(text.contains(line), "missing `{line}`:\n{text}");
    }
    let results = wafer_app::simrun::TestResults::run(&h.state().state.ws, "Sequencer");
    assert!(results.passed(), "Sequencer's tests fail:\n{text}");
}

/// Builds Crumb on the canvas the way the guide in the chat says to, then
/// runs hello.asm on it next to the reference Crumb.
#[test]
fn builds_crumb() {
    let dir = example_project("crumb");
    let ports = "  in  reset, mem_in[16]\n  out addr[16], mem_out[16], write\n  out execute, pc[13], t[16], r[16]\n";
    reset_chip(&dir, "Crumb", ports);
    let mut h = open_sized(&dir, egui::vec2(2600.0, 1900.0));
    h.state_mut().state.open_chip("Crumb");
    h.run_steps(3);

    // Step 1: the parts, laid out in columns like dragging them into place.
    let c = h.state().state.canvas.rect().center();
    let parts = [
        "Not",
        "Register16",
        "Decode",
        "Sequencer",
        "ALU16",
        "Mux",
        "Register16",
        "Stack",
        "Mux",
        "Or",
        "Stack",
        "Mux",
        "And",
    ];
    for (i, part) in parts.into_iter().enumerate() {
        let (col, row) = ((i % 5) as f32, (i / 5) as f32);
        add(&mut h, c + vec2(-500.0 + col * 220.0, -300.0 + row * 250.0), part);
    }
    let grid = [
        ("in:reset", (0.0, 0.0)),
        ("in:mem_in", (0.0, 260.0)),
        ("not1", (260.0, 0.0)),
        ("register16_1", (260.0, 160.0)),
        ("decode1", (260.0, 380.0)),
        ("sequencer1", (560.0, 0.0)),
        ("alu16_1", (560.0, 380.0)),
        ("mux1", (560.0, 620.0)),
        ("register16_2", (840.0, 600.0)),
        ("stack1", (840.0, 800.0)),
        ("mux2", (560.0, 1000.0)),
        ("or1", (560.0, 1140.0)),
        ("stack2", (840.0, 1040.0)),
        ("mux3", (1120.0, 100.0)),
        ("and1", (1120.0, 300.0)),
        ("out:addr", (1400.0, 100.0)),
        ("out:mem_out", (1400.0, 220.0)),
        ("out:write", (1400.0, 340.0)),
        ("out:execute", (1400.0, 460.0)),
        ("out:pc", (1400.0, 580.0)),
        ("out:t", (1400.0, 700.0)),
        ("out:r", (1400.0, 820.0)),
    ];
    let positions = grid.iter().map(|(k, at)| (k.to_string(), *at)).collect();
    h.state_mut().state.ws.move_nodes("Crumb", positions);
    h.run_steps(2);
    h.hover_at(c);
    h.run_steps(1);
    h.key_press(Key::F);
    h.run_steps(3);

    // Decode's outputs, in order.
    let dec = |name: &str| {
        let outs = ["lit", "value", "jump", "jz", "call", "addr", "ret", "op", "t_to_n", "t_to_r", "store", "rd", "dd"];
        ("decode1", outs.iter().position(|o| *o == name).unwrap())
    };
    let (execute, pc, pc1) = (("sequencer1", 0), ("sequencer1", 1), ("sequencer1", 2));
    let (t, n, r) = (("register16_2", 0), ("stack1", 0), ("stack2", 0));
    // Step 2: the instruction register loads in fetch ticks, and Decode reads it.
    connect(&mut h, execute, ("not1", 0), None);
    connect(&mut h, ("in:mem_in", 0), ("register16_1", 0), None);
    connect(&mut h, ("not1", 0), ("register16_1", 1), None);
    connect(&mut h, ("in:reset", 0), ("register16_1", 2), None);
    connect(&mut h, ("register16_1", 0), ("decode1", 0), None);
    // Step 3: the Sequencer.
    connect(&mut h, ("in:reset", 0), ("sequencer1", 0), None);
    connect(&mut h, dec("jump"), ("sequencer1", 1), None);
    connect(&mut h, dec("jz"), ("sequencer1", 2), None);
    connect(&mut h, dec("ret"), ("sequencer1", 3), None);
    connect(&mut h, t, ("sequencer1", 4), None);
    connect(&mut h, r, ("sequencer1", 5), None);
    connect(&mut h, dec("addr"), ("sequencer1", 6), None);
    // Step 4: the ALU, and the new T: the ALU's answer, or a literal's value.
    connect(&mut h, t, ("alu16_1", 0), None);
    connect(&mut h, n, ("alu16_1", 1), None);
    connect(&mut h, r, ("alu16_1", 2), None);
    connect(&mut h, ("in:mem_in", 0), ("alu16_1", 3), None);
    connect(&mut h, dec("op"), ("alu16_1", 4), None);
    connect(&mut h, ("alu16_1", 0), ("mux1", 0), None);
    connect(&mut h, dec("value"), ("mux1", 1), None);
    connect(&mut h, dec("lit"), ("mux1", 2), None);
    connect(&mut h, ("mux1", 0), ("register16_2", 0), None);
    connect(&mut h, execute, ("register16_2", 1), None);
    connect(&mut h, ("in:reset", 0), ("register16_2", 2), None);
    // Step 5: the data stack pushes T.
    connect(&mut h, t, ("stack1", 0), None);
    connect(&mut h, dec("t_to_n"), ("stack1", 1), None);
    connect(&mut h, dec("dd"), ("stack1", 2), None);
    connect(&mut h, execute, ("stack1", 3), None);
    connect(&mut h, ("in:reset", 0), ("stack1", 4), None);
    // Step 6: the return stack pushes T, or the return address on a call.
    connect(&mut h, t, ("mux2", 0), None);
    connect(&mut h, pc1, ("mux2", 1), Some("mux2.b[12:0]"));
    connect(&mut h, dec("call"), ("mux2", 2), None);
    connect(&mut h, dec("t_to_r"), ("or1", 0), None);
    connect(&mut h, dec("call"), ("or1", 1), None);
    connect(&mut h, ("mux2", 0), ("stack2", 0), None);
    connect(&mut h, ("or1", 0), ("stack2", 1), None);
    connect(&mut h, dec("rd"), ("stack2", 2), None);
    connect(&mut h, execute, ("stack2", 3), None);
    connect(&mut h, ("in:reset", 0), ("stack2", 4), None);
    // Step 7: the bus: PC while fetching, T while executing; stores only in execute.
    connect(&mut h, t, ("mux3", 1), None);
    connect(&mut h, pc, ("mux3", 0), Some("mux3.a[12:0]"));
    connect(&mut h, execute, ("mux3", 2), None);
    connect(&mut h, dec("store"), ("and1", 0), None);
    connect(&mut h, execute, ("and1", 1), None);
    // Step 8: the outputs.
    for (from, to) in [
        (("mux3", 0), "out:addr"),
        (n, "out:mem_out"),
        (("and1", 0), "out:write"),
        (execute, "out:execute"),
        (pc, "out:pc"),
        (t, "out:t"),
        (r, "out:r"),
    ] {
        connect(&mut h, from, (to, 0), None);
    }
    shot(&mut h, "crumb");

    let text = chip_text(&dir, "Crumb.chip");
    for line in [
        "not1 = Not(sequencer1.execute)",
        "register16_1 = Register16(mem_in, not1, reset)",
        "decode1 = Decode(register16_1)",
        "sequencer1 = Sequencer(reset, decode1.jump, decode1.jz, decode1.ret, register16_2, stack2, decode1.addr)",
        "alu16_1 = ALU16(register16_2, stack1, stack2, mem_in, decode1.op)",
        "mux1 = Mux(alu16_1, decode1.value, decode1.lit)",
        "register16_2 = Register16(mux1, sequencer1.execute, reset)",
        "stack1 = Stack(register16_2, decode1.t_to_n, decode1.dd, sequencer1.execute, reset)",
        "mux2 = Mux(register16_2, {0b000, sequencer1.pc1}, decode1.call)",
        "or1 = Or(decode1.t_to_r, decode1.call)",
        "stack2 = Stack(mux2, or1, decode1.rd, sequencer1.execute, reset)",
        "mux3 = Mux({0b000, sequencer1.pc}, register16_2, sequencer1.execute)",
        "and1 = And(decode1.store, sequencer1.execute)",
    ] {
        assert!(text.contains(line), "missing `{line}`:\n{text}");
    }
    let ws = &h.state().state.ws;
    assert!(wafer_app::simrun::TestResults::run(ws, "Crumb").passed(), "Crumb's tests fail:\n{text}");
    // And it runs a real program exactly like the reference.
    let hello = std::fs::read_to_string(dir.join("hello.asm")).unwrap();
    let program = crumb::assemble(&hello).unwrap();
    let run = wafer_core::lockstep::lockstep(&ws.design, "Crumb", &program.words, 10_000, crumb::isa::TICKS_PER_FRAME)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(run.halted && run.instructions == 164, "{run:?}");
}

/// Builds Match16 and then AddressDecoder on the canvas the way the guide
/// in the chat says to.
#[test]
fn builds_match16_and_address_decoder() {
    let dir = example_project("address_decoder");
    reset_chip(&dir, "Match16", "  in  a[16], mask[16], value[16]\n  out eq\n");
    reset_chip(&dir, "AddressDecoder", "  in  addr[16]\n  out rom, ram, screen, io\n");
    let mut h = open_sized(&dir, egui::vec2(2000.0, 1400.0));
    let frame = |h: &mut Harness<'static, WaferApp>| {
        let c = h.state().state.canvas.rect().center();
        h.hover_at(c);
        h.run_steps(1);
        h.key_press(Key::F);
        h.run_steps(3);
    };

    // Part 1: Match16.
    h.state_mut().state.open_chip("Match16");
    h.run_steps(3);
    let c = h.state().state.canvas.rect().center();
    for (part, at) in [
        ("And", vec2(-300.0, -100.0)),
        ("Xor", vec2(-150.0, -100.0)),
        ("Or", vec2(0.0, -200.0)),
        ("Or", vec2(150.0, -200.0)),
        ("Or", vec2(0.0, 50.0)),
        ("Or", vec2(150.0, 50.0)),
        ("Not", vec2(300.0, -50.0)),
    ] {
        add(&mut h, c + at, part);
    }
    frame(&mut h);
    connect(&mut h, ("in:a", 0), ("and1", 0), None);
    connect(&mut h, ("in:mask", 0), ("and1", 1), None);
    connect(&mut h, ("and1", 0), ("xor1", 0), None);
    connect(&mut h, ("in:value", 0), ("xor1", 1), None);
    for (from, to, top, bottom) in [
        ("xor1", "or1", "xor1[15:8]", "xor1[7:0]"),
        ("or1", "or2", "or1[7:4]", "or1[3:0]"),
        ("or2", "or3", "or2[3:2]", "or2[1:0]"),
        ("or3", "or4", "or3[1]", "or3[0]"),
    ] {
        option_connect(&mut h, (from, 0), (to, 0), top);
        option_connect(&mut h, (from, 0), (to, 1), bottom);
    }
    connect(&mut h, ("or4", 0), ("not1", 0), None);
    connect(&mut h, ("not1", 0), ("out:eq", 0), None);
    let text = chip_text(&dir, "Match16.chip");
    for line in ["and1 = And(a, mask)", "xor1 = Xor(and1, value)", "or1 = Or(xor1[15:8], xor1[7:0])", "eq = not1"] {
        assert!(text.contains(line), "missing `{line}`:\n{text}");
    }
    assert!(wafer_app::simrun::TestResults::run(&h.state().state.ws, "Match16").passed(), "Match16 fails:\n{text}");

    // Part 2: AddressDecoder, four Match16s with numbers typed in.
    h.state_mut().state.open_chip("AddressDecoder");
    h.run_steps(3);
    let c = h.state().state.canvas.rect().center();
    for y in [-240.0, -80.0, 80.0, 240.0] {
        add(&mut h, c + vec2(0.0, y), "Match16");
    }
    frame(&mut h);
    let devices = [
        ("match16_1", "0xE000", None, "out:rom"),
        ("match16_2", "0xE000", Some("0x2000"), "out:ram"),
        ("match16_3", "0xFE00", Some("0x4000"), "out:screen"),
        ("match16_4", "0xFFF8", Some("0x6000"), "out:io"),
    ];
    for (matcher, mask, value, out) in devices {
        connect(&mut h, ("in:addr", 0), (matcher, 0), None);
        pin_number(&mut h, (matcher, 1), mask);
        match value {
            Some(value) => pin_number(&mut h, (matcher, 2), value),
            None => pin_menu(&mut h, (matcher, 2), "Set to 0"),
        }
        connect(&mut h, (matcher, 0), (out, 0), None);
    }
    shot(&mut h, "address_decoder");
    let text = chip_text(&dir, "AddressDecoder.chip");
    for line in [
        "match16_1 = Match16(addr, 0xE000, 0)",
        "match16_2 = Match16(addr, 0xE000, 0x2000)",
        "match16_3 = Match16(addr, 0xFE00, 0x4000)",
        "match16_4 = Match16(addr, 0xFFF8, 0x6000)",
        "rom = match16_1",
        "io = match16_4",
    ] {
        assert!(text.contains(line), "missing `{line}`:\n{text}");
    }
    let results = wafer_app::simrun::TestResults::run(&h.state().state.ws, "AddressDecoder");
    assert!(results.passed(), "AddressDecoder's tests fail:\n{text}");
}
