//! Runs a gate-level Crumb next to the reference Crumb, one instruction at
//! a time, and stops at the first place they differ. The gate-level CPU gets
//! its memory and ports from a reference [`Machine`], so it can be checked
//! before the gate-level Console exists.

use std::fmt;
use std::time::Instant;

use crumb::Machine;
use crumb::isa::disasm;

use crate::check::Design;
use crate::logic::{bits_from_u64, bits_to_u64};
use crate::netlist::Netlist;
use crate::sim::Simulator;

/// Gate delays a tick may take before it counts as stuck.
const MAX_STEPS: u64 = 100_000;

/// The ports a CPU chip needs to be run this way, and their widths.
pub const INPUTS: [(&str, usize); 2] = [("reset", 1), ("mem_in", 16)];
pub const OUTPUTS: [(&str, usize); 7] =
    [("addr", 16), ("mem_out", 16), ("write", 1), ("execute", 1), ("pc", 13), ("t", 16), ("r", 16)];

/// A gate-level Crumb wired to a reference machine's memory and ports.
pub struct GateCrumb {
    sim: Simulator,
    reset: usize,
    mem_in: usize,
    /// Everything outside the CPU: memory, screen and ports.
    pub board: Machine,
}

impl GateCrumb {
    /// Builds `chip`, loads `program` into the board's ROM, and resets.
    pub fn new(design: &Design, chip: &str, program: &[u16]) -> Result<GateCrumb, String> {
        let netlist = Netlist::build(design, chip).map_err(|e| e.to_string())?;
        let width = |signals: &[crate::netlist::Signal], name: &str| {
            signals.iter().position(|s| s.path == name).map(|i| (i, signals[i].nets.len()))
        };
        let mut missing = Vec::new();
        for (name, bits) in INPUTS {
            if width(&netlist.inputs, name).map(|(_, w)| w) != Some(bits) {
                missing.push(format!("in {name}{}", if bits > 1 { format!("[{bits}]") } else { String::new() }));
            }
        }
        for (name, bits) in OUTPUTS {
            if width(&netlist.outputs, name).map(|(_, w)| w) != Some(bits) {
                missing.push(format!("out {name}{}", if bits > 1 { format!("[{bits}]") } else { String::new() }));
            }
        }
        if !missing.is_empty() {
            return Err(format!("`{chip}` needs these ports to run as Crumb: {}", missing.join(", ")));
        }
        let reset = width(&netlist.inputs, "reset").expect("checked").0;
        let mem_in = width(&netlist.inputs, "mem_in").expect("checked").0;
        let mut gates = GateCrumb { sim: Simulator::new(netlist), reset, mem_in, board: Machine::new(program) };
        gates.sim.set_input(gates.reset, &bits_from_u64(1, 1));
        gates.sim.set_input(gates.mem_in, &bits_from_u64(0, 16));
        gates.sim.tick(MAX_STEPS).map_err(|_| "the gates never settled during reset".to_string())?;
        gates.sim.set_input(gates.reset, &bits_from_u64(0, 1));
        Ok(gates)
    }

    /// An output as a number, or `None` if any bit is X or Z.
    pub fn read(&self, name: &str) -> Option<u64> {
        bits_to_u64(&self.sim.read(name)?)
    }

    /// One clock tick: the board answers the address on the bus, and a
    /// write lands at the end of the tick.
    pub fn tick(&mut self) -> Result<(), String> {
        let stuck = |_| "the gates never settled".to_string();
        self.sim.settle(MAX_STEPS).map_err(stuck)?;
        let addr = self.read("addr").ok_or("the bus address is unknown (X)")?;
        let word = self.board.read(addr as u16);
        self.sim.set_input(self.mem_in, &bits_from_u64(u64::from(word), 16));
        self.sim.settle(MAX_STEPS).map_err(stuck)?;
        let store = match self.read("write") {
            Some(0) => None,
            Some(_) => Some(self.read("mem_out").ok_or("a store's data is unknown (X)")?),
            None => return Err("`write` is unknown (X)".into()),
        };
        self.sim.clock_edge();
        self.board.tick();
        if let Some(value) = store {
            self.board.write(addr as u16, value as u16);
        }
        self.sim.settle(MAX_STEPS).map_err(stuck)?;
        Ok(())
    }

    /// Clock ticks the simulator has run, including the reset tick.
    pub fn sim_ticks(&self) -> u64 {
        self.sim.ticks()
    }
}

/// How a lockstep run went wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The chip couldn't be built or run at all.
    Setup(String),
    /// The two CPUs disagreed after running `instruction` (counting from 1)
    /// at `pc`.
    Differs { instruction: u64, pc: u16, word: u16, what: String },
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Setup(message) => write!(f, "{message}"),
            Failure::Differs { instruction: 0, what, .. } => write!(f, "before the first instruction: {what}"),
            Failure::Differs { instruction, pc, word, what } => {
                write!(f, "instruction {instruction}, `{}` at {pc:#06x} ({word:#06x}): {what}", disasm(*word))
            }
        }
    }
}

/// How far a lockstep run got.
#[derive(Clone, Debug)]
pub struct Run {
    pub instructions: u64,
    pub ticks: u64,
    pub seconds: f64,
    pub halted: bool,
    /// The board the gate-level CPU ran on: what it printed, showed and
    /// stored.
    pub board: Machine,
}

/// Runs `program` on `chip` (in gates) and on the reference Crumb side by
/// side, for up to `max` instructions or until it halts, comparing them
/// after every instruction.
pub fn lockstep(design: &Design, chip: &str, program: &[u16], max: u64, ticks_per_frame: u64) -> Result<Run, Failure> {
    let mut gates = GateCrumb::new(design, chip, program).map_err(Failure::Setup)?;
    let mut reference = Machine::new(program);
    gates.board.ticks_per_frame = ticks_per_frame;
    reference.ticks_per_frame = ticks_per_frame;
    let start = Instant::now();

    let mut instruction = 0;
    let (mut pc, mut word) = (0, 0);
    let differs = |instruction, pc, word, what: String| Failure::Differs { instruction, pc, word, what };
    let check = |name: &str, got: Option<u64>, want: u16| -> Result<(), String> {
        match got {
            Some(v) if v == u64::from(want) => Ok(()),
            Some(v) => Err(format!("{name} is {v:#06x} in gates, but {want:#06x} in the reference")),
            None => Err(format!("{name} is unknown (X) in gates, but {want:#06x} in the reference")),
        }
    };
    while instruction < max && !reference.halted {
        // Fetch. The last instruction's push lands now, so N and R are ready.
        gates.tick().map_err(|e| differs(instruction, pc, word, e))?;
        let ready = check("N", gates.read("mem_out"), reference.n())
            .and_then(|()| check("R", gates.read("r"), reference.r()))
            .and_then(|()| {
                check("execute", gates.read("execute"), 1).map_err(|_| "it isn't executing after a fetch tick".into())
            });
        ready.map_err(|e| differs(instruction, pc, word, e))?;

        // Execute.
        pc = reference.pc;
        word = reference.rom[usize::from(pc)];
        instruction += 1;
        reference.step();
        gates.tick().map_err(|e| differs(instruction, pc, word, e))?;
        let done = check("PC", gates.read("pc"), reference.pc)
            .and_then(|()| check("T", gates.read("t"), reference.t))
            .and_then(|()| check("the number display", Some(u64::from(gates.board.number)), reference.number))
            .and_then(|()| check("the LEDs", Some(u64::from(gates.board.leds)), reference.leds));
        done.map_err(|e| differs(instruction, pc, word, e))?;
        if gates.board.console != reference.console {
            let (g, r) = (gates.board.console_text(), reference.console_text());
            return Err(differs(
                instruction,
                pc,
                word,
                format!("the console says {g:?} in gates, but {r:?} in the reference"),
            ));
        }
    }
    if let Some(a) = (0..gates.board.ram.len()).find(|&a| gates.board.ram[a] != reference.ram[a]) {
        let what = format!(
            "RAM at {:#06x} is {:#06x} in gates, but {:#06x} in the reference",
            a + 0x2000,
            gates.board.ram[a],
            reference.ram[a]
        );
        return Err(differs(instruction, pc, word, what));
    }
    if gates.board.screen != reference.screen {
        return Err(differs(instruction, pc, word, "the screens differ".into()));
    }
    Ok(Run {
        board: gates.board,
        instructions: instruction,
        ticks: reference.ticks,
        seconds: start.elapsed().as_secs_f64(),
        halted: reference.halted,
    })
}
