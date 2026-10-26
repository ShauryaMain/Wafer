//! Pong, in `chips/pong.asm`, played on the reference Crumb with scripted
//! button presses. Its state is read through its `var`s by name, and the
//! screen is checked pixel by pixel against what those vars say should be
//! drawn. `WAFER_CHIPS` points it at another folder.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crumb::isa::{RAM_START, SCREEN_HEIGHT, SCREEN_WIDTH};
use crumb::{Machine, Program, assemble};

const BTN_UP: u16 = 1;
const BTN_DOWN: u16 = 2;
const BTN_START: u16 = 64;

fn chips_dir() -> PathBuf {
    std::env::var_os("WAFER_CHIPS")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../chips"), PathBuf::from)
}

/// Pong, assembled and just powered on.
fn pong() -> (Machine, Program) {
    let source = std::fs::read_to_string(chips_dir().join("pong.asm")).expect("chips/pong.asm is readable");
    let program = assemble(&source).unwrap_or_else(|errors| {
        let messages: Vec<String> = errors.iter().map(|e| e.message.clone()).collect();
        panic!("pong.asm doesn't assemble:\n{}", messages.join("\n"))
    });
    (Machine::new(&program.words), program)
}

fn addr(program: &Program, name: &str) -> usize {
    let symbol = program.symbol(name).unwrap_or_else(|| panic!("pong.asm has no `var {name}`"));
    usize::from(symbol.value - RAM_START)
}

/// A `var`'s value, signed.
fn var(m: &Machine, program: &Program, name: &str) -> i16 {
    m.ram[addr(program, name)] as i16
}

fn set(m: &mut Machine, program: &Program, name: &str, value: i16) {
    m.ram[addr(program, name)] = value as u16;
}

type Pixels = BTreeSet<(usize, usize)>;

fn lit(m: &Machine) -> Pixels {
    (0..SCREEN_HEIGHT).flat_map(|y| (0..SCREEN_WIDTH).map(move |x| (x, y))).filter(|&(x, y)| m.pixel(x, y)).collect()
}

/// Turns `pixels` on where they're off and off where they're on, the way
/// the game draws everything that moves.
fn flip(into: &mut Pixels, pixels: impl IntoIterator<Item = (usize, usize)>) {
    for p in pixels {
        if !into.remove(&p) {
            into.insert(p);
        }
    }
}

/// The walls along rows 0 and 63, and the net: x = 64 on rows 2 to 61,
/// two on and two off.
fn court() -> Pixels {
    let walls = (0..SCREEN_WIDTH).flat_map(|x| [(x, 0), (x, SCREEN_HEIGHT - 1)]);
    let net = (2..=61).filter(|y| y & 2 != 0).map(|y| (64, y));
    walls.chain(net).collect()
}

/// A paddle 2 pixels wide from `x`, 12 rows down from `top`.
fn paddle(x: usize, top: i16) -> Vec<(usize, usize)> {
    (top as usize..top as usize + 12).flat_map(|y| [(x, y), (x + 1, y)]).collect()
}

fn ball(x: i16, y: i16) -> Vec<(usize, usize)> {
    let (x, y) = (x as usize, y as usize);
    vec![(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)]
}

/// The 3×5 digits, a row at a time, the leftmost pixel in bit 2.
const FONT: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b011, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// Digit `n` with its top-left pixel at (`x`, 3).
fn digit(n: i16, x: usize) -> Vec<(usize, usize)> {
    let rows = FONT[n as usize];
    (0..5).flat_map(|r| (0..3).filter(move |c| rows[r] & (4 >> c) != 0).map(move |c| (x + c, 3 + r))).collect()
}

/// Everything that should be on the screen, worked out from the vars: the
/// court, both paddles, both scores (yours left of the net, the
/// computer's right), and the ball while it's in play.
fn scene(m: &Machine, p: &Program) -> Pixels {
    let mut want = court();
    flip(&mut want, paddle(2, var(m, p, "player-y")));
    flip(&mut want, paddle(124, var(m, p, "computer-y")));
    flip(&mut want, digit(var(m, p, "player-score"), 56));
    flip(&mut want, digit(var(m, p, "computer-score"), 70));
    if var(m, p, "serve") == 0 && var(m, p, "game-over") == 0 {
        flip(&mut want, ball(var(m, p, "ball-x"), var(m, p, "ball-y")));
    }
    want
}

/// Fails with what's different if the screen isn't exactly the scene.
#[track_caller]
fn assert_drawn(m: &Machine, p: &Program, when: &str) {
    let (got, want) = (lit(m), scene(m, p));
    if got != want {
        let extra: Vec<_> = got.difference(&want).take(8).collect();
        let missing: Vec<_> = want.difference(&got).take(8).collect();
        panic!(
            "{when}: the screen isn't what the vars say\nlit but shouldn't be: {extra:?}\nshould be lit but isn't: {missing:?}\n{}",
            m.screen_text()
        );
    }
}

/// Puts the ball somewhere, the way the game would have drawn it there.
/// Call between frames.
fn place_ball(m: &mut Machine, p: &Program, x: i16, y: i16, dx: i16, dy: i16) {
    let shown = var(m, p, "serve") == 0 && var(m, p, "game-over") == 0;
    let mut screen = lit(m);
    if shown {
        flip(&mut screen, ball(var(m, p, "ball-x"), var(m, p, "ball-y")));
    }
    for (name, value) in [("ball-x", x), ("ball-y", y), ("ball-dx", dx), ("ball-dy", dy), ("serve", 0)] {
        set(m, p, name, value);
    }
    flip(&mut screen, ball(x, y));
    draw(m, &screen);
}

/// Changes a score, the way the game would have drawn it.
fn place_score(m: &mut Machine, p: &Program, who: &str, value: i16) {
    let (name, x) = if who == "player" { ("player-score", 56) } else { ("computer-score", 70) };
    let mut screen = lit(m);
    flip(&mut screen, digit(var(m, p, name), x));
    set(m, p, name, value);
    flip(&mut screen, digit(value, x));
    draw(m, &screen);
}

/// Puts the computer's paddle somewhere, the way the game would have.
fn place_computer(m: &mut Machine, p: &Program, top: i16) {
    let mut screen = lit(m);
    flip(&mut screen, paddle(124, var(m, p, "computer-y")));
    set(m, p, "computer-y", top);
    flip(&mut screen, paddle(124, top));
    draw(m, &screen);
}

fn draw(m: &mut Machine, pixels: &Pixels) {
    m.screen.iter_mut().for_each(|w| *w = 0);
    for &(x, y) in pixels {
        m.screen[y * 8 + x / 16] |= 0x8000 >> (x % 16);
    }
}

/// Pong after the first two frames, ball not served yet.
fn started() -> (Machine, Program) {
    let (mut m, p) = pong();
    m.run_frames(2);
    (m, p)
}

// The court and the paddles ----------------------------------------------

#[test]
fn the_court_paddles_and_scores_are_drawn() {
    let (m, p) = started();
    assert_eq!((var(&m, &p, "player-y"), var(&m, &p, "computer-y")), (26, 26), "both paddles start in the middle");
    assert_eq!((var(&m, &p, "player-score"), var(&m, &p, "computer-score")), (0, 0));
    assert_drawn(&m, &p, "at the start");
    assert!(court().is_subset(&lit(&m)), "the walls and the net");
}

#[test]
fn up_and_down_move_your_paddle() {
    let (mut m, p) = started();
    m.pad = BTN_DOWN;
    m.run_frames(5);
    assert_eq!(var(&m, &p, "player-y"), 36, "five frames of down: two pixels a frame");
    assert_drawn(&m, &p, "after moving down");
    m.pad = 0;
    m.run_frames(3);
    assert_eq!(var(&m, &p, "player-y"), 36, "letting go stops it");
    m.pad = BTN_UP;
    m.run_frames(3);
    assert_eq!(var(&m, &p, "player-y"), 30, "three frames of up");
    assert_drawn(&m, &p, "after moving up");
}

#[test]
fn your_paddle_stops_at_the_walls() {
    let (mut m, p) = started();
    m.pad = BTN_UP;
    m.run_frames(20);
    assert_eq!(var(&m, &p, "player-y"), 1, "just under the top wall");
    assert_drawn(&m, &p, "at the top");
    m.pad = BTN_DOWN;
    m.run_frames(40);
    assert_eq!(var(&m, &p, "player-y"), 51, "just above the bottom wall");
    assert_drawn(&m, &p, "at the bottom");
}

// The ball ------------------------------------------------------------------

#[test]
fn the_ball_is_served_from_the_middle_after_a_pause() {
    let (mut m, p) = pong();
    m.run_frames(30);
    assert!(var(&m, &p, "serve") > 0, "no ball yet");
    assert_drawn(&m, &p, "waiting to serve");
    m.run_frames(12);
    assert_eq!(var(&m, &p, "serve"), 0, "served within 41 frames");
    let (x, dx, dy) = (var(&m, &p, "ball-x"), var(&m, &p, "ball-dx"), var(&m, &p, "ball-dy"));
    assert_eq!(dx, 1, "towards the computer first");
    assert!(dy == 1 || dy == -1, "at a slant: {dy}");
    assert!((63..66).contains(&x), "from the middle: x = {x}");
    assert_drawn(&m, &p, "just served");
}

#[test]
fn the_ball_moves_a_pixel_a_frame_and_bounces_off_the_walls() {
    let (mut m, p) = started();
    place_ball(&mut m, &p, 40, 3, 1, -1);
    let mut seen = Vec::new();
    for _ in 0..4 {
        m.run_frames(1);
        seen.push((var(&m, &p, "ball-x"), var(&m, &p, "ball-y"), var(&m, &p, "ball-dy")));
        assert_drawn(&m, &p, "near the top wall");
    }
    assert_eq!(seen, [(41, 2, -1), (42, 1, 1), (43, 2, 1), (44, 3, 1)], "(x, y, dy) frame by frame");
    // Two rows a frame towards the bottom: it stops at the wall, then turns.
    place_ball(&mut m, &p, 40, 60, 1, 2);
    m.run_frames(1);
    assert_eq!((var(&m, &p, "ball-y"), var(&m, &p, "ball-dy")), (61, -2));
    assert_drawn(&m, &p, "at the bottom wall");
}

#[test]
fn your_paddle_sends_the_ball_back_at_an_angle() {
    // Where the ball meets the paddle sets its slope: the edges send it off
    // steeply, the middle straight back.
    for (offset, dy) in [(0, -2), (3, -1), (6, 0), (9, 1), (12, 2)] {
        let (mut m, p) = started();
        // Your paddle covers rows 26 to 37. `offset` is the ball's lower row
        // counted from the paddle's top.
        place_ball(&mut m, &p, 6, 26 + offset - 1, -1, 0);
        m.run_frames(2);
        assert_eq!(var(&m, &p, "ball-x"), 4, "it reached the paddle");
        assert_eq!((var(&m, &p, "ball-dx"), var(&m, &p, "ball-dy")), (1, dy), "hit at {offset}");
        m.run_frames(1);
        assert_eq!(var(&m, &p, "ball-x"), 5, "and it's on its way back");
        assert_drawn(&m, &p, "after the hit");
    }
}

#[test]
fn a_ball_you_miss_is_the_computers_point() {
    let (mut m, p) = started();
    place_ball(&mut m, &p, 8, 50, -1, 0);
    m.run_frames(7);
    assert_eq!(var(&m, &p, "ball-x"), 1, "it went past your paddle");
    m.run_frames(1);
    assert_eq!((var(&m, &p, "player-score"), var(&m, &p, "computer-score")), (0, 1));
    assert!(var(&m, &p, "serve") > 0, "the next serve waits");
    assert_eq!(var(&m, &p, "ball-dx"), -1, "and comes to you, who lost the point");
    assert_eq!(m.number, 0x0001, "the number display shows the score: yours, then the computer's");
    assert_drawn(&m, &p, "after the point");
    m.run_frames(45);
    assert_eq!(var(&m, &p, "serve"), 0);
    assert_drawn(&m, &p, "served again");
}

// The computer ------------------------------------------------------------

#[test]
fn the_computer_follows_the_ball_coming_its_way() {
    let (mut m, p) = started();
    place_ball(&mut m, &p, 70, 10, 1, 0);
    m.run_frames(8);
    assert_eq!(var(&m, &p, "computer-y"), 18, "up a pixel a frame, towards the ball");
    assert_drawn(&m, &p, "following");
    m.run_frames(20);
    assert_eq!(var(&m, &p, "computer-y"), 5, "and level with it: its middle on the ball's");
}

#[test]
fn the_computer_goes_back_to_the_middle_when_the_ball_goes_away() {
    let (mut m, p) = started();
    place_computer(&mut m, &p, 40);
    place_ball(&mut m, &p, 100, 10, -1, 0);
    m.run_frames(5);
    assert_eq!(var(&m, &p, "computer-y"), 35);
    assert_drawn(&m, &p, "going back");
}

#[test]
fn the_computer_sends_the_ball_back() {
    let (mut m, p) = started();
    place_ball(&mut m, &p, 118, 31, 1, 0);
    m.run_frames(4);
    assert_eq!(var(&m, &p, "ball-x"), 122, "it reached the computer's paddle");
    assert_eq!(var(&m, &p, "ball-dx"), -1, "and comes back");
    assert_drawn(&m, &p, "after the computer's hit");
}

#[test]
fn the_computer_can_be_beaten() {
    // Too far away to get there in time.
    let (mut m, p) = started();
    place_computer(&mut m, &p, 1);
    place_ball(&mut m, &p, 100, 59, 1, 0);
    m.run_frames(30);
    assert_eq!((var(&m, &p, "player-score"), var(&m, &p, "computer-score")), (1, 0), "your point");
    assert_eq!(var(&m, &p, "ball-dx"), 1, "the next serve goes to the computer");
    assert_eq!(m.number, 0x0100);
    assert_drawn(&m, &p, "after your point");
}

// Winning -------------------------------------------------------------------

#[test]
fn first_to_nine_wins_and_start_begins_again() {
    let (mut m, p) = started();
    place_score(&mut m, &p, "player", 8);
    place_computer(&mut m, &p, 1);
    place_ball(&mut m, &p, 100, 59, 1, 0);
    m.run_frames(30);
    assert_eq!(var(&m, &p, "player-score"), 9);
    assert_ne!(var(&m, &p, "game-over"), 0, "you won");
    assert_eq!(m.leds, 0xFFFF, "every LED lights up for you");
    assert_drawn(&m, &p, "you won");
    // Nothing happens until Start...
    m.run_frames(60);
    assert_eq!(var(&m, &p, "player-score"), 9);
    assert_drawn(&m, &p, "still over");
    // ...which begins a new game.
    m.pad = BTN_START;
    m.run_frames(2);
    m.pad = 0;
    assert_eq!((var(&m, &p, "player-score"), var(&m, &p, "computer-score"), var(&m, &p, "game-over")), (0, 0, 0));
    assert_eq!((m.leds, m.number), (0, 0));
    assert_drawn(&m, &p, "a new game");
    m.run_frames(45);
    assert_eq!(var(&m, &p, "serve"), 0, "with a serve");

    // And the computer can win too: then the LEDs stay dark.
    let (mut m, p) = started();
    place_score(&mut m, &p, "computer", 8);
    place_ball(&mut m, &p, 8, 50, -1, 0);
    m.run_frames(10);
    assert_eq!(var(&m, &p, "computer-score"), 9);
    assert_ne!(var(&m, &p, "game-over"), 0);
    assert_eq!(m.leds, 0);
}

// A whole game ------------------------------------------------------------

/// Steps through one frame of play and counts the instructions it does
/// outside `wait-frame`.
fn busy_frame(m: &mut Machine, p: &Program) -> u64 {
    let (wait, next) = (p.symbol("wait-frame").unwrap().value, word_after(p, "wait-frame"));
    let frame = m.frame();
    let mut busy = 0;
    while m.frame() == frame {
        if !(wait..next).contains(&m.pc) {
            busy += 1;
        }
        m.step();
    }
    busy
}

/// Where the next word after `name` starts.
fn word_after(p: &Program, name: &str) -> u16 {
    let at = p.symbol(name).unwrap().value;
    let words = p.symbols.iter().filter(|s| s.kind == crumb::asm::SymbolKind::Word && s.value > at);
    words.map(|s| s.value).min().unwrap_or(p.words.len() as u16)
}

#[test]
fn a_whole_game_is_drawn_right_and_every_frame_fits() {
    // You chase the ball, meeting it with one end of your paddle or the
    // other so it flies off steeply, which is how the computer gets beaten,
    // but now and then you look away.
    let (mut m, p) = pong();
    let mut busiest = 0;
    let mut frames = 0;
    while var(&m, &p, "game-over") == 0 {
        let middle = var(&m, &p, "player-y") + 6;
        let target = var(&m, &p, "ball-y") + 1 + if frames % 600 < 300 { 5 } else { -5 };
        // Now and then you look away.
        let away = frames % 900 > 650;
        m.pad = if away {
            0
        } else if target < middle - 1 {
            BTN_UP
        } else if target > middle + 1 {
            BTN_DOWN
        } else {
            0
        };
        let busy = busy_frame(&mut m, &p);
        // The first frame draws the whole court, once, before play starts.
        if frames > 0 {
            busiest = busiest.max(busy);
        }
        frames += 1;
        if frames % 10 == 0 {
            assert_drawn(&m, &p, &format!("frame {frames}"));
        }
        assert!(frames < 20_000, "the game ended in time");
    }
    let (you, computer) = (var(&m, &p, "player-score"), var(&m, &p, "computer-score"));
    assert!(you == 9 || computer == 9, "someone got to 9: {you}-{computer}");
    assert!(you > 0 && computer > 0, "both scored: {you}-{computer}");
    assert!(busiest < 2048 / 2, "the busiest frame used {busiest} of 2048 instructions");
    eprintln!("{you}-{computer} after {frames} frames; the busiest frame used {busiest} of 2048 instructions");
}
