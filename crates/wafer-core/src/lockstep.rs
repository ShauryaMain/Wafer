//! Runs a gate-level Crumb next to the reference Crumb, one instruction at
//! a time, and stops at the first place they differ. The gates can be a
//! bare CPU, which gets its memory and ports from a reference [`Machine`],
//! or a whole console, which has its own and runs a program loaded into
//! its Rom.

use std::fmt;
use std::time::Instant;

use crumb::Machine;
use crumb::isa::{ROM_WORDS, TICKS_PER_FRAME, disasm};

use crate::check::Design;
use crate::logic::{bits_from_u64, bits_to_u64};
use crate::netlist::{Netlist, RamBlock, Signal};
use crate::sim::Simulator;

/// Gate delays a tick may take before it counts as stuck.
const MAX_STEPS: u64 = 100_000;

/// The ports a CPU chip needs to be run this way, and their widths.
pub const INPUTS: [(&str, usize); 2] = [("reset", 1), ("mem_in", 16)];
pub const OUTPUTS: [(&str, usize); 7] =
    [("addr", 16), ("mem_out", 16), ("write", 1), ("execute", 1), ("pc", 13), ("t", 16), ("r", 16)];

/// The ports a whole console needs to be run this way, and their widths.
pub const CONSOLE_INPUTS: [(&str, usize); 3] = [("reset", 1), ("pad", 8), ("switches", 16)];
pub const CONSOLE_OUTPUTS: [(&str, usize); 7] =
    [("leds", 16), ("number", 16), ("char", 8), ("print", 1), ("pc", 13), ("t", 16), ("execute", 1)];

/// Address bits of a console's ROM and RAM (8K words each) and its screen
/// (512 words).
const RAM_BITS: usize = 13;
const SCREEN_BITS: usize = 9;

/// The ones of `inputs` and `outputs` that `netlist` doesn't have, written
/// like `in mem_in[16]`.
fn missing_ports(netlist: &Netlist, inputs: &[(&str, usize)], outputs: &[(&str, usize)]) -> Vec<String> {
    let has =
        |signals: &[Signal], name: &str, bits: usize| signals.iter().any(|s| s.path == name && s.nets.len() == bits);
    let text = |dir: &str, name: &str, bits: usize| {
        format!("{dir} {name}{}", if bits > 1 { format!("[{bits}]") } else { String::new() })
    };
    let mut missing: Vec<String> =
        inputs.iter().filter(|&&(n, b)| !has(&netlist.inputs, n, b)).map(|&(n, b)| text("in", n, b)).collect();
    missing.extend(outputs.iter().filter(|&&(n, b)| !has(&netlist.outputs, n, b)).map(|&(n, b)| text("out", n, b)));
    missing
}

fn input_index(netlist: &Netlist, name: &str) -> usize {
    netlist.inputs.iter().position(|s| s.path == name).expect("the ports were checked")
}

/// What the lockstep needs from gates running Crumb, bare or in a console.
trait Gates {
    /// One clock tick, with memory and the ports answering.
    fn tick(&mut self) -> Result<(), String>;
    /// One of Crumb's signals: `pc`, `t`, `n`, `r` or `execute`.
    fn cpu(&self, name: &str) -> Option<u64>;
    fn number(&self) -> Option<u64>;
    fn leds(&self) -> Option<u64>;
    /// Every character printed so far.
    fn console(&self) -> &[u8];
    /// RAM's and the screen's words, `None` for any with unknown bits.
    fn memory(&self) -> (Vec<Option<u16>>, Vec<Option<u16>>);
    /// What it printed, showed and stored, as a reference machine.
    fn into_board(self, program: &[u16]) -> Machine;
}

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
        GateCrumb::from_netlist(chip, netlist, program)
    }

    fn from_netlist(chip: &str, netlist: Netlist, program: &[u16]) -> Result<GateCrumb, String> {
        let missing = missing_ports(&netlist, &INPUTS, &OUTPUTS);
        if !missing.is_empty() {
            return Err(format!("`{chip}` needs these ports to run as Crumb: {}", missing.join(", ")));
        }
        let (reset, mem_in) = (input_index(&netlist, "reset"), input_index(&netlist, "mem_in"));
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

impl Gates for GateCrumb {
    fn tick(&mut self) -> Result<(), String> {
        GateCrumb::tick(self)
    }

    fn cpu(&self, name: &str) -> Option<u64> {
        // N is what a store would write.
        self.read(if name == "n" { "mem_out" } else { name })
    }

    fn number(&self) -> Option<u64> {
        Some(u64::from(self.board.number))
    }

    fn leds(&self) -> Option<u64> {
        Some(u64::from(self.board.leds))
    }

    fn console(&self) -> &[u8] {
        &self.board.console
    }

    fn memory(&self) -> (Vec<Option<u16>>, Vec<Option<u16>>) {
        (self.board.ram.iter().copied().map(Some).collect(), self.board.screen.iter().copied().map(Some).collect())
    }

    fn into_board(self, _program: &[u16]) -> Machine {
        self.board
    }
}

/// A whole gate-level console: Crumb with its own Rom, RAM, screen and
/// ports, running a program loaded into the Rom.
pub struct GateConsole {
    sim: Simulator,
    /// The `reset`, `pad` and `switches` inputs.
    inputs: [usize; 3],
    /// The CPU box inside, for N and R.
    cpu: String,
    ram: usize,
    screen: usize,
    printed: Vec<u8>,
    /// Buttons held and switches set, applied on every tick.
    pub pad: u16,
    pub switches: u16,
}

impl GateConsole {
    /// Builds `chip`, loads `program` into its Rom (in place of the one it
    /// names), and resets.
    pub fn new(design: &Design, chip: &str, program: &[u16]) -> Result<GateConsole, String> {
        let netlist = Netlist::build(design, chip).map_err(|e| e.to_string())?;
        GateConsole::from_netlist(design, chip, netlist, program)
    }

    fn from_netlist(design: &Design, chip: &str, netlist: Netlist, program: &[u16]) -> Result<GateConsole, String> {
        let missing = missing_ports(&netlist, &CONSOLE_INPUTS, &CONSOLE_OUTPUTS);
        if !missing.is_empty() {
            return Err(format!("`{chip}` needs these ports to run as a console: {}", missing.join(", ")));
        }
        // The CPU is the box whose chip has a CPU's ports.
        let fits = |ports: &[(String, usize)], wanted: &[(&str, usize)]| {
            wanted.iter().all(|&(n, b)| ports.iter().any(|(m, w)| m == n && *w == b))
        };
        let info = design.chips.get(chip).ok_or_else(|| format!("there's no chip named `{chip}`"))?;
        let cpu = (info.instances.iter())
            .find(|inst| {
                let sig = design.signature(&inst.callee);
                fits(&sig.inputs, &INPUTS) && fits(&sig.outputs, &OUTPUTS)
            })
            .map(|inst| inst.name.clone())
            .ok_or_else(|| format!("`{chip}` needs a CPU inside it, like a Crumb box"))?;
        // Its memories are found by what they are, so their names don't matter.
        let find = |what: &str, wanted: &dyn Fn(&RamBlock) -> bool| {
            let found: Vec<usize> = (netlist.rams.iter().enumerate())
                .filter(|(_, r)| !r.path.contains('.') && wanted(r))
                .map(|(i, _)| i)
                .collect();
            match found[..] {
                [i] => Ok(i),
                [] => Err(format!("`{chip}` needs {what}")),
                _ => Err(format!("`{chip}` has more than one {what}")),
            }
        };
        let rom = find("a Rom with 13 address bits for the program (`addr[12:0]`)", &|r| {
            r.rom.is_some() && r.addr.len() == RAM_BITS
        })?;
        let ram = find("a Ram with 13 address bits for RAM (`addr[12:0]`)", &|r| {
            r.rom.is_none() && r.addr.len() == RAM_BITS
        })?;
        let screen = find("a Ram with 9 address bits for the screen (`addr[8:0]`)", &|r| {
            r.rom.is_none() && r.addr.len() == SCREEN_BITS
        })?;

        let inputs = ["reset", "pad", "switches"].map(|name| input_index(&netlist, name));
        let mut sim = Simulator::new(netlist);
        let mut words = vec![0; ROM_WORDS];
        for (word, &w) in words.iter_mut().zip(program) {
            *word = u64::from(w);
        }
        sim.load_memory(rom, &words);
        // Nothing is known until the reset tick, `print` included.
        for (input, (value, bits)) in inputs.into_iter().zip([(1, 1), (0, 8), (0, 16)]) {
            sim.set_input(input, &bits_from_u64(value, bits));
        }
        sim.tick(MAX_STEPS).map_err(|_| "the gates never settled during reset".to_string())?;
        sim.set_input(inputs[0], &bits_from_u64(0, 1));
        Ok(GateConsole { sim, inputs, cpu, ram, screen, printed: Vec::new(), pad: 0, switches: 0 })
    }

    /// A signal as a number, or `None` if any bit is X or Z.
    pub fn read(&self, name: &str) -> Option<u64> {
        bits_to_u64(&self.sim.read(name)?)
    }

    /// One clock tick. A character being printed is caught before the
    /// tick ends.
    pub fn tick(&mut self) -> Result<(), String> {
        let stuck = |_| "the gates never settled".to_string();
        self.sim.set_input(self.inputs[1], &bits_from_u64(u64::from(self.pad), 8));
        self.sim.set_input(self.inputs[2], &bits_from_u64(u64::from(self.switches), 16));
        self.sim.settle(MAX_STEPS).map_err(stuck)?;
        match self.read("print") {
            Some(0) => {}
            Some(_) => {
                let c = self.read("char").ok_or("a printed character is unknown (X)")?;
                self.printed.push(c as u8);
            }
            None => return Err("`print` is unknown (X)".into()),
        }
        self.sim.clock_edge();
        self.sim.settle(MAX_STEPS).map_err(stuck)?;
        Ok(())
    }

    fn words(&self, ram: usize) -> Vec<Option<u16>> {
        self.sim.memory(ram).chunks(16).map(|w| bits_to_u64(w).map(|v| v as u16)).collect()
    }
}

impl Gates for GateConsole {
    fn tick(&mut self) -> Result<(), String> {
        GateConsole::tick(self)
    }

    fn cpu(&self, name: &str) -> Option<u64> {
        match name {
            // N is what a store would write.
            "n" => self.read(&format!("{}.mem_out", self.cpu)),
            "r" => self.read(&format!("{}.r", self.cpu)),
            name => self.read(name),
        }
    }

    fn number(&self) -> Option<u64> {
        self.read("number")
    }

    fn leds(&self) -> Option<u64> {
        self.read("leds")
    }

    fn console(&self) -> &[u8] {
        &self.printed
    }

    fn memory(&self) -> (Vec<Option<u16>>, Vec<Option<u16>>) {
        (self.words(self.ram), self.words(self.screen))
    }

    fn into_board(self, program: &[u16]) -> Machine {
        let mut board = Machine::new(program);
        let (ram, screen) = self.memory();
        board.ram = ram.into_iter().map(|w| w.unwrap_or(0)).collect();
        board.screen = screen.into_iter().map(|w| w.unwrap_or(0)).collect();
        board.leds = self.leds().unwrap_or(0) as u16;
        board.number = self.number().unwrap_or(0) as u16;
        board.console = self.printed;
        board
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
    /// What the gates printed, showed and stored, as a reference machine.
    pub board: Machine,
}

/// Runs `program` on `chip` (in gates) and on the reference Crumb side by
/// side, for up to `max` instructions or until it halts, comparing them
/// after every instruction.
///
/// `chip` is either a CPU (`in reset, mem_in[16]`), given memory and ports
/// by a reference machine whose frames are `ticks_per_frame` ticks long,
/// or a whole console (`in reset, pad[8], switches[16]`), which counts
/// frames in its own gates, so the reference uses `TICKS_PER_FRAME`.
pub fn lockstep(design: &Design, chip: &str, program: &[u16], max: u64, ticks_per_frame: u64) -> Result<Run, Failure> {
    let netlist = Netlist::build(design, chip).map_err(|e| Failure::Setup(e.to_string()))?;
    let has = |name: &str| netlist.inputs.iter().any(|s| s.path == name);
    if has("mem_in") {
        let mut gates = GateCrumb::from_netlist(chip, netlist, program).map_err(Failure::Setup)?;
        gates.board.ticks_per_frame = ticks_per_frame;
        compare(gates, program, max, ticks_per_frame)
    } else if has("pad") {
        let gates = GateConsole::from_netlist(design, chip, netlist, program).map_err(Failure::Setup)?;
        compare(gates, program, max, TICKS_PER_FRAME)
    } else {
        Err(Failure::Setup(format!(
            "`{chip}` isn't a CPU (`in reset, mem_in[16]`) or a console (`in reset, pad[8], switches[16]`)"
        )))
    }
}

fn compare(mut gates: impl Gates, program: &[u16], max: u64, ticks_per_frame: u64) -> Result<Run, Failure> {
    let mut reference = Machine::new(program);
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
        let ready = check("N", gates.cpu("n"), reference.n())
            .and_then(|()| check("R", gates.cpu("r"), reference.r()))
            .and_then(|()| {
                check("execute", gates.cpu("execute"), 1).map_err(|_| "it isn't executing after a fetch tick".into())
            });
        ready.map_err(|e| differs(instruction, pc, word, e))?;

        // Execute.
        pc = reference.pc;
        word = reference.rom[usize::from(pc)];
        instruction += 1;
        reference.step();
        gates.tick().map_err(|e| differs(instruction, pc, word, e))?;
        let done = check("PC", gates.cpu("pc"), reference.pc)
            .and_then(|()| check("T", gates.cpu("t"), reference.t))
            .and_then(|()| check("the number display", gates.number(), reference.number))
            .and_then(|()| check("the LEDs", gates.leds(), reference.leds));
        done.map_err(|e| differs(instruction, pc, word, e))?;
        if gates.console() != reference.console.as_slice() {
            let (g, r) = (String::from_utf8_lossy(gates.console()), reference.console_text());
            return Err(differs(
                instruction,
                pc,
                word,
                format!("the console says {g:?} in gates, but {r:?} in the reference"),
            ));
        }
    }
    let shown = |w: Option<u16>| w.map_or_else(|| "unknown (X)".to_string(), |w| format!("{w:#06x}"));
    let (ram, screen) = gates.memory();
    if let Some(a) = (0..reference.ram.len()).find(|&a| ram.get(a).copied().flatten() != Some(reference.ram[a])) {
        let what = format!(
            "RAM at {:#06x} is {} in gates, but {:#06x} in the reference",
            a + usize::from(crumb::isa::RAM_START),
            shown(ram.get(a).copied().flatten()),
            reference.ram[a]
        );
        return Err(differs(instruction, pc, word, what));
    }
    if let Some(a) =
        (0..reference.screen.len()).find(|&a| screen.get(a).copied().flatten() != Some(reference.screen[a]))
    {
        let what = format!(
            "the screen at {:#06x} is {} in gates, but {:#06x} in the reference",
            a + usize::from(crumb::isa::SCREEN),
            shown(screen.get(a).copied().flatten()),
            reference.screen[a]
        );
        return Err(differs(instruction, pc, word, what));
    }
    Ok(Run {
        instructions: instruction,
        ticks: reference.ticks,
        seconds: start.elapsed().as_secs_f64(),
        halted: reference.halted,
        board: gates.into_board(program),
    })
}
