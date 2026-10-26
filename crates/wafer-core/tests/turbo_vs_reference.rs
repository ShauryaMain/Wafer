//! The Console, built in gates in `chips/`, compiled for turbo mode and run
//! next to the reference Crumb: the screen, RAM, LEDs, number display and
//! printed text must match. Turbo's state handed back to the normal
//! simulator must carry on matching too. `WAFER_CHIPS` points it at
//! another folder.

use std::path::PathBuf;
use std::time::Instant;

use crumb::Machine;
use crumb::isa::TICKS_PER_FRAME;
use wafer_core::lockstep::{ConsoleParts, load_program};
use wafer_core::logic::{bits_from_u64, bits_to_u64};
use wafer_core::turbo::Turbo;
use wafer_core::{Design, Netlist, Project, Simulator};

fn chips_dir() -> PathBuf {
    std::env::var_os("WAFER_CHIPS")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../chips"), PathBuf::from)
}

/// The Console with `program` in its Rom, reset in the normal simulator.
struct Setup {
    netlist: Netlist,
    parts: ConsoleParts,
    sim: Simulator,
    reference: Machine,
}

fn setup(design: &Design, program: &[u16]) -> Setup {
    let netlist = Netlist::build(design, "Console").expect("the Console builds");
    let parts = ConsoleParts::find(design, "Console", &netlist).expect("it's a console");
    let mut sim = Simulator::new(netlist.clone());
    load_program(&mut sim, parts.rom, program);
    sim.set_input_by_name("reset", &bits_from_u64(1, 1));
    sim.set_input_by_name("pad", &bits_from_u64(0, 8));
    sim.set_input_by_name("switches", &bits_from_u64(0, 16));
    sim.tick(100_000).expect("it settles");
    sim.set_input_by_name("reset", &bits_from_u64(0, 1));
    Setup { netlist, parts, sim, reference: Machine::new(program) }
}

fn output(netlist: &Netlist, name: &str) -> Vec<u32> {
    netlist.outputs.iter().find(|o| o.path == name).expect("a console's output").nets.clone()
}

/// Runs turbo for `ticks`, catching printed characters.
fn run(turbo: &mut Turbo, netlist: &Netlist, ticks: u64, printed: &mut Vec<u8>) {
    let (print, char) = (output(netlist, "print"), output(netlist, "char"));
    for _ in 0..ticks {
        turbo.settle();
        if turbo.bit(print[0]) {
            printed.push(turbo.read(&char) as u8);
        }
        turbo.clock_edge();
    }
    turbo.settle();
}

fn compare(name: &str, turbo: &Turbo, setup: &Setup, printed: &[u8]) {
    let (netlist, reference) = (&setup.netlist, &setup.reference);
    let words = |ram: usize| turbo.memory(ram).expect("the memory").iter().map(|&w| w as u16).collect::<Vec<u16>>();
    let screen = words(setup.parts.screen);
    let lit = |s: &[u16]| s.iter().map(|w| w.count_ones()).sum::<u32>();
    assert!(
        screen == reference.screen,
        "{name}: the screens differ ({} pixels lit, the reference has {})",
        lit(&screen),
        lit(&reference.screen)
    );
    assert!(words(setup.parts.ram) == reference.ram, "{name}: RAM differs");
    assert_eq!(turbo.read(&output(netlist, "number")), u64::from(reference.number), "{name}: the number display");
    assert_eq!(turbo.read(&output(netlist, "leds")), u64::from(reference.leds), "{name}: the LEDs");
    assert_eq!(String::from_utf8_lossy(printed), reference.console_text(), "{name}: what's printed");
}

#[test]
fn turbo_plays_programs_like_the_reference() {
    let design = Design::check(&Project::load(&chips_dir()).expect("the chips folder is readable"));
    let read = |file: &str| std::fs::read_to_string(chips_dir().join(file)).expect("the program is readable");
    // Pong serves its first ball at frame 40, so 60 frames see it in play.
    for (name, frames) in [("hello.asm", 1), ("count.asm", 5), ("bounce.asm", 30), ("buttons.asm", 5), ("pong.asm", 60)]
    {
        let program = crumb::assemble(&read(name)).expect("it assembles").words;
        let mut setup = setup(&design, &program);
        let mut turbo = Turbo::compile(&setup.netlist).expect("the Console compiles");
        turbo.load(&setup.sim);
        let start = Instant::now();
        let mut printed = Vec::new();
        let ticks = frames * TICKS_PER_FRAME;
        run(&mut turbo, &setup.netlist, ticks, &mut printed);
        let seconds = start.elapsed().as_secs_f64();
        setup.reference.run_frames(frames);
        compare(name, &turbo, &setup, &printed);
        eprintln!(
            "{name}: {frames} frames match ({:.0} ticks/s, {} ops a tick)",
            ticks as f64 / seconds,
            turbo.op_count()
        );
    }
}

#[test]
fn the_gamepad_reaches_turbo() {
    let design = Design::check(&Project::load(&chips_dir()).expect("the chips folder is readable"));
    let source = std::fs::read_to_string(chips_dir().join("buttons.asm")).expect("buttons.asm is readable");
    let program = crumb::assemble(&source).expect("it assembles").words;
    let mut setup = setup(&design, &program);
    let mut turbo = Turbo::compile(&setup.netlist).expect("the Console compiles");
    turbo.load(&setup.sim);
    let pad = setup.netlist.inputs.iter().position(|i| i.path == "pad").expect("a pad input");
    // Hold right and A for four frames.
    turbo.set_input(pad, 8 | 16);
    setup.reference.pad = 8 | 16;
    run(&mut turbo, &setup.netlist, 4 * TICKS_PER_FRAME, &mut Vec::new());
    setup.reference.run_frames(4);
    compare("buttons.asm held", &turbo, &setup, &[]);
}

#[test]
fn pong_on_turbo_follows_your_paddle() {
    let design = Design::check(&Project::load(&chips_dir()).expect("the chips folder is readable"));
    let source = std::fs::read_to_string(chips_dir().join("pong.asm")).expect("pong.asm is readable");
    let program = crumb::assemble(&source).expect("it assembles").words;
    let mut setup = setup(&design, &program);
    let mut turbo = Turbo::compile(&setup.netlist).expect("the Console compiles");
    turbo.load(&setup.sim);
    let pad = setup.netlist.inputs.iter().position(|i| i.path == "pad").expect("a pad input");
    // Down for twenty frames, then up for twenty, with the ball served.
    for (buttons, frames) in [(2, 20), (1, 20), (0, 15)] {
        turbo.set_input(pad, buttons);
        setup.reference.pad = buttons as u16;
        run(&mut turbo, &setup.netlist, frames * TICKS_PER_FRAME, &mut Vec::new());
        setup.reference.run_frames(frames);
        compare("pong.asm", &turbo, &setup, &[]);
    }
}

#[test]
fn turbo_hands_its_state_back() {
    let design = Design::check(&Project::load(&chips_dir()).expect("the chips folder is readable"));
    let source = std::fs::read_to_string(chips_dir().join("bounce.asm")).expect("bounce.asm is readable");
    let program = crumb::assemble(&source).expect("it assembles").words;
    let mut setup = setup(&design, &program);
    let mut turbo = Turbo::compile(&setup.netlist).expect("the Console compiles");
    turbo.load(&setup.sim);
    // Three frames in turbo, then hand back to the normal simulator...
    run(&mut turbo, &setup.netlist, 3 * TICKS_PER_FRAME, &mut Vec::new());
    let ticks_before = setup.sim.ticks();
    turbo.snapshot().apply(&mut setup.sim).expect("it settles");
    assert_eq!(setup.sim.ticks(), ticks_before + 3 * TICKS_PER_FRAME);
    // ...which carries on for a frame, exactly like the reference.
    for _ in 0..TICKS_PER_FRAME {
        setup.sim.tick(100_000).expect("it settles");
    }
    setup.reference.run_frames(4);
    let screen: Vec<u16> = (setup.sim.memory(setup.parts.screen).chunks(16))
        .map(|w| bits_to_u64(w).expect("no unknown pixels") as u16)
        .collect();
    assert!(screen == setup.reference.screen, "the screen after handing back");
    let pc = bits_to_u64(&setup.sim.read("pc").unwrap());
    assert_eq!(pc, Some(u64::from(setup.reference.pc)), "and the PC");
}
