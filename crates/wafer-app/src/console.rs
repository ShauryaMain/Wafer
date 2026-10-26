//! The Console panel: plays the open chip, when it's a game console, on its
//! own gates as fast as they go. It shows the screen (a green LCD), the
//! LEDs, the number display and what's been printed. The gamepad comes from
//! the keyboard while the screen has focus, and the switches from clicks.
//! The cartridge menu puts any program in the project into the console's
//! Rom, and a cartridge restarts by itself whenever its file is saved.
//!
//! Turbo mode compiles the gates and runs them on their own thread, at
//! sixty frames a second. Each frame of the app it hands over its state, so
//! the panel and the canvas show the game through the normal simulator;
//! pausing hands the final state back for good.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crumb::isa::{BUTTONS, SCREEN_HEIGHT, SCREEN_WIDTH, TICKS_PER_FRAME};
use egui::{Color32, Key, RichText, Sense, Stroke, TextureOptions};
use wafer_core::lockstep::{self, ConsoleParts};
use wafer_core::logic::{bits_from_u64, bits_to_u64};
use wafer_core::netlist::NetId;
use wafer_core::turbo::{Snapshot, Turbo};
use wafer_core::{Logic, SourceMap, program};

use crate::debugger::Source;
use crate::simrun::SimRunner;
use crate::theme;
use crate::workspace::Workspace;

/// How long each frame of the app may spend running the gates, so the rest
/// of the app stays smooth.
const BUDGET: Duration = Duration::from_millis(12);
/// How often to look at the cartridge's file for changes.
const LOOK_EVERY: Duration = Duration::from_millis(500);
/// Gate delays a tick may take before it counts as stuck.
const MAX_STEPS: u64 = 100_000;
/// Printed text kept for the panel.
const MAX_PRINTED: usize = 20_000;
/// Turbo plays in real time: no faster than this, however fast the gates go.
const TURBO_FPS: f64 = 60.0;
/// Clock ticks turbo runs between looks at what the app wants.
const BATCH: u64 = 1024;

/// The green LCD: lit pixels, dark pixels, and pixels whose bits are unknown.
pub const LCD_ON: Color32 = Color32::from_rgb(0x0f, 0x38, 0x0f);
pub const LCD_OFF: Color32 = Color32::from_rgb(0x9b, 0xbc, 0x0f);
pub const LCD_UNKNOWN: Color32 = Color32::from_rgb(0x6c, 0x8a, 0x0c);
const LED_ON: Color32 = Color32::from_rgb(0xff, 0x4d, 0x3d);

/// The keyboard keys for the gamepad buttons that aren't arrows, in the
/// order of `BUTTONS`.
const KEYS: [&str; 8] = ["", "", "", "", "Z", "X", "Enter", "Shift"];
/// Pale LCD around the pixels, so the edge rows and columns show.
const MARGIN: f32 = 6.0;

/// Where the console's ports are, found once per power-on.
struct Wiring {
    parts: ConsoleParts,
    /// The `reset`, `pad` and `switches` inputs.
    inputs: [usize; 3],
    print: NetId,
    char: Vec<NetId>,
    /// For breakpoints: Crumb's PC, and whether it's executing.
    pc: Vec<NetId>,
    execute: NetId,
    /// The memories inside the CPU holding its data and return stacks:
    /// the ones whose outputs are N and R.
    data_stack: Option<usize>,
    return_stack: Option<usize>,
}

impl Wiring {
    /// Stopped just after a fetch, with the instruction at a breakpoint
    /// about to run.
    fn at_breakpoint(&self, bit: impl Fn(NetId) -> bool, breakpoints: &[bool]) -> bool {
        bit(self.execute) && {
            let pc = self.pc.iter().enumerate().fold(0, |acc, (i, &n)| acc | usize::from(bit(n)) << i);
            breakpoints.get(pc).copied().unwrap_or(false)
        }
    }
}

/// Crumb's registers, read from the gates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registers {
    pub pc: Option<u16>,
    pub t: Option<u16>,
    pub n: Option<u16>,
    pub r: Option<u16>,
    /// The data stack under T, bottom first (as if it started empty), then T.
    pub stack: Vec<Option<u16>>,
    /// The return stack, bottom first.
    pub returns: Vec<Option<u16>>,
}

/// What the app and the turbo thread pass each other.
#[derive(Default)]
struct Exchange {
    stop: bool,
    pad: u16,
    switches: u16,
    /// Pull reset for a tick.
    reset: bool,
    /// The app wants the state, to show it.
    wanted: bool,
    snapshot: Option<Snapshot>,
    /// Characters printed since the app last looked.
    printed: String,
    /// Addresses to stop at, and whether that changed since turbo looked.
    breakpoints: Vec<u16>,
    breakpoints_changed: bool,
    /// Turbo stopped itself at a breakpoint.
    hit: bool,
}

/// Turbo running on its own thread. Dropping it stops the thread.
struct Worker {
    exchange: Arc<Mutex<Exchange>>,
    thread: Option<std::thread::JoinHandle<Turbo>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Ok(mut x) = self.exchange.lock() {
            x.stop = true;
        }
    }
}

#[derive(Default)]
pub struct Player {
    /// The chip and simulator build it last powered on. A new build (after
    /// an edit, or Power) gets the cartridge again and a reset.
    powered: Option<(String, u64)>,
    wiring: Option<Wiring>,
    /// Why the open chip can't be played, if it can't.
    pub problem: Option<String>,
    /// The program file in the project folder; `None` plays the one the
    /// chip's Rom names.
    pub cartridge: Option<String>,
    /// The cartridge's words, once it's assembled, and its source.
    program: Option<Vec<u16>>,
    cartridge_source: Option<Source>,
    /// The program being played, as source, for the debugger.
    pub source: Option<Source>,
    /// Addresses to stop at, just before the instruction there runs.
    pub breakpoints: BTreeSet<u16>,
    /// The last stop was a breakpoint.
    pub hit: bool,
    /// The simulator's tick count just after the last reset, where frames
    /// start counting.
    reset_tick: u64,
    /// The cartridge file as last read: (modified, length).
    stamp: Option<(SystemTime, u64)>,
    looked: Option<Instant>,
    /// What went wrong reading or assembling the cartridge.
    pub error: Option<String>,
    pub running: bool,
    pub printed: String,
    /// Buttons held and switches set.
    pub pad: u16,
    pub switches: u16,
    reset: bool,
    /// Frames a second it's managing, measured over the last half second.
    pub fps: f32,
    measure: Option<(Instant, u64)>,
    texture: Option<egui::TextureHandle>,
    /// Play on the normal simulator instead of turbo.
    pub turbo_off: bool,
    /// The chip compiled for turbo, once per power-on, or why it can't be.
    compiled: Option<Result<Turbo, String>>,
    worker: Option<Worker>,
    /// Which panel the app should bring to the front: this one (`true`)
    /// when a console you can play is opened, the Inspector (`false`) when
    /// you go from a console to some other chip.
    pub wants_front: Option<bool>,
}

impl Player {
    /// Per-frame work: power on when the simulator is new, notice a saved
    /// cartridge, and run the gates while playing. Returns true while it
    /// needs frames to keep going.
    pub fn update(&mut self, sim: &mut SimRunner, ws: &Workspace, chip: &str) -> bool {
        let is_console = sim.sim.as_ref().is_some_and(|s| lockstep::is_console(s.netlist()));
        if !is_console {
            if self.powered.is_some() {
                let showing = self.wiring.is_some();
                let (cartridge, switches, turbo_off) = (self.cartridge.take(), self.switches, self.turbo_off);
                *self = Player { cartridge, switches, turbo_off, ..Player::default() };
                self.wants_front = showing.then_some(false);
            }
            return false;
        }
        let newly_opened = self.powered.as_ref().is_none_or(|(c, _)| c != chip);
        if newly_opened {
            self.running = false;
        }
        if self.cartridge.is_some() && self.looked.is_none_or(|t| t.elapsed() >= LOOK_EVERY) {
            self.looked = Some(Instant::now());
            if self.read_cartridge(ws) {
                // Power on fresh with it.
                sim.reset();
                return true;
            }
        }
        if self.powered.as_ref() != Some(&(chip.to_string(), sim.builds)) {
            // A new simulator: turbo's state belongs to the old one.
            self.worker = None;
            self.power_on(sim, ws, chip);
            // Only one that's ready to play: while you're still building
            // it, the Inspector is what you need.
            if newly_opened && self.wiring.is_some() {
                self.wants_front = Some(true);
            }
        }
        if self.reset {
            self.reset = false;
            match &self.worker {
                Some(worker) => {
                    if let Ok(mut x) = worker.exchange.lock() {
                        x.reset = true;
                    }
                    self.printed.clear();
                }
                None => self.pulse_reset(sim),
            }
        }
        if self.running {
            if !self.turbo_off && self.worker.is_none() {
                self.start_turbo(sim);
            }
            if self.turbo_off && self.worker.is_some() {
                self.stop_turbo(sim);
            }
            match self.worker {
                Some(_) => self.trade(sim),
                None => self.run_for(sim, BUDGET, u64::MAX),
            }
        }
        self.running
    }

    /// Why turbo can't play this chip, once it's been tried.
    pub fn turbo_problem(&self) -> Option<&str> {
        match &self.compiled {
            Some(Err(problem)) => Some(problem),
            _ => None,
        }
    }

    /// Whether the open chip is a console that's ready to play.
    pub fn can_play(&self) -> bool {
        self.wiring.is_some()
    }

    /// Whether turbo is playing right now.
    pub fn in_turbo(&self) -> bool {
        self.worker.is_some()
    }

    /// Breakpoints as a quick lookup by address (empty when there are none).
    fn breakpoint_map(&self) -> Vec<bool> {
        let mut map = Vec::new();
        if let Some(&last) = self.breakpoints.last() {
            map = vec![false; usize::from(last) + 1];
            for &b in &self.breakpoints {
                map[usize::from(b)] = true;
            }
        }
        map
    }

    /// Runs one instruction: to just after the next fetch, so the
    /// instruction there is about to run.
    pub fn step(&mut self, sim: &mut SimRunner) {
        if self.running || self.wiring.is_none() {
            return;
        }
        self.hit = false;
        for _ in 0..4 {
            self.run_for(sim, Duration::MAX, 1);
            let executing =
                (sim.sim.as_ref()).zip(self.wiring.as_ref()).is_some_and(|(s, w)| s.value(w.execute) == Logic::One);
            if executing {
                break;
            }
        }
    }

    /// Runs to the start of the next frame (or a breakpoint), then to the
    /// next instruction.
    pub fn step_frame(&mut self, sim: &mut SimRunner) {
        if self.running || self.wiring.is_none() {
            return;
        }
        self.hit = false;
        // Frames count from reset, which was the tick before the count.
        let since = sim.ticks().saturating_sub(self.reset_tick);
        let left = TICKS_PER_FRAME - since % TICKS_PER_FRAME;
        self.run_for(sim, Duration::MAX, left);
        if !self.hit {
            self.step(sim);
        }
    }

    /// Crumb's registers and stacks, from the gates.
    pub fn registers(&self, sim: &SimRunner) -> Option<Registers> {
        let (s, w) = (sim.sim.as_ref()?, self.wiring.as_ref()?);
        let read = |path: &str| s.read(path).and_then(|bits| bits_to_u64(&bits)).map(|v| v as u16);
        let cpu = &w.parts.cpu;
        let stack = |ram: Option<usize>, from: usize| -> Vec<Option<u16>> {
            let Some(ram) = ram else { return Vec::new() };
            let block = &s.netlist().rams[ram];
            let Some(sp) = bits_to_u64(&s.values(&block.addr)) else { return Vec::new() };
            let width = block.data.len().max(1);
            let words = s.memory(ram);
            (from..=sp as usize)
                .map(|i| words.get(i * width..(i + 1) * width).and_then(bits_to_u64).map(|v| v as u16))
                .collect()
        };
        let t = read("t");
        let mut data = stack(w.data_stack, 2);
        let empty = w.data_stack.and_then(|ram| bits_to_u64(&s.values(&s.netlist().rams[ram].addr))) == Some(0);
        if !empty {
            data.push(t);
        }
        Some(Registers {
            pc: read("pc"),
            t,
            n: read(&format!("{cpu}.mem_out")),
            r: read(&format!("{cpu}.r")),
            stack: data,
            returns: stack(w.return_stack, 1),
        })
    }

    /// `count` words of RAM from `addr` (a RAM address, like a variable's).
    pub fn ram_words(&self, sim: &SimRunner, addr: u16, count: usize) -> Vec<Option<u16>> {
        let (Some(s), Some(w)) = (sim.sim.as_ref(), self.wiring.as_ref()) else { return Vec::new() };
        let words = s.memory(w.parts.ram);
        let start = usize::from(addr.wrapping_sub(crumb::isa::RAM_START));
        (start..start + count)
            .map(|i| words.get(i * 16..(i + 1) * 16).and_then(bits_to_u64).map(|v| v as u16))
            .collect()
    }

    /// Whether the console is stopped just before an instruction (after a
    /// fetch), where stepping and breakpoints leave it.
    pub fn between_instructions(&self, sim: &SimRunner) -> bool {
        (sim.sim.as_ref()).zip(self.wiring.as_ref()).is_some_and(|(s, w)| s.value(w.execute) == Logic::One)
    }

    /// Starts turbo on its own thread, from where the simulator is.
    fn start_turbo(&mut self, sim: &SimRunner) {
        let (Some(s), Some(w)) = (&sim.sim, &self.wiring) else { return };
        let compiled = self.compiled.get_or_insert_with(|| Turbo::compile(s.netlist()));
        let Ok(turbo) = compiled else { return };
        let mut turbo = turbo.clone();
        turbo.load(s);
        let exchange = Arc::new(Mutex::new(Exchange {
            pad: self.pad,
            switches: self.switches,
            breakpoints: self.breakpoints.iter().copied().collect(),
            breakpoints_changed: true,
            ..Exchange::default()
        }));
        let watch =
            Watch { inputs: w.inputs, print: w.print, char: w.char.clone(), pc: w.pc.clone(), execute: w.execute };
        let shared = exchange.clone();
        let thread = std::thread::spawn(move || play(turbo, &shared, &watch));
        self.worker = Some(Worker { exchange, thread: Some(thread) });
        self.measure = None;
    }

    /// Stops turbo and hands its state back to the simulator for good.
    fn stop_turbo(&mut self, sim: &mut SimRunner) {
        let Some(mut worker) = self.worker.take() else { return };
        if let Ok(mut x) = worker.exchange.lock() {
            x.stop = true;
        }
        let turbo = worker.thread.take().map(std::thread::JoinHandle::join);
        if let Ok(mut x) = worker.exchange.lock() {
            self.printed.push_str(&std::mem::take(&mut x.printed));
        }
        if let (Some(Ok(turbo)), Some(s)) = (turbo, &mut sim.sim) {
            let _ = turbo.snapshot().apply(s);
        }
        self.measure = None;
    }

    /// The app's frame with turbo playing: pass it the buttons, and show
    /// the state it hands over.
    fn trade(&mut self, sim: &mut SimRunner) {
        sim.trace = None;
        let Some(worker) = &self.worker else { return };
        let mut hit = false;
        let snapshot = match worker.exchange.lock() {
            Ok(mut x) => {
                x.pad = self.pad;
                x.switches = self.switches;
                x.wanted = true;
                let breakpoints: Vec<u16> = self.breakpoints.iter().copied().collect();
                if breakpoints != x.breakpoints {
                    x.breakpoints = breakpoints;
                    x.breakpoints_changed = true;
                }
                self.printed.push_str(&std::mem::take(&mut x.printed));
                hit = x.hit;
                x.snapshot.take()
            }
            Err(_) => None,
        };
        if hit {
            // Turbo stopped at a breakpoint: take its state back for good.
            self.stop_turbo(sim);
            self.running = false;
            self.hit = true;
            sim.retrace();
            return;
        }
        if let (Some(snapshot), Some(s)) = (snapshot, &mut sim.sim)
            && snapshot.apply(s).is_err()
        {
            self.problem = Some("the gates never settled".into());
        }
        self.trim_printed();
        self.measure(sim.ticks());
    }

    /// Reads and assembles the cartridge if its file changed. True when
    /// there's a new program to play.
    fn read_cartridge(&mut self, ws: &Workspace) -> bool {
        let Some(file) = self.cartridge.clone() else { return false };
        let path = ws.dir.join(&file);
        let stamp = std::fs::metadata(&path).ok().map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()));
        if stamp.is_some() && stamp == self.stamp {
            return false;
        }
        self.stamp = stamp;
        match load_program(&path) {
            Ok((words, source)) => {
                self.program = Some(words);
                self.cartridge_source = source;
                self.error = None;
                true
            }
            Err(error) => {
                // Keep playing the last version that worked.
                self.error = Some(error);
                false
            }
        }
    }

    /// Picks the cartridge to play (`None`: the chip's own program) and
    /// powers on fresh with it.
    pub fn choose(&mut self, cartridge: Option<String>, sim: &mut SimRunner, ws: &Workspace) {
        self.cartridge = cartridge;
        self.program = None;
        self.cartridge_source = None;
        self.breakpoints.clear();
        self.stamp = None;
        self.error = None;
        self.looked = Some(Instant::now());
        self.read_cartridge(ws);
        self.worker = None;
        sim.reset();
    }

    /// Everything fresh: memory all zeros and the cartridge put in again.
    pub fn power(&mut self, sim: &mut SimRunner) {
        self.worker = None;
        sim.reset();
    }

    /// Pulls `reset` for one tick on the next frame. Memory keeps its words.
    pub fn press_reset(&mut self) {
        self.reset = true;
    }

    /// Starts or stops the game. Stopping starts the timeline's history
    /// again, since it doesn't record while a game plays.
    pub fn set_running(&mut self, running: bool, sim: &mut SimRunner) {
        if self.running && !running {
            self.stop_turbo(sim);
            sim.retrace();
        }
        if running {
            self.hit = false;
        }
        self.running = running && self.wiring.is_some();
        self.measure = None;
    }

    fn power_on(&mut self, sim: &mut SimRunner, ws: &Workspace, chip: &str) {
        self.powered = Some((chip.to_string(), sim.builds));
        self.printed.clear();
        self.wiring = None;
        self.compiled = None;
        let Some(s) = &mut sim.sim else { return };
        let netlist = s.netlist();
        let parts = match ConsoleParts::find(&ws.design, chip, netlist) {
            Ok(parts) => parts,
            Err(problem) => {
                self.problem = Some(problem);
                self.running = false;
                return;
            }
        };
        let input = |name: &str| netlist.inputs.iter().position(|i| i.path == name);
        let output = |name: &str| netlist.outputs.iter().find(|o| o.path == name).map(|o| o.nets.clone());
        let (Some(reset), Some(pad), Some(switches), Some(print), Some(char), Some(pc), Some(execute)) = (
            input("reset"),
            input("pad"),
            input("switches"),
            output("print"),
            output("char"),
            output("pc"),
            output("execute"),
        ) else {
            return;
        };
        // The stacks: memories inside the CPU whose outputs are N and R.
        let feeding = |signal: &str| {
            let nets = &netlist.signal(&format!("{}.{signal}", parts.cpu))?.nets;
            let inside = format!("{}.", parts.cpu);
            netlist.rams.iter().position(|ram| ram.path.starts_with(&inside) && &ram.out == nets)
        };
        let (data_stack, return_stack) = (feeding("mem_out"), feeding("r"));
        // The program, as source, for the debugger.
        self.source = match &self.cartridge {
            Some(_) => self.cartridge_source.clone(),
            None => own_source(ws, chip),
        };
        if let Some(program) = &self.program {
            lockstep::load_program(s, parts.rom, program);
        } else if self.cartridge.is_some() {
            // The cartridge never assembled: don't play the chip's own
            // program as if it were the cartridge.
            self.problem = Some("the cartridge didn't load".into());
            self.running = false;
            return;
        }
        self.problem = None;
        self.wiring = Some(Wiring {
            parts,
            inputs: [reset, pad, switches],
            print: print[0],
            char,
            pc,
            execute: execute[0],
            data_stack,
            return_stack,
        });
        self.hit = false;
        self.pulse_reset(sim);
    }

    fn pulse_reset(&mut self, sim: &mut SimRunner) {
        let (Some(s), Some(w)) = (&mut sim.sim, &self.wiring) else { return };
        s.set_input(w.inputs[0], &bits_from_u64(1, 1));
        s.set_input(w.inputs[1], &bits_from_u64(u64::from(self.pad), 8));
        s.set_input(w.inputs[2], &bits_from_u64(u64::from(self.switches), 16));
        if s.tick(MAX_STEPS).is_err() {
            self.problem = Some("the gates never settled during reset".into());
        }
        s.set_input(w.inputs[0], &bits_from_u64(0, 1));
        let _ = s.settle(MAX_STEPS);
        self.reset_tick = s.ticks();
        self.printed.clear();
        if !self.running {
            sim.retrace();
        }
    }

    /// Runs clock ticks until `budget` has passed or `max` ticks have run,
    /// catching printed characters as they go.
    pub fn run_for(&mut self, sim: &mut SimRunner, budget: Duration, max: u64) {
        // History isn't kept while a game plays: it would be too much.
        sim.trace = None;
        let (Some(s), Some(w)) = (&mut sim.sim, &self.wiring) else { return };
        s.set_input(w.inputs[1], &bits_from_u64(u64::from(self.pad), 8));
        s.set_input(w.inputs[2], &bits_from_u64(u64::from(self.switches), 16));
        let start = Instant::now();
        let breakpoints = self.breakpoint_map();
        let mut ticks = 0;
        'run: while ticks < max {
            for _ in 0..max.saturating_sub(ticks).min(64) {
                if s.settle(MAX_STEPS).is_err() {
                    self.problem = Some("the gates never settled".into());
                    self.running = false;
                    return;
                }
                // Not where it started: that may be a breakpoint it's leaving.
                if ticks > 0 && !breakpoints.is_empty() && w.at_breakpoint(|n| s.value(n) == Logic::One, &breakpoints) {
                    self.running = false;
                    self.hit = true;
                    break 'run;
                }
                if s.value(w.print) == Logic::One {
                    let c = bits_to_u64(&s.values(&w.char)).map_or(b'?', |c| c as u8);
                    if c == b'\n' || (0x20..0x7f).contains(&c) {
                        self.printed.push(char::from(c));
                    }
                }
                s.clock_edge();
                ticks += 1;
            }
            if start.elapsed() >= budget {
                break;
            }
        }
        let _ = s.settle(MAX_STEPS);
        if !self.running {
            // Run by hand (a test fast-forwarding, say): the timeline picks
            // up from here.
            sim.retrace();
        }
        self.trim_printed();
        self.measure(sim.ticks());
    }

    fn trim_printed(&mut self) {
        if self.printed.len() > MAX_PRINTED {
            let cut = self.printed.len() - MAX_PRINTED;
            let cut = (cut..self.printed.len()).find(|&i| self.printed.is_char_boundary(i)).unwrap_or(0);
            self.printed.drain(..cut);
        }
    }

    /// Frames a second, over half a second or so.
    fn measure(&mut self, total: u64) {
        let now = Instant::now();
        match self.measure {
            Some((since, from)) if now.duration_since(since) >= Duration::from_millis(500) => {
                let seconds = now.duration_since(since).as_secs_f32();
                self.fps = (total - from) as f32 / TICKS_PER_FRAME as f32 / seconds;
                self.measure = Some((now, total));
            }
            Some(_) => {}
            None => self.measure = Some((now, total)),
        }
    }

    /// Whether the pixel at (`x`, `y`) is lit: `None` while unknown.
    pub fn pixel(&self, sim: &SimRunner, x: usize, y: usize) -> Option<bool> {
        let (s, w) = (sim.sim.as_ref()?, self.wiring.as_ref()?);
        let word = y * 8 + x / 16;
        match s.memory(w.parts.screen).get(word * 16 + 15 - x % 16)? {
            Logic::One => Some(true),
            Logic::Zero => Some(false),
            _ => None,
        }
    }

    /// The screen as an image, one texel per pixel.
    fn screen_image(&self, sim: &SimRunner) -> egui::ColorImage {
        let mut image = egui::ColorImage::filled([SCREEN_WIDTH, SCREEN_HEIGHT], LCD_OFF);
        for y in 0..SCREEN_HEIGHT {
            for x in 0..SCREEN_WIDTH {
                image.pixels[y * SCREEN_WIDTH + x] = match self.pixel(sim, x, y) {
                    Some(true) => LCD_ON,
                    Some(false) => LCD_OFF,
                    None if self.wiring.is_some() => LCD_UNKNOWN,
                    None => LCD_OFF,
                };
            }
        }
        image
    }
}

/// The nets the turbo thread drives and watches.
struct Watch {
    /// The `reset`, `pad` and `switches` inputs.
    inputs: [usize; 3],
    print: NetId,
    char: Vec<NetId>,
    pc: Vec<NetId>,
    execute: NetId,
}

/// The turbo thread: runs the gates in batches, catching what's printed,
/// at no more than sixty frames a second, until it's told to stop or
/// reaches a breakpoint.
fn play(mut turbo: Turbo, exchange: &Mutex<Exchange>, watch: &Watch) -> Turbo {
    let (start, from) = (Instant::now(), turbo.ticks());
    let mut printed = String::new();
    let mut breakpoints: Vec<bool> = Vec::new();
    let at_breakpoint = |turbo: &Turbo, breakpoints: &[bool]| {
        turbo.bit(watch.execute) && breakpoints.get(turbo.read(&watch.pc) as usize).copied().unwrap_or(false)
    };
    'play: loop {
        let (pad, switches, reset) = {
            let Ok(mut x) = exchange.lock() else { break };
            if x.stop {
                x.printed.push_str(&printed);
                break;
            }
            x.printed.push_str(&std::mem::take(&mut printed));
            if x.wanted {
                x.snapshot = Some(turbo.snapshot());
                x.wanted = false;
            }
            if std::mem::take(&mut x.breakpoints_changed) {
                breakpoints = vec![false; x.breakpoints.iter().max().map_or(0, |&b| usize::from(b) + 1)];
                for &b in &x.breakpoints {
                    breakpoints[usize::from(b)] = true;
                }
            }
            (x.pad, x.switches, std::mem::take(&mut x.reset))
        };
        turbo.set_input(watch.inputs[1], u64::from(pad));
        turbo.set_input(watch.inputs[2], u64::from(switches));
        if reset {
            turbo.set_input(watch.inputs[0], 1);
            turbo.tick();
            turbo.set_input(watch.inputs[0], 0);
        }
        for _ in 0..BATCH {
            turbo.settle();
            // Not where it started: that may be a breakpoint it's leaving.
            if turbo.ticks() > from && !breakpoints.is_empty() && at_breakpoint(&turbo, &breakpoints) {
                if let Ok(mut x) = exchange.lock() {
                    x.printed.push_str(&printed);
                    x.hit = true;
                }
                break 'play;
            }
            if turbo.bit(watch.print) {
                let c = turbo.read(&watch.char) as u8;
                if c == b'\n' || (0x20..0x7f).contains(&c) {
                    printed.push(char::from(c));
                }
            }
            turbo.clock_edge();
        }
        turbo.settle();
        let frames = (turbo.ticks() - from) as f64 / TICKS_PER_FRAME as f64;
        let due = start + Duration::from_secs_f64(frames / TURBO_FPS);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    turbo
}

/// Reads a program file: `.asm` assembled (with its source, for the
/// debugger), or `.hex` words.
fn load_program(path: &Path) -> Result<(Vec<u16>, Option<Source>), String> {
    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let text = std::fs::read_to_string(path).map_err(|e| format!("can't read {name}: {e}"))?;
    let mut sources = SourceMap::default();
    let file = sources.add(path.file_name().map(Into::into).unwrap_or_default(), text.clone());
    match program::load(&sources, file) {
        Ok(program) => {
            let words = program.words.iter().map(|&w| w as u16).collect();
            Ok((words, program.assembled.map(|p| Source::new(name, text, p))))
        }
        Err(errors) => Err(errors.iter().map(|d| d.render(&sources, false)).collect::<Vec<_>>().join("\n")),
    }
}

/// The program a console chip's own Rom holds, as source.
fn own_source(ws: &Workspace, chip: &str) -> Option<Source> {
    let held = ws.design.chips.get(chip)?.instances.iter().find_map(|i| i.program.as_ref())?;
    let rom = ws.design.programs.get(&held.path)?;
    let assembled = rom.assembled.clone()?;
    Some(Source::new(held.name.clone(), ws.project.sources.get(rom.file).text.clone(), assembled))
}

/// Every program file in the project folder.
fn program_files(dir: &Path) -> Vec<String> {
    let mut files: Vec<String> = (std::fs::read_dir(dir).into_iter().flatten().flatten())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|file| program::is_program(Path::new(file)))
        .collect();
    files.sort();
    files
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::MUTED).small().strong());
}

/// The panel.
pub fn ui(ui: &mut egui::Ui, state: &mut crate::app::AppState) {
    let crate::app::AppState { console: player, sim, ws, canvas, .. } = state;
    let chip = canvas.root.clone();
    if player.powered.is_none() {
        ui.add_space(8.0);
        ui.label(RichText::new("The open chip isn't a game console.").color(theme::TEXT));
        ui.label(
            RichText::new(
                "Open one with a console's ports (in reset, pad[8], switches[16]; out leds[16], number[16], \
                 char[8], print, pc[13], t[16], execute) to play it here, like the Console chip.",
            )
            .color(theme::MUTED),
        );
        return;
    }
    egui::ScrollArea::vertical().id_salt("console").show(ui, |ui| {
        controls(ui, player, sim, ws, &chip);
        ui.add_space(6.0);
        screen(ui, player, sim);
        ui.add_space(6.0);
        gamepad(ui, player);
        ui.add_space(8.0);
        lights(ui, sim);
        ui.add_space(8.0);
        switches(ui, player);
        ui.add_space(8.0);
        heading(ui, "PRINTED");
        egui::ScrollArea::vertical().id_salt("printed").max_height(120.0).stick_to_bottom(true).show(ui, |ui| {
            let text = if player.printed.is_empty() { " " } else { &player.printed };
            ui.label(RichText::new(text).monospace().color(theme::TEXT));
        });
    });
}

fn controls(ui: &mut egui::Ui, player: &mut Player, sim: &mut SimRunner, ws: &Workspace, chip: &str) {
    // The chip's own program, as its Rom names it.
    let own = (ws.design.chips.get(chip))
        .and_then(|c| c.instances.iter().find_map(|i| i.program.as_ref()))
        .map_or_else(|| "its own program".to_string(), |p| format!("{} (its own)", p.name));
    let mut choice: Option<Option<String>> = None;
    ui.horizontal(|ui| {
        ui.label("Cartridge");
        egui::ComboBox::from_id_salt("cartridge")
            .selected_text(RichText::new(player.cartridge.clone().unwrap_or_else(|| own.clone())).monospace())
            .show_ui(ui, |ui| {
                if ui.selectable_label(player.cartridge.is_none(), RichText::new(&own).monospace()).clicked() {
                    choice = Some(None);
                }
                for file in program_files(&ws.dir) {
                    let on = player.cartridge.as_ref() == Some(&file);
                    if ui.selectable_label(on, RichText::new(&file).monospace()).clicked() {
                        choice = Some(Some(file));
                    }
                }
            });
    });
    if let Some(cartridge) = choice {
        player.choose(cartridge, sim, ws);
    }
    ui.horizontal(|ui| {
        let label = if player.running { "Pause" } else { "Play" };
        if ui.add_enabled(player.wiring.is_some(), egui::Button::new(label)).clicked() {
            let running = !player.running;
            player.set_running(running, sim);
        }
        let mut turbo = !player.turbo_off;
        let hint = "Your gates compiled to run fast, at 60 frames a second. They don't track X and Z.";
        if ui.checkbox(&mut turbo, "Turbo").on_hover_text(hint).changed() {
            player.turbo_off = !turbo;
        }
        if ui.button("Reset").on_hover_text("Pull reset for a tick: memory keeps its words").clicked() {
            player.press_reset();
        }
        if ui.button("Power").on_hover_text("Start everything fresh, memory and all").clicked() {
            player.power(sim);
        }
    });
    let mode = if player.in_turbo() { "turbo · " } else { "" };
    let status = match (&player.problem, player.running) {
        (Some(problem), _) => RichText::new(format!("Can't play: {problem}")).color(theme::UNKNOWN),
        (None, true) => {
            RichText::new(format!("{mode}{:.1} frames/s · tick {}", player.fps, sim.ticks())).color(theme::ONE)
        }
        (None, false) => RichText::new(format!("paused · tick {}", sim.ticks())).color(theme::MUTED),
    };
    ui.label(status);
    if let Some(problem) = player.turbo_problem().filter(|_| !player.turbo_off) {
        let text = format!("Turbo can't play this chip: {problem}. Playing it on the normal simulator.");
        ui.label(RichText::new(text).small().color(theme::WARNING));
    }
    if let Some(error) = &player.error {
        ui.label(RichText::new(error).monospace().small().color(theme::UNKNOWN));
    }
}

fn screen(ui: &mut egui::Ui, player: &mut Player, sim: &SimRunner) {
    let image = player.screen_image(sim);
    let texture = match &mut player.texture {
        Some(texture) => {
            texture.set(image, TextureOptions::NEAREST);
            texture.clone()
        }
        None => player.texture.insert(ui.ctx().load_texture("console-screen", image, TextureOptions::NEAREST)).clone(),
    };
    let width = ui.available_width().min(SCREEN_WIDTH as f32 * 6.0);
    let pixels = width - 2.0 * MARGIN;
    let size = egui::vec2(width, pixels * SCREEN_HEIGHT as f32 / SCREEN_WIDTH as f32 + 2.0 * MARGIN);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Screen"));
    if response.clicked() {
        response.request_focus();
    }
    ui.painter().rect_filled(rect, 2.0, LCD_OFF);
    egui::Image::new(&texture).paint_at(ui, rect.shrink(MARGIN));
    let focused = response.has_focus();
    let stroke = if focused { Stroke::new(2.0, theme::ONE) } else { Stroke::new(1.0, theme::GATE_STROKE) };
    ui.painter().rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Outside);
    if focused {
        // Arrows and Tab stay with the game instead of moving focus; Escape lets go.
        let filter = egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false };
        ui.memory_mut(|m| m.set_focus_lock_filter(response.id, filter));
        player.pad = ui.input(|i| {
            let held = [
                i.key_down(Key::ArrowUp),
                i.key_down(Key::ArrowDown),
                i.key_down(Key::ArrowLeft),
                i.key_down(Key::ArrowRight),
                i.key_down(Key::Z),
                i.key_down(Key::X),
                i.key_down(Key::Enter),
                i.modifiers.shift,
            ];
            BUTTONS.iter().zip(held).filter(|(_, down)| *down).map(|((_, bit), _)| bit).sum()
        });
    } else {
        player.pad = 0;
    }
    let hint = if focused {
        "Playing: arrows, Z = A, X = B, Enter = Start, Shift = Select. Escape lets go."
    } else {
        "Click the screen to play with the keyboard."
    };
    ui.label(RichText::new(hint).small().color(theme::MUTED));
}

fn gamepad(ui: &mut egui::Ui, player: &Player) {
    heading(ui, "GAMEPAD");
    ui.horizontal_wrapped(|ui| {
        for ((name, bit), key) in BUTTONS.iter().zip(KEYS) {
            let held = player.pad & bit != 0;
            let label = if key.is_empty() { name.to_string() } else { format!("{name} ({key})") };
            let text = RichText::new(label).monospace().small();
            let text = if held { text.color(theme::BACKGROUND).background_color(theme::ONE) } else { text };
            ui.label(text);
        }
    });
}

fn lights(ui: &mut egui::Ui, sim: &SimRunner) {
    let read = |name: &str| sim.read(name);
    heading(ui, "LEDS");
    let leds = read("leds").unwrap_or_default();
    ui.horizontal(|ui| {
        // Bit 15 on the left, like the number reads.
        for bit in leds.iter().rev() {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
            let color = match bit {
                Logic::One => LED_ON,
                Logic::Zero => theme::GATE_FILL,
                _ => theme::UNKNOWN,
            };
            ui.painter().circle(rect.center(), 5.0, color, Stroke::new(1.0, theme::GATE_STROKE));
        }
    });
    ui.add_space(6.0);
    heading(ui, "NUMBER");
    let number = read("number").and_then(|bits| bits_to_u64(&bits));
    let text = match number {
        Some(n) => format!("{n:04X}"),
        None => "????".into(),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(text).monospace().size(28.0).color(theme::BUS));
        if let Some(n) = number {
            ui.label(RichText::new(format!("({n})")).monospace().color(theme::MUTED));
        }
    });
}

fn switches(ui: &mut egui::Ui, player: &mut Player) {
    heading(ui, "SWITCHES");
    ui.horizontal(|ui| {
        for bit in (0..16).rev() {
            let on = player.switches >> bit & 1 == 1;
            let text = RichText::new(if on { "1" } else { "0" }).monospace().small();
            if ui.selectable_label(on, text).on_hover_text(format!("switch {bit}")).clicked() {
                player.switches ^= 1 << bit;
            }
        }
    });
    ui.label(RichText::new(format!("0x{:04X}", player.switches)).monospace().small().color(theme::MUTED));
}
