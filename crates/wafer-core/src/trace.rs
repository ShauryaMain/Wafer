//! Signal history for the waveform timeline: every change to a set of
//! signals, stamped with the simulator's time step.

use crate::logic::Logic;
use crate::netlist::{NetId, Signal};
use crate::sim::Simulator;

pub struct Trace {
    pub signals: Vec<Signal>,
    /// Per signal, each value it took and the step it started at, oldest first.
    pub changes: Vec<Vec<(u64, Vec<Logic>)>>,
    /// The earliest step still in the history.
    pub start: u64,
    /// The latest step recorded.
    pub end: u64,
    /// Steps where an input was changed by hand.
    pub marks: Vec<u64>,
    /// For each net, the traced signals that include it.
    readers: Vec<Vec<u32>>,
    recorded: usize,
}

/// History beyond this many recorded changes gets trimmed, oldest first.
const MAX_CHANGES: usize = 400_000;

impl Trace {
    /// Starts tracing `signals`, recording their current values.
    pub fn new(sim: &Simulator, signals: Vec<Signal>) -> Trace {
        let mut readers = vec![Vec::new(); sim.netlist().net_count];
        for (i, signal) in signals.iter().enumerate() {
            for &net in &signal.nets {
                if !readers[net as usize].contains(&(i as u32)) {
                    readers[net as usize].push(i as u32);
                }
            }
        }
        let time = sim.time();
        let changes = signals.iter().map(|s| vec![(time, sim.values(&s.nets))]).collect();
        let recorded = signals.len();
        Trace { signals, changes, start: time, end: time, marks: Vec::new(), readers, recorded }
    }

    /// Records whatever changed among `nets` (as returned by
    /// [`Simulator::step`]).
    pub fn record(&mut self, sim: &Simulator, nets: &[NetId]) {
        let time = sim.time();
        let mut touched: Vec<u32> = nets.iter().flat_map(|&n| self.readers[n as usize].iter().copied()).collect();
        touched.sort_unstable();
        touched.dedup();
        for i in touched {
            self.update(sim, i as usize, time);
        }
        self.end = self.end.max(time);
        if self.recorded > MAX_CHANGES {
            self.trim((self.start + self.end) / 2);
        }
    }

    /// Records every signal; use after setting inputs.
    pub fn record_all(&mut self, sim: &Simulator) {
        let time = sim.time();
        for i in 0..self.signals.len() {
            self.update(sim, i, time);
        }
        self.end = self.end.max(time);
    }

    pub fn mark(&mut self, time: u64) {
        if self.marks.last() != Some(&time) {
            self.marks.push(time);
        }
    }

    fn update(&mut self, sim: &Simulator, i: usize, time: u64) {
        let nets = &self.signals[i].nets;
        let history = &mut self.changes[i];
        let (last_time, last) = history.last_mut().expect("every signal starts with a value");
        let same = nets.iter().zip(last.iter()).all(|(&n, &v)| sim.value(n) == v);
        if same {
            return;
        }
        let now = sim.values(nets);
        if *last_time == time {
            // Changed twice in the same step (an input set mid-step).
            *last = now;
        } else {
            history.push((time, now));
            self.recorded += 1;
        }
    }

    /// A signal's value at a step, or `None` before the history starts.
    pub fn value_at(&self, signal: usize, time: u64) -> Option<&[Logic]> {
        let history = &self.changes[signal];
        let after = history.partition_point(|(t, _)| *t <= time);
        after.checked_sub(1).map(|i| history[i].1.as_slice())
    }

    pub fn index_of(&self, path: &str) -> Option<usize> {
        self.signals.iter().position(|s| s.path == path)
    }

    /// Forgets everything before `cutoff`, keeping each signal's value at
    /// that moment.
    pub fn trim(&mut self, cutoff: u64) {
        if cutoff <= self.start {
            return;
        }
        self.recorded = 0;
        for history in &mut self.changes {
            let keep_from = history.partition_point(|(t, _)| *t <= cutoff).saturating_sub(1);
            history.drain(..keep_from);
            if let Some(first) = history.first_mut() {
                first.0 = first.0.max(cutoff);
            }
            self.recorded += history.len();
        }
        self.marks.retain(|&m| m >= cutoff);
        self.start = cutoff;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::bits_from_u64;
    use crate::{Design, Netlist, Project};

    #[test]
    fn records_a_carry_rippling_through_an_adder() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../chips");
        let project = Project::load(&dir).unwrap();
        let design = Design::check(&project);
        let netlist = Netlist::build(&design, "Add4").unwrap();
        let top: Vec<Signal> = netlist.signals.iter().filter(|s| s.depth == 0).cloned().collect();
        let mut sim = Simulator::new(netlist);
        sim.settle(1000).unwrap();
        let mut trace = Trace::new(&sim, top);

        // 0xF + 0x1: every bit carries.
        sim.set_input(0, &bits_from_u64(0xF, 4));
        sim.set_input(1, &bits_from_u64(0x1, 4));
        trace.record_all(&sim);
        trace.mark(sim.time());
        while !sim.is_settled() {
            let changed = sim.step().to_vec();
            trace.record(&sim, &changed);
        }

        // Each adder's carry rises after the one below it.
        let rose = |path: &str| {
            let i = trace.index_of(path).unwrap();
            trace.changes[i].iter().find(|(_, v)| v.as_slice() == [Logic::One]).map(|(t, _)| *t).unwrap()
        };
        let times: Vec<u64> = ["fa0.cout", "fa1.cout", "fa2.cout", "fa3.cout"].iter().map(|p| rose(p)).collect();
        assert!(times.windows(2).all(|w| w[0] < w[1]), "carries should ripple in order: {times:?}");
        let sum = trace.index_of("sum").unwrap();
        assert_eq!(trace.value_at(sum, trace.end), Some(bits_from_u64(0, 4).as_slice()));
        assert_eq!(trace.marks, vec![trace.start]);
    }

    #[test]
    fn trimming_keeps_the_value_at_the_cutoff() {
        let project = Project::from_sources(vec![("t.chip".into(), "chip T {\n in a\n out y\n y = Not(a)\n}".into())]);
        let design = Design::check(&project);
        let netlist = Netlist::build(&design, "T").unwrap();
        let signals = netlist.signals.clone();
        let mut sim = Simulator::new(netlist);
        let mut trace = Trace::new(&sim, signals);
        for _ in 0..3 {
            for bit in [Logic::One, Logic::Zero] {
                sim.set_input(0, &[bit]);
                trace.record_all(&sim);
                while !sim.is_settled() {
                    let changed = sim.step().to_vec();
                    trace.record(&sim, &changed);
                }
            }
        }
        let y = trace.index_of("y").unwrap();
        let before = trace.value_at(y, 4).unwrap().to_vec();
        trace.trim(4);
        assert_eq!(trace.start, 4);
        assert_eq!(trace.value_at(y, 4).unwrap(), before.as_slice());
        assert_eq!(trace.value_at(y, 3), None);
    }
}
