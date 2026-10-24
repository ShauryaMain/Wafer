//! Runs the current chip's simulation in real time: play, pause, step, and
//! a speed in gate delays per second. Every change is recorded for the
//! timeline, which can also wind the view back to an earlier step.

use std::collections::HashMap;

use wafer_core::logic::Logic;
use wafer_core::testing::{self, BuiltinCheck, TestOutcome};
use wafer_core::trace::Trace;
use wafer_core::{Netlist, Simulator};

use crate::workspace::Workspace;

pub struct SimRunner {
    pub chip: String,
    generation: u64,
    pub sim: Option<Simulator>,
    /// Why the chip can't be simulated, if it can't.
    pub error: Option<String>,
    /// Input values by name, kept when the chip is edited and rebuilt.
    remembered: HashMap<String, Vec<Logic>>,
    pub playing: bool,
    /// Gate delays per second.
    pub speed: f32,
    /// Settle immediately instead of animating.
    pub instant: bool,
    budget: f32,
    /// Gate delays since an input last changed.
    pub steps: u64,
    /// History of the chip's own signals, for the timeline.
    pub trace: Option<Trace>,
    /// When set, the canvas shows values from this step instead of now.
    pub cursor: Option<u64>,
}

impl Default for SimRunner {
    fn default() -> SimRunner {
        SimRunner::new()
    }
}

impl SimRunner {
    pub fn new() -> SimRunner {
        SimRunner {
            chip: String::new(),
            generation: 0,
            sim: None,
            error: None,
            remembered: HashMap::new(),
            playing: true,
            speed: 20.0,
            instant: false,
            budget: 0.0,
            steps: 0,
            trace: None,
            cursor: None,
        }
    }

    /// Rebuilds the simulator when the chip or its files change.
    pub fn sync(&mut self, ws: &Workspace, chip: &str) {
        if chip == self.chip && ws.generation == self.generation {
            return;
        }
        if chip != self.chip {
            self.remembered.clear();
        }
        self.chip = chip.to_string();
        self.generation = ws.generation;
        self.steps = 0;
        self.cursor = None;
        self.trace = None;
        match Netlist::build(&ws.design, chip) {
            Ok(netlist) => {
                let mut sim = Simulator::new(netlist);
                for (name, bits) in &self.remembered {
                    let fits = sim.netlist().inputs.iter().any(|s| &s.path == name && s.nets.len() == bits.len());
                    if fits {
                        sim.set_input_by_name(name, bits);
                    }
                }
                // Everything at every level, so diving into a chip can show
                // (and scrub) its insides too.
                self.trace = Some(Trace::new(&sim, sim.netlist().signals.clone()));
                self.sim = Some(sim);
                self.error = None;
            }
            Err(e) => {
                self.sim = None;
                self.error = Some(e.to_string());
            }
        }
    }

    /// Starts the chip over: every gate back to unknown, inputs kept.
    pub fn reset(&mut self) {
        self.generation = 0;
    }

    /// A signal's value right now, ignoring the timeline cursor.
    pub fn read(&self, path: &str) -> Option<Vec<Logic>> {
        self.sim.as_ref()?.read(path)
    }

    /// A signal's value as the canvas should show it: at the timeline
    /// cursor if one is set, otherwise now.
    pub fn value(&self, path: &str) -> Option<Vec<Logic>> {
        if let (Some(time), Some(trace)) = (self.cursor, &self.trace)
            && let Some(index) = trace.index_of(path)
        {
            return trace.value_at(index, time).map(<[Logic]>::to_vec);
        }
        self.read(path)
    }

    /// The current time step.
    pub fn now(&self) -> u64 {
        self.sim.as_ref().map_or(0, Simulator::time)
    }

    pub fn set_input(&mut self, name: &str, bits: Vec<Logic>) {
        if let Some(sim) = &mut self.sim
            && sim.set_input_by_name(name, &bits)
        {
            self.remembered.insert(name.to_string(), bits);
            self.steps = 0;
            self.cursor = None;
            if let Some(trace) = &mut self.trace {
                trace.record_all(sim);
                trace.mark(sim.time());
            }
        }
    }

    fn step_once(&mut self) {
        let Some(sim) = &mut self.sim else { return };
        let changed = sim.step().to_vec();
        if let Some(trace) = &mut self.trace {
            trace.record(sim, &changed);
        }
        self.steps += 1;
    }

    /// Flips a 1-bit input between 0 and 1.
    pub fn toggle(&mut self, name: &str) {
        let current = self.read(name);
        if let Some([bit]) = current.as_deref() {
            let next = if *bit == Logic::One { Logic::Zero } else { Logic::One };
            self.set_input(name, vec![next]);
        }
    }

    pub fn is_settled(&self) -> bool {
        self.sim.as_ref().is_none_or(Simulator::is_settled)
    }

    /// Advances by `dt` seconds of real time. Returns true while signals are
    /// still changing, so the caller keeps repainting.
    pub fn advance(&mut self, dt: f32) -> bool {
        if !self.playing || self.is_settled() {
            self.budget = 0.0;
            return false;
        }
        let steps = if self.instant {
            testing::MAX_STEPS
        } else {
            self.budget += dt * self.speed;
            let whole = self.budget.floor();
            self.budget -= whole;
            whole as u64
        };
        for _ in 0..steps {
            if self.is_settled() {
                break;
            }
            self.step_once();
        }
        !self.is_settled()
    }

    /// One gate delay, whether playing or paused.
    pub fn step(&mut self) {
        if !self.is_settled() {
            self.step_once();
        }
    }

    pub fn status(&self) -> String {
        if let Some(error) = &self.error {
            return format!("can't simulate: {error}");
        }
        let delays = |n: u64| if n == 1 { "1 gate delay".to_string() } else { format!("{n} gate delays") };
        if self.is_settled() {
            format!("settled after {}", delays(self.steps))
        } else if self.playing {
            format!("running… {}", delays(self.steps))
        } else {
            format!("paused after {}", delays(self.steps))
        }
    }
}

/// The latest test results for the current chip.
pub struct TestResults {
    pub chip: String,
    pub generation: u64,
    pub outcomes: Vec<TestOutcome>,
    pub builtin: Option<BuiltinCheck>,
}

impl TestResults {
    pub fn run(ws: &Workspace, chip: &str) -> TestResults {
        let buildable = ws.design.is_buildable(chip);
        let outcomes = if buildable {
            ws.design.tests.iter().filter(|t| t.chip == chip).map(|t| testing::run_test(&ws.design, t)).collect()
        } else {
            Vec::new()
        };
        let builtin = if buildable { testing::check_against_builtin(&ws.design, chip) } else { None };
        TestResults { chip: chip.to_string(), generation: ws.generation, outcomes, builtin }
    }

    pub fn passed(&self) -> bool {
        self.outcomes.iter().all(TestOutcome::passed) && self.builtin.as_ref().is_none_or(BuiltinCheck::passed)
    }

    pub fn is_empty(&self) -> bool {
        self.outcomes.is_empty() && self.builtin.is_none()
    }
}
