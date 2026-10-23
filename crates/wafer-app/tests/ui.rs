//! Drives the real app with simulated mouse and keyboard input. Each test
//! works on a fresh copy of a project in a temporary folder, and saves a
//! screenshot to `target/tmp/shots/` for a human (or Claude) to look at.

use std::path::{Path, PathBuf};

use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use wafer_app::app::WaferApp;
use wafer_core::Logic;

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
        if path.extension().is_some_and(|e| e == "chip") {
            std::fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
        }
    }
    dir
}

fn open(dir: &Path) -> Harness<'static, WaferApp> {
    let dir = dir.to_path_buf();
    let mut harness =
        Harness::builder().with_size(egui::vec2(1400.0, 860.0)).wgpu().build_eframe(move |cc| WaferApp::new(cc, dir));
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
    assert_eq!(h.state().state.canvas.chip, "Add4", "opens the most built-up chip");
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
