//! The example programs in `chips/` assemble and do what they say.

use std::path::PathBuf;

use crumb::{Machine, assemble};

fn machine(name: &str) -> Machine {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../chips").join(name);
    let source = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let program = assemble(&source).unwrap_or_else(|errors| panic!("{name}: {errors:#?}"));
    Machine::new(&program.words)
}

#[test]
fn hello_prints_a_greeting() {
    let mut m = machine("hello.asm");
    m.run(10_000);
    assert!(m.halted);
    assert_eq!(m.console_text(), "Hello, Crumb!\n");
}

#[test]
fn count_counts_frames() {
    let mut m = machine("count.asm");
    m.run_frames(10);
    // Each new frame is spotted just after it starts.
    assert_eq!(m.number, 9);
    assert_eq!(m.leds, 9);
}

#[test]
fn bounce_bounces_off_the_walls() {
    let mut m = machine("bounce.asm");
    m.run_frames(60);
    // 59 moves from (10, 20): y hits the bottom (59) after 39 and comes
    // back up 20, while x is still heading right.
    let (x, y) = (69, 39);
    assert!(m.pixel(x, y) && m.pixel(x + 3, y + 3), "the ball is at ({x}, {y})\n{}", m.screen_text());
    assert!(!m.pixel(x + 4, y) && !m.pixel(x, y - 1), "and is only 4×4");
    let corners = [(0, 0), (127, 0), (0, 63), (127, 63), (0, 30), (127, 30), (64, 0), (64, 63)];
    assert!(corners.iter().all(|&(cx, cy)| m.pixel(cx, cy)), "the border is whole");
    let lit = m.screen_text().matches('#').count();
    assert_eq!(lit, 128 * 2 + 62 * 2 + 16, "border plus ball, nothing else");
}
