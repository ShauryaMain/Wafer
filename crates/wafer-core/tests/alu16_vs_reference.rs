//! ALU16, built in gates in `chips/`, against the reference Crumb's ALU on
//! random inputs. `WAFER_CHIPS` points it at another folder.

use std::path::PathBuf;

use crumb::isa::Op;
use wafer_core::logic::{bits_from_u64, bits_to_u64};
use wafer_core::{Design, Netlist, Project, Simulator};

#[test]
fn alu16_matches_the_reference_crumb() {
    let dir = std::env::var_os("WAFER_CHIPS")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../chips"), PathBuf::from);
    let project = Project::load(&dir).expect("the chips folder is readable");
    let design = Design::check(&project);
    let mut sim = Simulator::new(Netlist::build(&design, "ALU16").expect("ALU16 builds"));

    // xorshift: the same "random" inputs every run.
    let mut seed = 0x2545_F491_u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as u16
    };
    for i in 0..400u16 {
        let op = i % 16;
        // Small T now and then, so shifts and T - 1 near 0 get tried.
        let t = if i % 3 == 0 { next() & 0x1F } else { next() };
        let (n, r, mem) = (next(), next(), next());
        for (port, value, width) in [(0, t, 16), (1, n, 16), (2, r, 16), (3, mem, 16), (4, op, 4)] {
            sim.set_input(port, &bits_from_u64(u64::from(value), width));
        }
        sim.settle(100_000).expect("ALU16 settles");
        let got = bits_to_u64(&sim.read("out").unwrap()).map(|v| v as u16);
        let want = Op::from_code(op).eval(t, n, r, mem);
        assert_eq!(
            got,
            Some(want),
            "op {op} ({}) with t={t:#06x} n={n:#06x} r={r:#06x} mem={mem:#06x}",
            Op::from_code(op).symbol()
        );
    }
}
