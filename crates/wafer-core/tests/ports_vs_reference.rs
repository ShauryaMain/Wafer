//! Ports, built in gates in `chips/`, against the reference Crumb's ports
//! over a few thousand random reads and writes. `WAFER_CHIPS` points it at
//! another folder.

use std::path::PathBuf;

use crumb::Machine;
use crumb::isa::{CONSOLE, PAD};
use wafer_core::logic::{bits_from_u64, bits_to_u64};
use wafer_core::{Design, Netlist, Project, Simulator};

#[test]
fn ports_match_the_reference_crumb() {
    let dir = std::env::var_os("WAFER_CHIPS")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../chips"), PathBuf::from);
    let design = Design::check(&Project::load(&dir).expect("the chips folder is readable"));
    let mut sim = Simulator::new(Netlist::build(&design, "Ports").expect("Ports builds"));
    let set = |sim: &mut Simulator, name: &str, value: u16, width: usize| {
        assert!(sim.set_input_by_name(name, &bits_from_u64(u64::from(value), width)), "Ports has no `in {name}`");
    };
    let read = |sim: &Simulator, name: &str| {
        bits_to_u64(&sim.read(name).unwrap_or_else(|| panic!("Ports has no `out {name}`"))).map(|v| v as u16)
    };

    // The reference keeps the LEDs, number and console, and counts frames
    // and steps its random numbers; the gates are handed those two.
    let mut reference = Machine::new(&[]);
    reference.ticks_per_frame = 1;
    for (name, width) in [("addr", 16), ("io", 1), ("write", 1), ("data", 16), ("pad", 8)] {
        set(&mut sim, name, 0, width);
    }
    for name in ["frame", "random", "switches"] {
        set(&mut sim, name, 0, 16);
    }
    set(&mut sim, "reset", 1, 1);
    sim.tick(100_000).expect("Ports settles");
    set(&mut sim, "reset", 0, 1);

    // xorshift: the same "random" steps every run.
    let mut seed = 0x9E37_79B9_u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for step in 0..3000 {
        // Mostly the ports, sometimes anywhere else.
        let addr = if next() % 4 == 0 { next() as u16 } else { PAD + (next() % 8) as u16 };
        let io = (PAD..PAD + 8).contains(&addr);
        let write = next() % 2 == 0;
        let data = next() as u16;
        reference.pad = (next() & 0xFF) as u16;
        reference.switches = next() as u16;
        for _ in 0..next() % 4 {
            reference.tick();
        }
        set(&mut sim, "addr", addr, 16);
        set(&mut sim, "io", u16::from(io), 1);
        set(&mut sim, "write", u16::from(write), 1);
        set(&mut sim, "data", data, 16);
        set(&mut sim, "pad", reference.pad, 8);
        set(&mut sim, "frame", reference.frame(), 16);
        set(&mut sim, "random", reference.random(), 16);
        set(&mut sim, "switches", reference.switches, 16);
        sim.settle(100_000).expect("Ports settles");

        let what = format!(
            "step {step}: {} {addr:#06x}{}",
            if write { "write" } else { "read" },
            if write { format!(" <- {data:#06x}") } else { String::new() }
        );
        let want = if io { reference.read(addr) } else { 0 };
        assert_eq!(read(&sim, "out"), Some(want), "`out` after {what}");
        let printing = write && addr == CONSOLE;
        assert_eq!(read(&sim, "print"), Some(u16::from(printing)), "`print` after {what}");
        if printing {
            assert_eq!(read(&sim, "char"), Some(data & 0xFF), "`char` after {what}");
        }

        sim.tick(100_000).expect("Ports settles");
        if write && io {
            reference.write(addr, data);
        }
        assert_eq!(read(&sim, "leds"), Some(reference.leds), "`leds` after {what}");
        assert_eq!(read(&sim, "number"), Some(reference.number), "`number` after {what}");
    }
}
