//! Runs the current chip's simulation in real time: play, pause, step, and
//! a speed in gate delays per second.

use std::collections::HashMap;

use wafer_core::logic::Logic;
use wafer_core::testing::{self, BuiltinCheck, TestOutcome};
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
        match Netlist::build(&ws.design, chip) {
            Ok(netlist) => {
                let mut sim = Simulator::new(netlist);
                for (name, bits) in &self.remembered {
                    let fits = sim.netlist().inputs.iter().any(|s| &s.path == name && s.nets.len() == bits.len());
                    if fits {
                        sim.set_input_by_name(name, bits);
                    }
                }
                self.sim = Some(sim);
                self.error = None;
            }
            Err(e) => {
                self.sim = None;
                self.error = Some(e.to_string());
            }
        }
    }

    pub fn read(&self, path: &str) -> Option<Vec<Logic>> {
        self.sim.as_ref()?.read(path)
    }

    pub fn set_input(&mut self, name: &str, bits: Vec<Logic>) {
        if let Some(sim) = &mut self.sim
            && sim.set_input_by_name(name, &bits)
        {
            self.remembered.insert(name.to_string(), bits);
            self.steps = 0;
        }
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
        let Some(sim) = &mut self.sim else { return false };
        if !self.playing || sim.is_settled() {
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
            if sim.is_settled() {
                break;
            }
            sim.step();
            self.steps += 1;
        }
        !sim.is_settled()
    }

    /// One gate delay, whether playing or paused.
    pub fn step(&mut self) {
        if let Some(sim) = &mut self.sim
            && !sim.is_settled()
        {
            sim.step();
            self.steps += 1;
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
