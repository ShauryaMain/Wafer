//! The Console, built in gates in `chips/`, run next to the reference
//! Crumb: each program is loaded into its Rom in place of count.asm, and
//! after every instruction PC, T, N, R and the ports must match, then RAM
//! and the screen at the end. `WAFER_CHIPS` points it at another folder.

use std::path::PathBuf;

use wafer_core::lockstep::lockstep;
use wafer_core::{Design, Project};

fn chips_dir() -> PathBuf {
    std::env::var_os("WAFER_CHIPS")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../chips"), PathBuf::from)
}

/// Runs `source` both ways for up to `max` instructions.
fn matches(design: &Design, name: &str, source: &str, max: u64) {
    let program = crumb::assemble(source).unwrap_or_else(|e| panic!("{name} doesn't assemble: {e:#?}"));
    // The frame length is in the Console's gates, so the last argument
    // doesn't matter here.
    match lockstep(design, "Console", &program.words, max, 0) {
        Ok(run) => eprintln!(
            "{name}: {} instructions match ({:.0} instructions/s)",
            run.instructions,
            run.instructions as f64 / run.seconds.max(1e-9)
        ),
        Err(failure) => panic!("{name}: {failure}"),
    }
}

#[test]
fn the_console_runs_programs_like_the_reference() {
    let design = Design::check(&Project::load(&chips_dir()).expect("the chips folder is readable"));
    let read = |file: &str| std::fs::read_to_string(chips_dir().join(file)).expect("the program is readable");
    let programs: [(&str, &str, u64); 5] = [
        // Every device: RAM, the screen, tables in the ROM, every port, and
        // addresses where nothing is (they read 0).
        (
            "memory",
            "var x\nvar buf[4]\n: main 42 x ! 7 buf 3 + ! x @ buf 3 + @ + NUMBER ! 0x1234 SCREEN ! \
             0xFFFF SCREEN 511 + ! SCREEN @ SCREEN 511 + @ halt ;",
            10_000,
        ),
        ("tables", "table squares 0 1 4 9 16 end\n: main squares 3 + @ squares 4 + @ + LEDS ! halt ;", 10_000),
        (
            "ports",
            ": main PAD @ FRAME @ RANDOM @ LEDS @ SWITCHES @ NUMBER @ CONSOLE @ 0x6007 @ \
             0x4200 @ 0x5FFF @ 0x6008 @ 0x8000 @ 0xFFFF @ 33 CONSOLE ! halt ;",
            10_000,
        ),
        ("hello.asm", &read("hello.asm"), 10_000),
        ("bounce.asm", &read("bounce.asm"), 3_000),
    ];
    for (name, source, max) in programs {
        matches(&design, name, source, max);
    }
}

#[test]
fn the_console_counts_frames_like_the_reference() {
    // count.asm (the program the Console's Rom holds) over three frames.
    let design = Design::check(&Project::load(&chips_dir()).expect("the chips folder is readable"));
    let source = std::fs::read_to_string(chips_dir().join("count.asm")).expect("count.asm is readable");
    matches(&design, "count.asm", &source, 3 * crumb::isa::TICKS_PER_FRAME / crumb::isa::TICKS_PER_INSTRUCTION);
}
