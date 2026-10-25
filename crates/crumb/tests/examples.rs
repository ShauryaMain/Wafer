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
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../chips/bounce.asm");
    let program = assemble(&std::fs::read_to_string(path).unwrap()).unwrap();
    let var = |name: &str| usize::from(program.symbol(name).unwrap().value - crumb::isa::RAM_START);
    let mut m = Machine::new(&program.words);
    let mut bounced = (false, false);
    for _ in 0..200 {
        m.run_frames(1);
        let (x, y) = (usize::from(m.ram[var("x")]), usize::from(m.ram[var("y")]));
        assert!((1..=123).contains(&x) && (1..=59).contains(&y), "the ball stays inside the walls: ({x}, {y})");
        bounced.0 |= m.ram[var("dx")] == 0xFFFF;
        bounced.1 |= m.ram[var("dy")] == 0xFFFF;
    }
    assert!(bounced.0 && bounced.1, "it turned around in both directions");
    // The ball is drawn where x and y say, 4×4, and nothing else is lit.
    let (x, y) = (usize::from(m.ram[var("x")]), usize::from(m.ram[var("y")]));
    assert!(m.pixel(x, y) && m.pixel(x + 3, y + 3), "the ball is at ({x}, {y})\n{}", m.screen_text());
    assert!(!m.pixel(x + 4, y) && !m.pixel(x, y - 1), "and is only 4×4");
    let corners = [(0, 0), (127, 0), (0, 63), (127, 63), (0, 30), (127, 30), (64, 0), (64, 63)];
    assert!(corners.iter().all(|&(cx, cy)| m.pixel(cx, cy)), "the border is whole");
    let lit = m.screen_text().matches('#').count();
    assert_eq!(lit, 128 * 2 + 62 * 2 + 16, "border plus ball, nothing else");
}

#[test]
fn buttons_shows_the_gamepad() {
    let mut m = machine("buttons.asm");
    m.run_frames(2);
    assert_eq!((m.leds, m.number), (0, 0x4020), "nothing held: the dot waits at (64, 32)");
    assert!(m.pixel(64, 32));
    // Hold right and A for three frames: the LEDs show them and the dot moves.
    m.pad = 8 | 16;
    m.run_frames(3);
    assert_eq!(m.leds, 8 | 16);
    assert_eq!(m.number, 0x4320, "three pixels to the right");
    assert!(m.pixel(67, 32) && !m.pixel(64, 32), "the dot moved, and its old spot is dark\n{}", m.screen_text());
    // Up past the top edge comes back at the bottom.
    m.pad = 1;
    m.run_frames(40);
    assert_eq!(m.number & 0xFF, 56, "32 - 40 wraps round to 56");
    assert_eq!(m.screen_text().matches('#').count(), 1, "only the dot is lit");
}
