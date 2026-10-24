//! Runs `test` blocks, and checks home-made gates against the built-in ones.

use crate::check::{Design, TestInfo, TestStepInfo};
use crate::diag::Span;
use crate::gate::GateKind;
use crate::logic::{Logic, format_bits};
use crate::netlist::{BuildError, Netlist};
use crate::sim::Simulator;

/// How long a row may take to settle before it's reported as oscillating.
pub const MAX_STEPS: u64 = 10_000;

pub struct TestOutcome {
    pub chip: String,
    pub span: Span,
    pub rows: Vec<RowOutcome>,
    /// Set when the chip couldn't be built at all.
    pub error: Option<BuildError>,
}

impl TestOutcome {
    pub fn passed(&self) -> bool {
        self.error.is_none() && self.rows.iter().all(RowOutcome::passed)
    }

    pub fn passed_rows(&self) -> usize {
        self.rows.iter().filter(|r| r.passed()).count()
    }
}

/// The result of one check: a truth-table row or an `expect` step.
pub struct RowOutcome {
    pub span: Span,
    /// Every input and its value at the time.
    pub inputs: Vec<(String, String)>,
    pub failures: Vec<Failure>,
    pub unsettled: bool,
    /// Time steps until the last signal stopped changing.
    pub steps: u64,
    /// Clock ticks so far.
    pub ticks: u64,
}

impl RowOutcome {
    pub fn passed(&self) -> bool {
        self.failures.is_empty() && !self.unsettled
    }
}

pub struct Failure {
    pub output: String,
    pub expected: String,
    pub actual: String,
}

/// Runs every step in order. State carries over, so a latch can be set in
/// one row and checked in the next, and `tick` moves the clock.
pub fn run_test(design: &Design, test: &TestInfo) -> TestOutcome {
    let mut outcome = TestOutcome { chip: test.chip.clone(), span: test.span, rows: Vec::new(), error: None };
    let netlist = match Netlist::build(design, &test.chip) {
        Ok(netlist) => netlist,
        Err(error) => {
            outcome.error = Some(error);
            return outcome;
        }
    };
    let mut sim = Simulator::new(netlist);
    for step in &test.steps {
        match step {
            TestStepInfo::Row { span, inputs, checks } => {
                for (port, bits) in inputs {
                    sim.set_input(*port, bits);
                }
                let settled = sim.settle(MAX_STEPS);
                outcome.rows.push(check(&sim, *span, checks, settled));
            }
            TestStepInfo::Set { inputs, .. } => {
                for (port, bits) in inputs {
                    sim.set_input(*port, bits);
                }
            }
            TestStepInfo::Tick { span, count } => {
                for _ in 0..*count {
                    if sim.tick(MAX_STEPS).is_err() {
                        let mut row = check(&sim, *span, &[], Err(crate::sim::Unsettled { steps: MAX_STEPS }));
                        row.unsettled = true;
                        outcome.rows.push(row);
                        return outcome;
                    }
                }
            }
            TestStepInfo::Expect { span, checks } => {
                let settled = sim.settle(MAX_STEPS);
                outcome.rows.push(check(&sim, *span, checks, settled));
            }
        }
    }
    outcome
}

fn check(
    sim: &Simulator,
    span: Span,
    checks: &[(String, Option<Vec<Logic>>)],
    settled: Result<u64, crate::sim::Unsettled>,
) -> RowOutcome {
    let netlist = sim.netlist();
    let inputs = netlist.inputs.iter().map(|s| (s.path.clone(), format_bits(&sim.values(&s.nets)))).collect();
    let mut failures = Vec::new();
    for (path, expected) in checks {
        let Some(expected) = expected else { continue };
        let actual = sim.read(path).unwrap_or_default();
        if &actual != expected {
            failures.push(Failure {
                output: path.clone(),
                expected: format_bits(expected),
                actual: format_bits(&actual),
            });
        }
    }
    RowOutcome {
        span,
        inputs,
        failures,
        unsettled: settled.is_err(),
        steps: settled.unwrap_or(MAX_STEPS),
        ticks: sim.ticks(),
    }
}

/// The result of comparing a home-made gate with the built-in one.
pub struct BuiltinCheck {
    pub chip: String,
    pub gate: GateKind,
    pub cases: usize,
    pub mismatches: Vec<BuiltinMismatch>,
    pub error: Option<BuildError>,
}

impl BuiltinCheck {
    pub fn passed(&self) -> bool {
        self.error.is_none() && self.mismatches.is_empty()
    }
}

pub struct BuiltinMismatch {
    pub inputs: Vec<Logic>,
    pub expected: Logic,
    pub actual: Logic,
}

/// When a project defines its own version of a built-in gate (like an `Xor`
/// made of `Nand`s), it replaces the built-in everywhere in the project. This
/// tries every input combination on both and compares them.
///
/// Returns `None` if the chip isn't named after a built-in gate or doesn't
/// have the same shape (same number of 1-bit inputs, one 1-bit output).
pub fn check_against_builtin(design: &Design, chip: &str) -> Option<BuiltinCheck> {
    let gate = GateKind::from_name(chip).filter(|g| !g.is_sequential())?;
    let info = design.chips.get(chip)?;
    let n = gate.inputs().len();
    let same_shape = info.inputs.len() == n
        && info.outputs.len() == 1
        && info.inputs.iter().chain(&info.outputs).all(|p| p.width == 1);
    if !same_shape {
        return None;
    }
    let mut check = BuiltinCheck { chip: chip.into(), gate, cases: 1 << n, mismatches: Vec::new(), error: None };
    let netlist = match Netlist::build(design, chip) {
        Ok(netlist) => netlist,
        Err(error) => {
            check.error = Some(error);
            return Some(check);
        }
    };
    let mut sim = Simulator::new(netlist);
    for case in 0..check.cases {
        // First input is the most significant bit, like a truth table.
        let inputs: Vec<Logic> = (0..n).map(|i| Logic::from_bool((case >> (n - 1 - i)) & 1 == 1)).collect();
        for (port, &bit) in inputs.iter().enumerate() {
            sim.set_input(port, &[bit]);
        }
        let _ = sim.settle(MAX_STEPS);
        let actual = sim.value(sim.netlist().outputs[0].nets[0]);
        let input = |i: usize| *inputs.get(i).unwrap_or(&inputs[0]);
        let expected = gate.eval(input(0), input(1), input(2));
        if actual != expected {
            check.mismatches.push(BuiltinMismatch { inputs, expected, actual });
        }
    }
    Some(check)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    fn design(text: &str) -> Design {
        let project = Project::from_sources(vec![("test.chip".into(), text.into())]);
        assert!(project.diagnostics.is_empty(), "{:#?}", project.diagnostics);
        let design = Design::check(&project);
        assert!(!design.has_errors(), "{:#?}", design.diagnostics);
        design
    }

    #[test]
    fn reports_failing_rows() {
        let d = design("chip A {\n in a, b\n out y\n y = Or(a, b)\n}\ntest A {\n 0 0 | 0\n 1 1 | 1\n 1 0 | 0\n}");
        let outcome = run_test(&d, &d.tests[0]);
        assert_eq!(outcome.passed_rows(), 2);
        let failed = &outcome.rows[2];
        assert_eq!(failed.failures[0].output, "y");
        assert_eq!(failed.failures[0].expected, "0");
        assert_eq!(failed.failures[0].actual, "1");
    }

    #[test]
    fn home_made_gates_are_compared_with_built_ins() {
        let good = design("chip Not {\n in a\n out out\n out = Nand(a, a)\n}");
        assert!(check_against_builtin(&good, "Not").unwrap().passed());

        let bad = design("chip Xor {\n in a, b\n out out\n out = Or(a, b)\n}");
        let check = check_against_builtin(&bad, "Xor").unwrap();
        assert_eq!(check.mismatches.len(), 1);
        assert_eq!(check.mismatches[0].inputs, vec![Logic::One, Logic::One]);
    }
}
