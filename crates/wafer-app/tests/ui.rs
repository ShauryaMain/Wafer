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
    let dir = dir.to_path_buf();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1400.0, 860.0))
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
    assert_eq!(h.state().state.canvas.chip, "Counter8", "opens the most built-up chip");
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
