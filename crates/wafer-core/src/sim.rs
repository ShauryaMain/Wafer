//! The event-driven simulator. Every gate takes exactly one time step: when
//! a gate's input changes, its new output appears on the next step. That's
//! what lets you watch a carry ripple through an adder, and it's why latches
//! built from plain gates work.

use crate::logic::Logic;
use crate::netlist::{NetId, Netlist};

pub struct Simulator {
    netlist: Netlist,
    values: Vec<Logic>,
    /// For each net, the gates that read it (compressed: `fanout[start[n]..start[n + 1]]`).
    fanout_start: Vec<u32>,
    fanout: Vec<u32>,
    /// Gates to re-evaluate on the next step.
    dirty: Vec<u32>,
    is_dirty: Vec<bool>,
    changes: Vec<(NetId, Logic)>,
    changed: Vec<NetId>,
    time: u64,
}

/// The circuit was still changing after the step limit, which usually means
/// it oscillates (like an odd loop of Not gates).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsettled {
    pub steps: u64,
}

impl Simulator {
    /// Starts with every input at 0, every gate output unknown (X), and
    /// undriven nets disconnected (Z).
    pub fn new(netlist: Netlist) -> Simulator {
        let mut values = vec![Logic::Z; netlist.net_count];
        for gate in &netlist.gates {
            values[gate.out as usize] = Logic::X;
        }
        for &(net, value) in &netlist.constants {
            values[net as usize] = value;
        }
        for input in &netlist.inputs {
            for &net in &input.nets {
                values[net as usize] = Logic::Zero;
            }
        }

        let mut counts = vec![0u32; netlist.net_count + 1];
        let reads = |g: &crate::netlist::Gate| {
            let single = g.kind == crate::GateKind::Not || g.a == g.b;
            if single { [Some(g.a), None] } else { [Some(g.a), Some(g.b)] }
        };
        for gate in &netlist.gates {
            for net in reads(gate).into_iter().flatten() {
                counts[net as usize + 1] += 1;
            }
        }
        for i in 1..counts.len() {
            counts[i] += counts[i - 1];
        }
        let fanout_start = counts.clone();
        let mut next = counts;
        let mut fanout = vec![0u32; netlist.gates.len() * 2];
        for (index, gate) in netlist.gates.iter().enumerate() {
            for net in reads(gate).into_iter().flatten() {
                fanout[next[net as usize] as usize] = index as u32;
                next[net as usize] += 1;
            }
        }
        fanout.truncate(*fanout_start.last().unwrap_or(&0) as usize);

        let gate_count = netlist.gates.len();
        Simulator {
            netlist,
            values,
            fanout_start,
            fanout,
            dirty: (0..gate_count as u32).collect(),
            is_dirty: vec![true; gate_count],
            changes: Vec::new(),
            changed: Vec::new(),
            time: 0,
        }
    }

    pub fn netlist(&self) -> &Netlist {
        &self.netlist
    }

    /// Time steps taken so far.
    pub fn time(&self) -> u64 {
        self.time
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
            if self.values[net as usize] != value {
                self.values[net as usize] = value;
                self.mark_readers(net);
            }
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
        self.dirty.is_empty()
    }

    /// Advances one time step and returns the nets that changed.
    pub fn step(&mut self) -> &[NetId] {
        let mut changes = std::mem::take(&mut self.changes);
        changes.clear();
        let dirty = std::mem::take(&mut self.dirty);
        for &index in &dirty {
            self.is_dirty[index as usize] = false;
            let gate = self.netlist.gates[index as usize];
            let value = gate.kind.eval(self.values[gate.a as usize], self.values[gate.b as usize]);
            if value != self.values[gate.out as usize] {
                changes.push((gate.out, value));
            }
        }
        self.dirty = dirty;
        self.dirty.clear();

        self.changed.clear();
        for &(net, value) in &changes {
            self.values[net as usize] = value;
            self.changed.push(net);
            self.mark_readers(net);
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

    fn mark_readers(&mut self, net: NetId) {
        let (start, end) = (self.fanout_start[net as usize], self.fanout_start[net as usize + 1]);
        for &gate in &self.fanout[start as usize..end as usize] {
            if !self.is_dirty[gate as usize] {
                self.is_dirty[gate as usize] = true;
                self.dirty.push(gate);
            }
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
        let mut sim = simulator(
            "chip Latch {\n in s_n, r_n\n out q, q_n\n q = Nand(s_n, q_n)\n q_n = Nand(r_n, q)\n}",
            "Latch",
        );
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
}
