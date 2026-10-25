//! FrameCounter, built in gates in `chips/`, against the reference Crumb's
//! frame counter after every tick, with resets now and then.
//! `WAFER_CHIPS` points it at another folder.

use std::path::PathBuf;

use crumb::Machine;
use crumb::isa::TICKS_PER_FRAME;
use wafer_core::logic::{bits_from_u64, bits_to_u64};
use wafer_core::{Design, Netlist, Project, Simulator};

#[test]
fn frame_counter_matches_the_reference_crumb() {
    let dir = std::env::var_os("WAFER_CHIPS")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../chips"), PathBuf::from);
    let design = Design::check(&Project::load(&dir).expect("the chips folder is readable"));
    let mut sim = Simulator::new(Netlist::build(&design, "FrameCounter").expect("FrameCounter builds"));
    let mut reference = Machine::new(&[]);
    let reset = |sim: &mut Simulator, reference: &mut Machine| {
        sim.set_input_by_name("reset", &bits_from_u64(1, 1));
        sim.tick(100_000).expect("FrameCounter settles");
        sim.set_input_by_name("reset", &bits_from_u64(0, 1));
        reference.reset();
    };
    reset(&mut sim, &mut reference);
    // Three frames and a bit, a reset part way through a frame, then two
    // frames more.
    let resets = [3 * TICKS_PER_FRAME + 1000];
    for tick in 0..6 * TICKS_PER_FRAME {
        if resets.contains(&tick) {
            reset(&mut sim, &mut reference);
        }
        sim.tick(100_000).expect("FrameCounter settles");
        reference.tick();
        let got = bits_to_u64(&sim.read("frame").expect("FrameCounter has `out frame`"));
        assert_eq!(
            got,
            Some(u64::from(reference.frame())),
            "`frame` after {} ticks since the last reset",
            reference.ticks
        );
    }
}
