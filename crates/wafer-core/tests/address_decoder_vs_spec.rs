//! AddressDecoder, built in gates in `chips/`, against the memory map in
//! docs/crumb.md for all 65536 addresses. `WAFER_CHIPS` points it at
//! another folder.

use std::path::PathBuf;

use crumb::isa::{PAD, RAM_START, SCREEN, SCREEN_WORDS};
use wafer_core::logic::{bits_from_u64, bits_to_u64};
use wafer_core::{Design, Netlist, Project, Simulator};

#[test]
fn address_decoder_matches_the_memory_map() {
    let dir = std::env::var_os("WAFER_CHIPS")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../chips"), PathBuf::from);
    let design = Design::check(&Project::load(&dir).expect("the chips folder is readable"));
    let mut sim = Simulator::new(Netlist::build(&design, "AddressDecoder").expect("AddressDecoder builds"));
    for addr in 0..=u16::MAX {
        let want = [
            ("rom", addr < RAM_START),
            ("ram", (RAM_START..SCREEN).contains(&addr)),
            ("screen", (SCREEN..SCREEN + SCREEN_WORDS as u16).contains(&addr)),
            ("io", (PAD..PAD + 8).contains(&addr)),
        ];
        sim.set_input(0, &bits_from_u64(u64::from(addr), 16));
        sim.settle(10_000).expect("AddressDecoder settles");
        for (name, on) in want {
            let got = bits_to_u64(&sim.read(name).unwrap());
            assert_eq!(got, Some(u64::from(on)), "`{name}` for {addr:#06x}");
        }
    }
}
