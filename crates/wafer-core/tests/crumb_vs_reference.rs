//! Crumb, built in gates in `chips/`, run next to the reference Crumb on
//! real programs: after every instruction, PC, T, N, R and the ports must
//! match. `WAFER_CHIPS` points it at another folder.

use std::path::PathBuf;

use wafer_core::lockstep::lockstep;
use wafer_core::{Design, Project};

fn chips_dir() -> PathBuf {
    std::env::var_os("WAFER_CHIPS")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../chips"), PathBuf::from)
}

fn design() -> Design {
    Design::check(&Project::load(&chips_dir()).expect("the chips folder is readable"))
}

/// Runs `source` both ways for up to `max` instructions.
fn matches(design: &Design, name: &str, source: &str, max: u64, ticks_per_frame: u64) {
    let program = crumb::assemble(source).unwrap_or_else(|e| panic!("{name} doesn't assemble: {e:#?}"));
    match lockstep(design, "Crumb", &program.words, max, ticks_per_frame) {
        Ok(run) => eprintln!(
            "{name}: {} instructions match ({:.0} instructions/s)",
            run.instructions,
            run.instructions as f64 / run.seconds.max(1e-9)
        ),
        Err(failure) => panic!("{name}: {failure}"),
    }
}

#[test]
fn instructions_match_the_reference() {
    let design = design();
    let stack_wraps = format!(": main{} halt ;", " 1".repeat(40));
    let programs: [(&str, &str); 9] = [
        (
            "arithmetic",
            ": main 2 3 + 10 3 - 3 10 - 0b1100 0b1010 and 0b1100 0b1010 or 0b1100 0b1010 xor \
             0 invert 5 1+ 5 1- 5 negate 1 4 lshift 0x8000 15 rshift 3 2* 12 2/ halt ;",
        ),
        ("comparisons", ": main 1 2 < 2 1 < 2 2 = -1 1 < -1 1 u< 2 1 > 1 2 u> 1 2 <> 0 0= 7 0= halt ;"),
        ("big numbers", ": main 0x7FFF 0x8000 0xFFFF -1 40000 -32768 halt ;"),
        ("stack words", ": main 1 2 3 rot tuck 2dup over nip swap drop 5 >r 6 r@ r> halt ;"),
        (
            "memory",
            "var x\nvar buf[4]\nvar y\n\
             : main 42 x ! 7 y ! x @ 1+ buf 2 + ! 5 y +! y @ NUMBER ! 0x00F0 LEDS ! LEDS @ halt ;",
        ),
        (
            "control",
            ": main 0 10 for i + next 1 if 10 else 20 then 0 if 10 else 20 then \
             0 100 begin dup while 2/ swap 1+ swap repeat drop halt ;",
        ),
        (
            "calls",
            ": square dup 0 swap for over + next nip ;\n\
             : fib dup 2 < if exit then dup 1- fib swap 2 - fib + ;\n\
             : main 7 square 6 fib NUMBER ! halt ;",
        ),
        ("tables", "table squares 0 1 4 9 16 end\n: main squares 3 + @ NUMBER ! halt ;"),
        ("stack wraps", &stack_wraps),
    ];
    for (name, source) in programs {
        matches(&design, name, source, 10_000, crumb::isa::TICKS_PER_FRAME);
    }
}

#[test]
fn example_programs_match_the_reference() {
    let design = design();
    let read = |file: &str| std::fs::read_to_string(chips_dir().join(file)).expect("the program is readable");
    matches(&design, "hello.asm", &read("hello.asm"), 10_000, crumb::isa::TICKS_PER_FRAME);
    // Short frames, so the frame waits don't take long.
    matches(&design, "count.asm", &read("count.asm"), 600, 64);
    matches(&design, "bounce.asm", &read("bounce.asm"), 1500, 256);
}
