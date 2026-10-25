//! A chip whose ports are all 1 bit works on buses bit by bit, like a
//! built-in gate.

use wafer_core::check::Callee;
use wafer_core::logic::{bits_from_u64, bits_to_u64};
use wafer_core::{Design, Netlist, Project, Simulator, schematic};

/// Xor from four Nands, like `chips/Xor.chip`, plus our own 1-bit adders.
const PARTS: &str = "\
chip Not {\n in a\n out out\n out = Nand(a, a)\n}\n\
chip Xor {\n in a, b\n out out\n n1 = Nand(a, b)\n n2 = Nand(a, n1)\n n3 = Nand(b, n1)\n out = Nand(n2, n3)\n}\n\
chip And {\n in a, b\n out out\n n = Nand(a, b)\n out = Nand(n, n)\n}\n\
chip HalfAdder {\n in a, b\n out sum, carry\n sum = Xor(a, b)\n carry = And(a, b)\n}\n";

fn design(chip: &str) -> Design {
    let project = Project::from_sources(vec![("parts.chip".into(), PARTS.into()), ("t.chip".into(), chip.into())]);
    assert!(project.diagnostics.is_empty(), "{:#?}", project.diagnostics);
    Design::check(&project)
}

fn errors(chip: &str) -> Vec<String> {
    design(chip).diagnostics.iter().filter(|d| d.is_error()).map(|d| d.message.clone()).collect()
}

/// Sets the inputs (in order), settles, and reads `signal`.
fn run(design: &Design, chip: &str, inputs: &[(u64, usize)], signal: &str) -> Option<u64> {
    let mut sim = Simulator::new(Netlist::build(design, chip).unwrap());
    for (i, &(value, width)) in inputs.iter().enumerate() {
        sim.set_input(i, &bits_from_u64(value, width));
    }
    sim.settle(10_000).unwrap();
    bits_to_u64(&sim.read(signal)?)
}

#[test]
fn a_1_bit_chip_spreads_across_a_bus() {
    let d = design("chip T {\n in a[16], b[16]\n out y[16]\n y = Xor(a, b)\n}");
    assert!(!d.has_errors(), "{:#?}", d.diagnostics);
    assert_eq!(d.chips["T"].instances[0].callee, Callee::Chip { name: "Xor".into(), copies: 16 });
    assert_eq!(run(&d, "T", &[(0xF0F0, 16), (0xFF00, 16)], "y"), Some(0x0FF0));
}

#[test]
fn a_1_bit_input_feeds_every_copy() {
    let d = design("chip T {\n in a[8], en\n out y[8]\n y = And(a, en)\n}");
    assert!(!d.has_errors(), "{:#?}", d.diagnostics);
    assert_eq!(run(&d, "T", &[(0xA5, 8), (1, 1)], "y"), Some(0xA5));
    assert_eq!(run(&d, "T", &[(0xA5, 8), (0, 1)], "y"), Some(0));
}

#[test]
fn copies_follow_the_wires_and_numbers() {
    // Through a wire whose width comes from another box.
    let d = design("chip T {\n in a[4], b[4]\n out y[4]\n x = Xor(a, b)\n y = Xor(x, a)\n}");
    assert!(!d.has_errors(), "{:#?}", d.diagnostics);
    assert_eq!(run(&d, "T", &[(0b1100, 4), (0b1010, 4)], "y"), Some(0b1010));
    // Only numbers: as wide as the widest.
    let d = design("chip T {\n out y[8]\n y = And(0xFF, 0x0F)\n}");
    assert!(!d.has_errors(), "{:#?}", d.diagnostics);
    // Nothing wired yet: one copy.
    let d = design("chip T {\n out y\n y = Xor()\n}");
    assert!(!d.has_errors(), "{:#?}", d.diagnostics);
    // A gate fed by a spread chip that also takes numbers: the signal sets
    // the copies, and the gate takes the chip's width.
    let d = design("chip T {\n in x[16], s\n out y[16]\n m = And(0x00FF, x)\n y = Mux(m, 0x0081, s)\n}");
    assert!(!d.has_errors(), "{:#?}", d.diagnostics);
    assert_eq!(run(&d, "T", &[(0x1234, 16), (0, 1)], "y"), Some(0x0034));
    // Only 1-bit signals: the widest number sets the copies.
    let d = design("chip T {\n in s\n out y[3]\n m = And(0b101, s)\n y = Mux(m, 0, s)\n}");
    assert!(!d.has_errors(), "{:#?}", d.diagnostics);
    assert_eq!(run(&d, "T", &[(1, 1)], "y"), Some(0));
    assert_eq!(run(&d, "T", &[(0, 1)], "y"), Some(0));
}

#[test]
fn bus_inputs_have_to_match() {
    let messages = errors("chip T {\n in a[16], b[8]\n out y[16]\n y = Xor(a, b)\n}");
    assert_eq!(messages, ["`Xor` input `b` is 16 bits, but this is 8"]);
    // Chips with a bus port of their own don't spread.
    let messages = errors("chip W {\n in a[2]\n out y[2]\n y = a\n}\nchip T {\n in a[4]\n out y[4]\n y = W(a)\n}");
    assert!(messages.iter().any(|m| m.contains("is 2 bits, but this is 4")), "{messages:?}");
}

#[test]
fn several_outputs_spread_too() {
    // On buses a HalfAdder is separate adders: no carry between bits.
    let d = design("chip T {\n in a[4], b[4]\n out s[4], c[4]\n h = HalfAdder(a, b)\n s = h.sum\n c = h.carry\n}");
    assert!(!d.has_errors(), "{:#?}", d.diagnostics);
    assert_eq!(run(&d, "T", &[(0b0011, 4), (0b0101, 4)], "s"), Some(0b0110));
    assert_eq!(run(&d, "T", &[(0b0011, 4), (0b0101, 4)], "c"), Some(0b0001));
}

#[test]
fn inside_every_signal_is_as_wide_as_the_bus() {
    let d = design("chip T {\n in a[16], b[16]\n out y[16]\n y = Xor(a, b)\n}");
    let mut sim = Simulator::new(Netlist::build(&d, "T").unwrap());
    sim.set_input(0, &bits_from_u64(0x00FF, 16));
    sim.set_input(1, &bits_from_u64(0x0F0F, 16));
    sim.settle(10_000).unwrap();
    // n1 = Nand(a, b), bit by bit.
    assert_eq!(bits_to_u64(&sim.read("y.n1").unwrap()), Some(!(0x00FF & 0x0F0F) & 0xFFFF));
    assert_eq!(bits_to_u64(&sim.read("y.a").unwrap()), Some(0x00FF));
    // The schematic draws the inside 16 bits wide, and the box says so.
    let project = Project::from_sources(vec![("parts.chip".into(), PARTS.into())]);
    let inside = schematic::layout_copies(&d, &project.sources, "Xor", 16).unwrap();
    assert!(inside.wires.iter().all(|w| w.width == 16));
    let outside = schematic::layout(&d, &project.sources, "T").unwrap();
    let node = outside.nodes.iter().find(|n| n.key == "y").unwrap();
    assert!(matches!(&node.kind, schematic::NodeKind::Chip { copies: 16, .. }));
    assert!(node.inputs.iter().all(|p| p.width == 16));
}

#[test]
fn a_loop_through_a_bitwise_chip_settles_at_one_bit() {
    // Nothing in this loop has a width of its own: Not spreads to whatever
    // it's given, and Mux and Dff take what they're given.
    let d = design("chip T {\n in reset\n out q\n n = Not(d)\n m = Mux(n, 0, reset)\n d = Dff(m)\n q = d\n}");
    assert!(!d.has_errors(), "{:#?}", d.diagnostics);
    let mut sim = Simulator::new(Netlist::build(&d, "T").unwrap());
    sim.set_input(0, &bits_from_u64(1, 1));
    sim.tick(1000).unwrap();
    sim.set_input(0, &bits_from_u64(0, 1));
    let mut seen = Vec::new();
    for _ in 0..4 {
        sim.tick(1000).unwrap();
        seen.push(bits_to_u64(&sim.read("q").unwrap()));
    }
    assert_eq!(seen, [Some(1), Some(0), Some(1), Some(0)], "it toggles every tick");
}
