//! Random, built in gates in `chips/`, against the reference Crumb's LFSR
//! after every tick: once all the way round (65535 ticks) and a bit, then
//! again after a reset. `WAFER_CHIPS` points it at another folder.

use std::path::PathBuf;

use crumb::Machine;
use wafer_core::logic::{bits_from_u64, bits_to_u64};
use wafer_core::{Design, Netlist, Project, Simulator};

#[test]
fn random_matches_the_reference_crumb() {
    let dir = std::env::var_os("WAFER_CHIPS")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../chips"), PathBuf::from);
    let design = Design::check(&Project::load(&dir).expect("the chips folder is readable"));
    let mut sim = Simulator::new(Netlist::build(&design, "Random").expect("Random builds"));
    let mut reference = Machine::new(&[]);
    let reset = |sim: &mut Simulator, reference: &mut Machine| {
        sim.set_input_by_name("reset", &bits_from_u64(1, 1));
        sim.tick(100_000).expect("Random settles");
        sim.set_input_by_name("reset", &bits_from_u64(0, 1));
        reference.reset();
    };
    let check = |sim: &Simulator, reference: &Machine| {
        let got = bits_to_u64(&sim.read("random").expect("Random has `out random`"));
        assert_eq!(
            got,
            Some(u64::from(reference.random())),
            "`random` after {} ticks since the last reset",
            reference.ticks
        );
    };
    reset(&mut sim, &mut reference);
    check(&sim, &reference);
    for tick in 0..70_000 {
        if tick == 66_000 {
            reset(&mut sim, &mut reference);
            check(&sim, &reference);
        }
        sim.tick(100_000).expect("Random settles");
        reference.tick();
        check(&sim, &reference);
    }
}
