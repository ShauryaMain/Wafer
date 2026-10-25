//! The reference Crumb: the whole console (CPU, memory, screen and ports)
//! modeled directly in Rust, following `docs/crumb.md` tick for tick. It
//! runs games at full speed, and it's what the gate-level Crumb is checked
//! against.

use crate::isa::{
    ADDRESS_MASK, CONSOLE, FRAME, Insn, LEDS, NUMBER, Op, PAD, RAM_START, RAM_WORDS, RANDOM, ROM_WORDS, SCREEN,
    SCREEN_ROW_WORDS, SCREEN_WIDTH, SCREEN_WORDS, STACK_DEPTH, SWITCHES, TICKS_PER_FRAME,
};

/// Where the random-number LFSR starts after reset.
pub const RANDOM_SEED: u16 = 0xACE1;

#[derive(Clone, Debug)]
pub struct Machine {
    pub pc: u16,
    pub t: u16,
    pub dsp: usize,
    pub rsp: usize,
    pub dstack: [u16; STACK_DEPTH],
    pub rstack: [u16; STACK_DEPTH],
    pub rom: Vec<u16>,
    pub ram: Vec<u16>,
    pub screen: Vec<u16>,
    /// Buttons held right now (set by whoever is playing).
    pub pad: u16,
    pub switches: u16,
    pub leds: u16,
    pub number: u16,
    /// Every character written to the console so far.
    pub console: Vec<u8>,
    /// Clock ticks since reset.
    pub ticks: u64,
    pub instructions: u64,
    pub ticks_per_frame: u64,
    lfsr: u16,
    /// True once the program has run into a `halt` (a jump to itself).
    pub halted: bool,
}

impl Machine {
    /// A console with `program` in its ROM, just after reset.
    pub fn new(program: &[u16]) -> Machine {
        let mut rom = vec![0; ROM_WORDS];
        let n = program.len().min(ROM_WORDS);
        rom[..n].copy_from_slice(&program[..n]);
        let mut machine = Machine {
            pc: 0,
            t: 0,
            dsp: 0,
            rsp: 0,
            dstack: [0; STACK_DEPTH],
            rstack: [0; STACK_DEPTH],
            rom,
            ram: vec![0; RAM_WORDS],
            screen: vec![0; SCREEN_WORDS],
            pad: 0,
            switches: 0,
            leds: 0,
            number: 0,
            console: Vec::new(),
            ticks: 0,
            instructions: 0,
            ticks_per_frame: TICKS_PER_FRAME,
            lfsr: RANDOM_SEED,
            halted: false,
        };
        machine.reset();
        machine
    }

    /// Back to the reset state, keeping the ROM, the buttons and switches.
    /// Memory keeps its words too, like real RAM: the stacks, RAM and the
    /// screen are only all zeros in a new machine (at power-on).
    pub fn reset(&mut self) {
        self.pc = 0;
        self.t = 0;
        self.dsp = 0;
        self.rsp = 0;
        self.leds = 0;
        self.number = 0;
        self.console.clear();
        self.ticks = 0;
        self.instructions = 0;
        self.lfsr = RANDOM_SEED;
        self.halted = false;
    }

    /// Second on the data stack.
    pub fn n(&self) -> u16 {
        self.dstack[self.dsp]
    }

    /// Top of the return stack.
    pub fn r(&self) -> u16 {
        self.rstack[self.rsp]
    }

    /// The data stack, bottom first, assuming it started empty at reset:
    /// `dsp` values, the last one being `T`.
    pub fn stack(&self) -> Vec<u16> {
        let mut out: Vec<u16> = (2..=self.dsp).map(|i| self.dstack[i]).collect();
        if self.dsp > 0 {
            out.push(self.t);
        }
        out
    }

    /// The return stack, bottom first, assuming it started empty.
    pub fn return_stack(&self) -> Vec<u16> {
        (1..=self.rsp).map(|i| self.rstack[i]).collect()
    }

    pub fn frame(&self) -> u16 {
        (self.ticks / self.ticks_per_frame.max(1)) as u16
    }

    pub fn random(&self) -> u16 {
        self.lfsr
    }

    /// Whether the pixel at (`x`, `y`) is lit.
    pub fn pixel(&self, x: usize, y: usize) -> bool {
        let word = self.screen[y * SCREEN_ROW_WORDS + x / 16];
        word & (0x8000 >> (x % 16)) != 0
    }

    /// What reading `addr` gives right now. Reading never changes anything.
    pub fn read(&self, addr: u16) -> u16 {
        let a = usize::from(addr);
        match addr {
            0x0000..0x2000 => self.rom[a],
            0x2000..0x4000 => self.ram[a - usize::from(RAM_START)],
            _ if (SCREEN..SCREEN + SCREEN_WORDS as u16).contains(&addr) => self.screen[a - usize::from(SCREEN)],
            PAD => self.pad & 0xFF,
            FRAME => self.frame(),
            RANDOM => self.lfsr,
            LEDS => self.leds,
            SWITCHES => self.switches,
            NUMBER => self.number,
            _ => 0,
        }
    }

    /// What a store to `addr` does: RAM and the screen keep it, the LEDs and
    /// number display show it, and the console prints it.
    pub fn write(&mut self, addr: u16, value: u16) {
        let a = usize::from(addr);
        match addr {
            0x2000..0x4000 => self.ram[a - usize::from(RAM_START)] = value,
            _ if (SCREEN..SCREEN + SCREEN_WORDS as u16).contains(&addr) => self.screen[a - usize::from(SCREEN)] = value,
            LEDS => self.leds = value,
            NUMBER => self.number = value,
            CONSOLE => self.console.push(value as u8),
            _ => {}
        }
    }

    /// One clock tick passes for the console: the frame counter and the
    /// random-number LFSR move on. [`Self::step`] does this twice; a
    /// gate-level CPU using this machine as its memory calls it itself.
    pub fn tick(&mut self) {
        self.ticks += 1;
        // Galois LFSR, taps 0xB400.
        let bit = self.lfsr & 1;
        self.lfsr >>= 1;
        if bit != 0 {
            self.lfsr ^= 0xB400;
        }
    }

    /// Runs one instruction: a fetch tick, then an execute tick.
    pub fn step(&mut self) {
        let word = self.rom[usize::from(self.pc & ADDRESS_MASK)];
        self.tick();
        let next = (self.pc + 1) & ADDRESS_MASK;
        let wrap = |sp: usize, delta: i8| (sp as isize + delta as isize).rem_euclid(STACK_DEPTH as isize) as usize;
        match Insn::decode(word) {
            Insn::Lit(v) => {
                self.dsp = wrap(self.dsp, 1);
                self.dstack[self.dsp] = self.t;
                self.t = v;
                self.pc = next;
            }
            Insn::Jump(a) => {
                self.halted |= a == self.pc;
                self.pc = a;
            }
            Insn::Jz(a) => {
                let flag = self.t;
                self.t = self.n();
                self.dsp = wrap(self.dsp, -1);
                self.pc = if flag == 0 { a } else { next };
            }
            Insn::Call(a) => {
                self.rsp = wrap(self.rsp, 1);
                self.rstack[self.rsp] = next;
                self.pc = a;
            }
            Insn::Alu(alu) => {
                let (t, n, r) = (self.t, self.n(), self.r());
                let mem = if alu.op == Op::Load { self.read(t) } else { 0 };
                let result = alu.op.eval(t, n, r, mem);
                self.dsp = wrap(self.dsp, alu.dd);
                self.rsp = wrap(self.rsp, alu.rd);
                if alu.t_to_n {
                    self.dstack[self.dsp] = t;
                }
                if alu.t_to_r {
                    self.rstack[self.rsp] = t;
                }
                if alu.store {
                    self.write(t, n);
                }
                self.pc = if alu.ret { r & ADDRESS_MASK } else { next };
                self.t = result;
            }
        }
        self.tick();
        self.instructions += 1;
    }

    /// Runs until the program halts or `max` instructions have run. Returns
    /// how many ran.
    pub fn run(&mut self, max: u64) -> u64 {
        let start = self.instructions;
        while !self.halted && self.instructions - start < max {
            self.step();
        }
        self.instructions - start
    }

    /// Runs until `frames` more frames have started. A halted program
    /// keeps running its jump-to-itself, like the real chip would.
    pub fn run_frames(&mut self, frames: u64) {
        let target = (self.ticks / self.ticks_per_frame.max(1) + frames) * self.ticks_per_frame.max(1);
        while self.ticks < target {
            self.step();
        }
    }

    /// The console's text so far.
    pub fn console_text(&self) -> String {
        String::from_utf8_lossy(&self.console).into_owned()
    }

    /// The screen as rows of `#` (lit) and `.` (dark), for tests.
    pub fn screen_text(&self) -> String {
        let mut out = String::new();
        for y in 0..SCREEN_WORDS / SCREEN_ROW_WORDS {
            for x in 0..SCREEN_WIDTH {
                out.push(if self.pixel(x, y) { '#' } else { '.' });
            }
            out.push('\n');
        }
        out
    }
}
