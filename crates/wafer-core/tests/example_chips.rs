//! Every chip in the repo's `chips/` folder must check cleanly, pass its
//! tests, and lay out without panicking.

use std::path::Path;

use wafer_core::testing::{check_against_builtin, run_test};
use wafer_core::{Design, Project, schematic};

#[test]
fn example_chips_pass() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../chips");
    let project = Project::load(&dir).expect("chips/ is readable");
    let design = Design::check(&project);
    let problems: Vec<String> = project
        .diagnostics
        .iter()
        .chain(&design.diagnostics)
        .map(|d| d.render(&project.sources, false))
        .collect();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(!design.tests.is_empty());

    for test in &design.tests {
        let outcome = run_test(&design, test);
        let failed = outcome.rows.iter().filter(|r| !r.passed()).count();
        assert!(outcome.passed(), "{}: {failed} rows failed", test.chip);
    }
    for chip in design.chips.keys() {
        if let Some(check) = check_against_builtin(&design, chip) {
            assert!(check.passed(), "{chip} differs from the built-in");
        }
        assert!(schematic::layout(&design, &project.sources, chip).is_some());
    }
}
