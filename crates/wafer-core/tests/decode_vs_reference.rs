//! Decode, built in gates in `chips/`, against the reference Crumb's
//! decoder for all 65536 instructions. `WAFER_CHIPS` points it at another
//! folder.

use std::path::PathBuf;

use crumb::isa::Insn;
use wafer_core::logic::{bits_from_u64, bits_to_u64};
use wafer_core::{Design, Netlist, Project, Simulator};

/// What Decode should say about an instruction, in its output order after
/// `value` and `addr`: lit, jump, jz, call, ret, op, t_to_n, t_to_r, store,
/// rd, dd.
fn expected(word: u16) -> [u64; 11] {
    let alu_like = |op: u16, ret: bool, tn: bool, tr: bool, st: bool, rd: i8, dd: i8| {
        let two = |d: i8| u64::from((d as u8) & 3);
        [u64::from(ret), u64::from(op), u64::from(tn), u64::from(tr), u64::from(st), two(rd), two(dd)]
    };
    let (kinds, control) = match Insn::decode(word) {
        Insn::Lit(_) => ([1, 0, 0, 0], alu_like(0, false, true, false, false, 0, 1)),
        Insn::Jump(_) => ([0, 1, 0, 0], alu_like(0, false, false, false, false, 0, 0)),
        Insn::Jz(_) => ([0, 0, 1, 0], alu_like(1, false, false, false, false, 0, -1)),
        Insn::Call(_) => ([0, 1, 0, 1], alu_like(0, false, false, false, false, 1, 0)),
        Insn::Alu(a) => ([0, 0, 0, 0], alu_like(a.op.code(), a.ret, a.t_to_n, a.t_to_r, a.store, a.rd, a.dd)),
    };
    let mut out = [0; 11];
    out[..4].copy_from_slice(&kinds);
    out[4..].copy_from_slice(&control);
    out
}

#[test]
fn decode_matches_the_reference_crumb() {
    let dir = std::env::var_os("WAFER_CHIPS")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../chips"), PathBuf::from);
    let project = Project::load(&dir).expect("the chips folder is readable");
    let design = Design::check(&project);
    let mut sim = Simulator::new(Netlist::build(&design, "Decode").expect("Decode builds"));
    let names = ["lit", "jump", "jz", "call", "ret", "op", "t_to_n", "t_to_r", "store", "rd", "dd"];
    for word in 0..=u16::MAX {
        sim.set_input(0, &bits_from_u64(u64::from(word), 16));
        sim.settle(10_000).expect("Decode settles");
        let read = |name: &str| bits_to_u64(&sim.read(name).unwrap());
        for (name, want) in names.iter().zip(expected(word)) {
            assert_eq!(read(name), Some(want), "`{name}` for {word:#06x} ({})", crumb::isa::disasm(word));
        }
        if word >> 15 == 1 {
            assert_eq!(read("value"), Some(u64::from(word & 0x7FFF)), "`value` for {word:#06x}");
        } else if word >> 13 != 3 {
            assert_eq!(read("addr"), Some(u64::from(word & 0x1FFF)), "`addr` for {word:#06x}");
        }
    }
}
