//! The event-driven simulator. Every gate takes exactly one time step: when
//! a gate's input changes, its new output appears on the next step. That's
//! what lets you watch a carry ripple through an adder, and it's why latches
//! built from plain gates work.
//!
//! On top of that there's one global clock. A tick samples every flip-flop's
//! input at the same instant, stores RAM writes, and then the logic settles
//! again, one gate delay at a time.

use crate::logic::{Logic, bits_to_u64};
use crate::netlist::{Gate, NetId, Netlist};

/// Address widths beyond this are refused, to keep memory sane (1M words).
pub const MAX_ADDRESS_BITS: usize = 20;

pub struct Simulator {
    netlist: Netlist,
    values: Vec<Logic>,
    /// For each net, the gates that read it (compressed: `fanout[start[n]..start[n + 1]]`).
    fanout_start: Vec<u32>,
    fanout: Vec<u32>,
    /// For each net, the RAM blocks whose address reads it.
    ram_readers: Vec<Vec<u32>>,
    /// For each net that drives a shared wire, which one (`u32::MAX` if none).
    wired_of: Vec<u32>,
    /// Gates and RAMs to re-evaluate on the next step.
    dirty: Vec<u32>,
    is_dirty: Vec<bool>,
    dirty_rams: Vec<u32>,
    is_dirty_ram: Vec<bool>,
    /// Each RAM's words, `data width` values per word, lowest address first.
    memories: Vec<Vec<Logic>>,
    changes: Vec<(NetId, Logic)>,
    changed: Vec<NetId>,
    time: u64,
    ticks: u64,
}

/// The circuit was still changing after the step limit, which usually means
/// it oscillates (like an odd loop of Not gates).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsettled {
    pub steps: u64,
}

/// The nets a gate reads, each once.
fn reads(g: &Gate) -> Vec<NetId> {
    let mut nets = match g.kind {
        crate::GateKind::Not => vec![g.a],
        crate::GateKind::Mux => vec![g.a, g.b, g.c],
        _ => vec![g.a, g.b],
    };
    nets.sort_unstable();
    nets.dedup();
    nets
}

/// What a shared wire carries: the one driver that isn't letting go, Z if
/// none is driving, X if they disagree.
fn resolve(values: impl Iterator<Item = Logic>) -> Logic {
    values.fold(Logic::Z, |acc, v| match (acc, v) {
        (Logic::Z, v) => v,
        (acc, Logic::Z) => acc,
        (a, b) if a == b => a,
        _ => Logic::X,
    })
}

fn solid(v: Logic) -> Logic {
    if v == Logic::Z { Logic::X } else { v }
}

impl Simulator {
    /// Starts with every input at 0, gate and flip-flop outputs unknown (X),
    /// RAM cleared to 0, and undriven nets disconnected (Z).
    pub fn new(netlist: Netlist) -> Simulator {
        let n = netlist.net_count;
        let mut values = vec![Logic::Z; n];
        for gate in &netlist.gates {
            values[gate.out as usize] = Logic::X;
        }
        for flop in &netlist.flops {
            values[flop.q as usize] = Logic::X;
        }
        for ram in &netlist.rams {
            for &net in &ram.out {
                values[net as usize] = Logic::X;
            }
        }
        for &(net, value) in &netlist.constants {
            values[net as usize] = value;
        }
        for input in &netlist.inputs {
            for &net in &input.nets {
                values[net as usize] = Logic::Zero;
            }
        }

        let mut counts = vec![0u32; n + 1];
        for gate in &netlist.gates {
            for net in reads(gate) {
                counts[net as usize + 1] += 1;
            }
        }
        for i in 1..counts.len() {
            counts[i] += counts[i - 1];
        }
        let fanout_start = counts.clone();
        let mut next = counts;
        let mut fanout = vec![0u32; *fanout_start.last().unwrap_or(&0) as usize];
        for (index, gate) in netlist.gates.iter().enumerate() {
            for net in reads(gate) {
                fanout[next[net as usize] as usize] = index as u32;
                next[net as usize] += 1;
            }
        }

        let mut ram_readers = vec![Vec::new(); n];
        for (i, ram) in netlist.rams.iter().enumerate() {
            for &net in &ram.addr {
                if !ram_readers[net as usize].contains(&(i as u32)) {
                    ram_readers[net as usize].push(i as u32);
                }
            }
        }
        let mut wired_of = vec![u32::MAX; n];
        for (i, wire) in netlist.wired.iter().enumerate() {
            for &net in &wire.drivers {
                wired_of[net as usize] = i as u32;
            }
        }
        for wire in &netlist.wired {
            values[wire.out as usize] = resolve(wire.drivers.iter().map(|&d| values[d as usize]));
        }
        let memories = netlist
            .rams
            .iter()
            .map(|r| vec![Logic::Zero; (1usize << r.addr.len().min(MAX_ADDRESS_BITS)) * r.data.len()])
            .collect();

        let (gates, rams) = (netlist.gates.len(), netlist.rams.len());
        Simulator {
            netlist,
            values,
            fanout_start,
            fanout,
            ram_readers,
            wired_of,
            dirty: (0..gates as u32).collect(),
            is_dirty: vec![true; gates],
            dirty_rams: (0..rams as u32).collect(),
            is_dirty_ram: vec![true; rams],
            memories,
            changes: Vec::new(),
            changed: Vec::new(),
            time: 0,
            ticks: 0,
        }
    }

    pub fn netlist(&self) -> &Netlist {
        &self.netlist
    }

    /// Time steps (gate delays) taken so far.
    pub fn time(&self) -> u64 {
        self.time
    }

    /// Clock ticks so far.
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    pub fn value(&self, net: NetId) -> Logic {
        self.values[net as usize]
    }

    pub fn values(&self, nets: &[NetId]) -> Vec<Logic> {
        nets.iter().map(|&n| self.value(n)).collect()
    }

    /// The current value of a named signal, like `sum` or `fa0.ab`.
    pub fn read(&self, path: &str) -> Option<Vec<Logic>> {
        self.netlist.signal(path).map(|s| self.values(&s.nets))
    }

    pub fn set_input(&mut self, index: usize, bits: &[Logic]) {
        let nets = self.netlist.inputs[index].nets.clone();
        for (&net, &value) in nets.iter().zip(bits) {
            self.apply(net, value);
        }
    }

    /// Returns false if the chip has no input with that name.
    pub fn set_input_by_name(&mut self, name: &str, bits: &[Logic]) -> bool {
        match self.netlist.inputs.iter().position(|s| s.path == name) {
            Some(index) => {
                self.set_input(index, bits);
                true
            }
            None => false,
        }
    }

    pub fn is_settled(&self) -> bool {
        self.dirty.is_empty() && self.dirty_rams.is_empty()
    }

    /// Advances one time step and returns the nets that changed.
    pub fn step(&mut self) -> &[NetId] {
        let mut changes = std::mem::take(&mut self.changes);
        changes.clear();
        let dirty = std::mem::take(&mut self.dirty);
        for &index in &dirty {
            self.is_dirty[index as usize] = false;
            let gate = self.netlist.gates[index as usize];
            let v = |net: NetId| self.values[net as usize];
            let value = gate.kind.eval(v(gate.a), v(gate.b), v(gate.c));
            if value != v(gate.out) {
                changes.push((gate.out, value));
            }
        }
        self.dirty = dirty;
        self.dirty.clear();

        let rams = std::mem::take(&mut self.dirty_rams);
        for &r in &rams {
            self.is_dirty_ram[r as usize] = false;
            let word = self.read_word(r as usize);
            for (&net, value) in self.netlist.rams[r as usize].out.iter().zip(word) {
                if value != self.values[net as usize] {
                    changes.push((net, value));
                }
            }
        }
        self.dirty_rams = rams;
        self.dirty_rams.clear();

        self.changed.clear();
        for &(net, value) in &changes {
            self.apply(net, value);
        }
        self.changes = changes;
        self.time += 1;
        &self.changed
    }

    /// Steps until nothing changes. Returns how many steps it took for the
    /// last signal to change, or an error if it's still changing after
    /// `max_steps`.
    pub fn settle(&mut self, max_steps: u64) -> Result<u64, Unsettled> {
        let start = self.time;
        let mut last_change = 0;
        while !self.is_settled() {
            if self.time - start >= max_steps {
                return Err(Unsettled { steps: max_steps });
            }
            if !self.step().is_empty() {
                last_change = self.time - start;
            }
        }
        Ok(last_change)
    }

    /// The clock edge: every flip-flop takes its input's value at once, and
    /// RAMs store writes. Returns the nets that changed. Call [`Self::settle`]
    /// (or step) afterwards to let the logic catch up.
    pub fn clock_edge(&mut self) -> &[NetId] {
        self.changed.clear();
        let sampled: Vec<(NetId, Logic)> =
            self.netlist.flops.iter().map(|f| (f.q, solid(self.values[f.d as usize]))).collect();
        for r in 0..self.netlist.rams.len() {
            let ram = &self.netlist.rams[r];
            let write = self.values[ram.write as usize];
            if write == Logic::Zero {
                continue;
            }
            let addr = bits_to_u64(&self.values(&ram.addr));
            let width = ram.data.len();
            let data: Vec<Logic> = match write {
                Logic::One => ram.data.iter().map(|&n| solid(self.values[n as usize])).collect(),
                // Not sure whether it's writing: whatever is there becomes unknown.
                _ => vec![Logic::X; width],
            };
            if let Some(a) = addr {
                let start = a as usize * width;
                if let Some(slot) = self.memories[r].get_mut(start..start + width) {
                    slot.copy_from_slice(&data);
                }
            }
            self.mark_ram(r as u32);
        }
        for (q, v) in sampled {
            self.apply(q, v);
        }
        self.ticks += 1;
        &self.changed
    }

    /// Settles, ticks the clock, and settles again. Returns the gate delays
    /// it took to settle after the tick.
    pub fn tick(&mut self, max_steps: u64) -> Result<u64, Unsettled> {
        self.settle(max_steps)?;
        self.clock_edge();
        self.settle(max_steps)
    }

    /// A RAM's contents: `data width` values per word, lowest address first.
    pub fn memory(&self, ram: usize) -> &[Logic] {
        &self.memories[ram]
    }

    /// Fills a RAM with numbers, starting at address 0.
    pub fn load_memory(&mut self, ram: usize, words: &[u64]) {
        let width = self.netlist.rams[ram].data.len();
        for (a, &word) in words.iter().enumerate() {
            let bits = crate::logic::bits_from_u64(word, width);
            if let Some(slot) = self.memories[ram].get_mut(a * width..(a + 1) * width) {
                slot.copy_from_slice(&bits);
            }
        }
        self.mark_ram(ram as u32);
    }

    fn read_word(&self, r: usize) -> Vec<Logic> {
        let ram = &self.netlist.rams[r];
        let width = ram.data.len();
        match bits_to_u64(&self.values(&ram.addr)) {
            Some(a) => self.memories[r]
                .get(a as usize * width..(a as usize + 1) * width)
                .map_or_else(|| vec![Logic::X; width], <[Logic]>::to_vec),
            None => vec![Logic::X; width],
        }
    }

    /// Sets a net, wakes whatever reads it, and updates the shared wire it
    /// drives, if any.
    fn apply(&mut self, net: NetId, value: Logic) {
        if self.values[net as usize] == value {
            return;
        }
        self.values[net as usize] = value;
        self.changed.push(net);
        self.mark_readers(net);
        let wire = self.wired_of[net as usize];
        if wire != u32::MAX {
            let w = &self.netlist.wired[wire as usize];
            let resolved = resolve(w.drivers.iter().map(|&d| self.values[d as usize]));
            let out = w.out;
            self.apply(out, resolved);
        }
    }

    fn mark_readers(&mut self, net: NetId) {
        let (start, end) = (self.fanout_start[net as usize], self.fanout_start[net as usize + 1]);
        for &gate in &self.fanout[start as usize..end as usize] {
            if !self.is_dirty[gate as usize] {
                self.is_dirty[gate as usize] = true;
                self.dirty.push(gate);
            }
        }
        for i in 0..self.ram_readers[net as usize].len() {
            let r = self.ram_readers[net as usize][i];
            self.mark_ram(r);
        }
    }

    fn mark_ram(&mut self, r: u32) {
        if !self.is_dirty_ram[r as usize] {
            self.is_dirty_ram[r as usize] = true;
            self.dirty_rams.push(r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::Logic::*;
    use crate::{Design, Project};

    fn simulator(text: &str, chip: &str) -> Simulator {
        let project = Project::from_sources(vec![("test.chip".into(), text.into())]);
        assert!(project.diagnostics.is_empty(), "{:#?}", project.diagnostics);
        let design = Design::check(&project);
        assert!(!design.has_errors(), "{:#?}", design.diagnostics);
        Simulator::new(Netlist::build(&design, chip).unwrap())
    }

    #[test]
    fn signals_take_one_step_per_gate() {
        let mut sim = simulator("chip Chain {\n in a\n out y\n n1 = Not(a)\n n2 = Not(n1)\n y = Not(n2)\n}", "Chain");
        assert_eq!(sim.settle(100), Ok(3));
        assert_eq!(sim.read("y"), Some(vec![One]));
        sim.set_input(0, &[One]);
        assert_eq!(sim.settle(100), Ok(3));
        assert_eq!(sim.read("y"), Some(vec![Zero]));
    }

    #[test]
    fn nand_latch_holds_its_value() {
        let mut sim =
            simulator("chip Latch {\n in s_n, r_n\n out q, q_n\n q = Nand(s_n, q_n)\n q_n = Nand(r_n, q)\n}", "Latch");
        sim.set_input(0, &[Zero]);
        sim.set_input(1, &[One]);
        sim.settle(100).unwrap();
        assert_eq!(sim.read("q"), Some(vec![One]));
        sim.set_input(0, &[One]);
        sim.settle(100).unwrap();
        assert_eq!(sim.read("q"), Some(vec![One]), "latch should remember");
        assert_eq!(sim.read("q_n"), Some(vec![Zero]));
    }

    #[test]
    fn unknowns_spread_and_oscillators_are_caught() {
        // With nothing setting it, a latch starts out unknown.
        let mut latch = simulator("chip L {\n in en\n out q\n q = Nand(en, q)\n}", "L");
        latch.set_input(0, &[One]);
        latch.settle(100).unwrap();
        assert_eq!(latch.read("q"), Some(vec![X]));

        // Once it's known, Nand(1, q) feeding itself flips every step.
        let mut ring = simulator("chip R {\n in en\n out q\n q = Nand(en, q)\n}", "R");
        ring.settle(100).unwrap();
        assert_eq!(ring.read("q"), Some(vec![One]));
        ring.set_input(0, &[One]);
        assert_eq!(ring.settle(50), Err(Unsettled { steps: 50 }));
    }

    #[test]
    fn buses_and_hierarchy() {
        let mut sim = simulator(
            "chip Inv4 {\n in a[4]\n out y[4]\n y = Not(a)\n}\n\
             chip Top {\n in a[4]\n out y[4], top\n inv = Inv4(a)\n y = {inv[1:0], inv[3:2]}\n top = inv[3]\n}",
            "Top",
        );
        sim.set_input(0, &crate::logic::bits_from_u64(0b0011, 4));
        sim.settle(100).unwrap();
        assert_eq!(sim.read("inv"), Some(crate::logic::bits_from_u64(0b1100, 4)));
        assert_eq!(sim.read("y"), Some(crate::logic::bits_from_u64(0b0011, 4)));
        assert_eq!(sim.read("top"), Some(vec![One]));
        assert!(sim.read("inv.a").is_some(), "inner signals are named");
    }

    #[test]
    fn flip_flops_change_only_on_a_tick() {
        let mut sim = simulator(
            "chip Toggle {\n in reset\n out q\n nq = Not(q)\n d = Mux(nq, 0, reset)\n q = Dff(d)\n}",
            "Toggle",
        );
        sim.settle(100).unwrap();
        assert_eq!(sim.read("q"), Some(vec![X]), "unknown until reset");
        sim.set_input(0, &[One]);
        sim.tick(100).unwrap();
        assert_eq!(sim.read("q"), Some(vec![Zero]));
        sim.set_input(0, &[Zero]);
        sim.settle(100).unwrap();
        assert_eq!(sim.read("q"), Some(vec![Zero]), "no tick, no change");
        sim.tick(100).unwrap();
        assert_eq!(sim.read("q"), Some(vec![One]));
        sim.tick(100).unwrap();
        assert_eq!(sim.read("q"), Some(vec![Zero]));
        assert_eq!(sim.ticks(), 3);
    }

    #[test]
    fn ram_writes_on_a_tick_and_reads_any_time() {
        use crate::logic::bits_from_u64 as bits;
        let mut sim =
            simulator("chip Mem {\n in addr[2], data[4], write\n out out[4]\n out = Ram(addr, data, write)\n}", "Mem");
        sim.set_input(0, &bits(1, 2));
        sim.set_input(1, &bits(5, 4));
        sim.set_input(2, &[One]);
        sim.settle(100).unwrap();
        assert_eq!(sim.read("out"), Some(bits(0, 4)), "memory starts cleared");
        sim.tick(100).unwrap();
        assert_eq!(sim.read("out"), Some(bits(5, 4)));
        sim.set_input(2, &[Zero]);
        sim.set_input(0, &bits(0, 2));
        sim.settle(100).unwrap();
        assert_eq!(sim.read("out"), Some(bits(0, 4)));
        sim.set_input(0, &bits(1, 2));
        sim.settle(100).unwrap();
        assert_eq!(sim.read("out"), Some(bits(5, 4)));
        assert_eq!(&sim.memory(0)[4..8], bits(5, 4).as_slice());
    }

    #[test]
    fn shared_buses_take_one_driver_at_a_time() {
        use crate::logic::bits_from_u64 as bits;
        let mut sim = simulator(
            "chip Bus {\n in a[4], b[4], ea, eb\n out y[4]\n wire bus[4]\n bus <- TriBuf(a, ea)\n bus <- TriBuf(b, eb)\n y = bus\n}",
            "Bus",
        );
        sim.set_input(0, &bits(3, 4));
        sim.set_input(1, &bits(12, 4));
        for (ea, eb, expected) in
            [(One, Zero, bits(3, 4)), (Zero, One, bits(12, 4)), (Zero, Zero, vec![Z; 4]), (One, One, vec![X; 4])]
        {
            sim.set_input(2, &[ea]);
            sim.set_input(3, &[eb]);
            sim.settle(100).unwrap();
            assert_eq!(sim.read("y"), Some(expected), "ea={ea} eb={eb}");
        }
        assert_eq!(sim.read("bus_d2"), Some(bits(12, 4)), "each driver keeps its own name");
    }

    #[test]
    fn shared_wires_need_every_driver_to_use_arrows() {
        let project = Project::from_sources(vec![(
            "t.chip".into(),
            "chip M {\n in a, e\n out y\n wire w\n w <- TriBuf(a, e)\n w = a\n y = w\n}".into(),
        )]);
        let design = Design::check(&project);
        assert!(design.diagnostics.iter().any(|d| d.message.contains("is a shared wire")), "{:#?}", design.diagnostics);
    }
}
