//! Assembles small programs and runs them on the reference Crumb.

use crumb::isa::{self, Insn};
use crumb::{Machine, assemble};

/// Assembles and runs `source` until it halts.
fn run(source: &str) -> Machine {
    let program = assemble(source).unwrap_or_else(|errors| panic!("{errors:#?}"));
    let mut m = Machine::new(&program.words);
    m.run(1_000_000);
    assert!(m.halted, "the program never halted");
    m
}

/// The data stack left by `: main <code> halt ;`.
fn stack(code: &str) -> Vec<u16> {
    run(&format!(": main {code} halt ;")).stack()
}

/// The first error message for `source`.
fn error(source: &str) -> crumb::asm::Error {
    match assemble(source) {
        Ok(_) => panic!("`{source}` assembled"),
        Err(errors) => errors[0].clone(),
    }
}

#[test]
fn stack_words() {
    assert_eq!(stack("1 2"), [1, 2]);
    assert_eq!(stack("1 dup"), [1, 1]);
    assert_eq!(stack("1 2 over"), [1, 2, 1]);
    assert_eq!(stack("1 2 drop"), [1]);
    assert_eq!(stack("1 2 nip"), [2]);
    assert_eq!(stack("1 2 swap"), [2, 1]);
    assert_eq!(stack("1 2 3 rot"), [2, 3, 1]);
    assert_eq!(stack("1 2 tuck"), [2, 1, 2]);
    assert_eq!(stack("1 2 2dup"), [1, 2, 1, 2]);
    assert_eq!(stack("1 2 3 2drop"), [1]);
    assert_eq!(stack("5 >r 6 r@ r>"), [6, 5, 5]);
}

#[test]
fn arithmetic_and_logic() {
    assert_eq!(stack("2 3 +"), [5]);
    assert_eq!(stack("10 3 -"), [7]);
    assert_eq!(stack("3 10 -"), [0xFFF9]);
    assert_eq!(stack("0b1100 0b1010 and"), [0b1000]);
    assert_eq!(stack("0b1100 0b1010 or"), [0b1110]);
    assert_eq!(stack("0b1100 0b1010 xor"), [0b0110]);
    assert_eq!(stack("0 invert"), [0xFFFF]);
    assert_eq!(stack("5 1+ 5 1- 5 negate"), [6, 4, 0xFFFB]);
    assert_eq!(stack("1 4 lshift 0x8000 15 rshift"), [16, 1]);
    assert_eq!(stack("3 2* 12 2/"), [6, 6]);
}

#[test]
fn comparisons_give_forth_flags() {
    assert_eq!(stack("1 2 < 2 1 < 2 2 ="), [0xFFFF, 0, 0xFFFF]);
    assert_eq!(stack("-1 1 < -1 1 u<"), [0xFFFF, 0], "signed vs unsigned");
    assert_eq!(stack("2 1 > 1 2 u> 1 2 <> 0 0= 7 0="), [0xFFFF, 0, 0xFFFF, 0xFFFF, 0]);
    assert_eq!(stack("true false"), [0xFFFF, 0]);
}

#[test]
fn big_and_negative_numbers() {
    assert_eq!(stack("0x7FFF 0x8000 0xFFFF -1 40000 -32768"), [0x7FFF, 0x8000, 0xFFFF, 0xFFFF, 40000, 0x8000]);
    assert_eq!(stack("'A' '\\n' 0b1111_0000"), [65, 10, 0xF0]);
    let program = assemble(": main 0x8000 halt ;").unwrap();
    // A literal and an `invert`.
    assert_eq!(program.words[1], Insn::Lit(0x7FFF).encode());
    assert_eq!(isa::disasm(program.words[2]), "invert");
}

#[test]
fn memory_and_variables() {
    let m = run("var x\nvar buf[4]\nvar y\n: main 42 x ! 7 y ! x @ 1+ buf 2 + ! 5 y +! halt ;");
    assert_eq!(m.read(0x2000), 42, "x is the first word of RAM");
    assert_eq!(m.read(0x2003), 43, "buf[2]");
    assert_eq!(m.read(0x2005), 12, "y comes after the 4 words of buf");
    let program = assemble("var x\nvar buf[4]\n: main halt ;").unwrap();
    assert_eq!(program.ram_used, 5);
    assert_eq!(program.symbol("buf").unwrap().value, 0x2001);
}

#[test]
fn constants() {
    let source = "const ROW = 8\nconst ROWS = 64\nconst LAST = SCREEN + ROW * ROWS - 1\nconst NEG = -5\n\
                  const UP = 0b0001\n: main LAST NEG 10 + UP halt ;";
    assert_eq!(run(source).stack(), [0x41FF, 5, 1]);
    // A program's own constant replaces a built-in one.
    assert_eq!(run("const PAD = 7\n: main PAD halt ;").stack(), [7]);
}

#[test]
fn if_else_then() {
    assert_eq!(stack("1 if 10 then"), [10]);
    assert_eq!(stack("0 if 10 then"), []);
    assert_eq!(stack("1 if 10 else 20 then 0 if 10 else 20 then"), [10, 20]);
    assert_eq!(stack("1 if 0 if 1 else 2 then then"), [2], "nested");
}

#[test]
fn loops() {
    // Sum 1..10 with begin/until.
    assert_eq!(stack("0 10 begin dup >r + r> 1- dup 0= until drop"), [55]);
    // The same with for/next: `i` counts down from 10 to 1.
    assert_eq!(stack("0 10 for i + next"), [55]);
    assert_eq!(stack("0 3 for 4 for 1+ next next"), [12], "nested");
    assert_eq!(stack("1 for 7 next"), [7], "runs once");
    // begin/while/repeat: halve 100 until it's 0.
    assert_eq!(stack("0 100 begin dup while 2/ swap 1+ swap repeat drop"), [7]);
}

#[test]
fn words_calls_and_recursion() {
    let source = "\
        : square ( n -- n*n ) dup 0 swap for over + next nip ;\n\
        : fib ( n -- fib ) dup 2 < if exit then dup 1- fib swap 2 - fib + ;\n\
        : main 7 square 10 fib halt ;";
    assert_eq!(run(source).stack(), [49, 55]);
    // Words can be used before they're defined.
    assert_eq!(run(": main later halt ;\n: later 3 ;").stack(), [3]);
}

#[test]
fn returns_merge_and_tail_calls_jump() {
    let program = assemble(": double dup + ;\n: quad double double ;\n: main 1 quad halt ;").unwrap();
    let double = program.symbol("double").unwrap().value as usize;
    assert_eq!(isa::disasm(program.words[double + 1]), "+ ;", "the return shares the add's instruction");
    let quad = program.symbol("quad").unwrap().value as usize;
    assert_eq!(Insn::decode(program.words[quad + 1]), Insn::Jump(double as u16), "a tail call");
    assert_eq!(quad + 2, program.symbol("main").unwrap().value as usize, "quad is 2 words");
    let mut m = Machine::new(&program.words);
    m.run(100);
    assert_eq!(m.stack(), [4]);
    // A return right after `then` is a jump target, so it can't merge.
    assert_eq!(stack("3 0 if drop then"), [3]);
    // Nothing can reach a return after `halt`, so it's left out.
    assert_eq!(assemble(": main halt ;").unwrap().words.len(), 2);
    // ...and the only way out of `begin again` is an early `exit`.
    assert_eq!(run(": count 0 begin 1+ dup 3 = if exit then again ;\n: main count halt ;").stack(), [3]);
}

#[test]
fn tables_and_the_console() {
    let source = "\
        table greeting \"Hello, Crumb!\\n\" 0 end\n\
        : print ( addr -- ) begin dup @ dup while CONSOLE ! 1+ repeat 2drop ;\n\
        : main greeting print 'o' CONSOLE ! 'k' CONSOLE ! halt ;";
    assert_eq!(run(source).console_text(), "Hello, Crumb!\nok");
    let lookup = run("table squares 0 1 4 9 16 end\n: main squares 3 + @ halt ;");
    assert_eq!(lookup.stack(), [9]);
}

#[test]
fn the_screen() {
    let m = run(": main 0x8001 SCREEN ! 0xFFFF SCREEN 8 + 7 + ! halt ;");
    assert!(m.pixel(0, 0), "bit 15 is the leftmost pixel");
    assert!(!m.pixel(1, 0));
    assert!(m.pixel(15, 0));
    assert!((112..128).all(|x| m.pixel(x, 1)), "row 1, last word");
    assert_eq!(m.screen_text().lines().nth(1).unwrap().matches('#').count(), 16);
}

#[test]
fn ports() {
    let program = assemble(": main PAD @ NUMBER ! 0x00F0 LEDS ! LEDS @ halt ;").unwrap();
    let mut m = Machine::new(&program.words);
    m.pad = 0xFF11;
    m.run(100);
    assert_eq!(m.number, 0x11, "only 8 buttons");
    assert_eq!(m.leds, 0xF0);
    assert_eq!(m.stack(), [0xF0], "LEDS reads back");
    // ROM ignores writes; unused addresses read 0.
    let m = run(": main 99 0 ! 0 @ 0x5000 @ 0x7000 @ 0xFFFF @ halt ;");
    assert_eq!(m.stack(), [Insn::Jump(1).encode(), 0, 0, 0]);
}

#[test]
fn frames_count_ticks() {
    // Wait for two frame changes, then report how many ticks that took.
    let source = "var last\n: wait-frame FRAME @ last @ = if wait-frame exit then FRAME @ last ! ;\n\
                  : main wait-frame wait-frame FRAME @ NUMBER ! halt ;";
    let program = assemble(source).unwrap();
    let mut m = Machine::new(&program.words);
    m.ticks_per_frame = 100;
    m.run(10_000);
    assert!(m.halted);
    assert_eq!(m.number, 2);
    assert!((200..260).contains(&m.ticks), "{} ticks", m.ticks);
    assert_eq!(m.ticks, 2 * m.instructions, "2 ticks per instruction");
}

#[test]
fn frames_keep_coming_after_a_halt() {
    // `begin again` with nothing inside is a jump to itself: a halt.
    let program = assemble(": main begin again ;").unwrap();
    let mut m = Machine::new(&program.words);
    m.ticks_per_frame = 1000;
    m.run_frames(3);
    assert!(m.halted);
    assert_eq!(m.frame(), 3);
    assert!(m.ticks >= 3000 && m.ticks < 3002);
}

#[test]
fn random_steps_every_tick() {
    let m = run(": main RANDOM @ RANDOM @ halt ;");
    // Instruction 0 jumps to main, so the two `@`s are instructions 2 and
    // 4: they read during ticks 5 and 9.
    let lfsr = |ticks: u32| {
        let mut l: u16 = 0xACE1;
        for _ in 0..ticks {
            l = (l >> 1) ^ if l & 1 != 0 { 0xB400 } else { 0 };
        }
        l
    };
    assert_eq!(m.stack(), [lfsr(5), lfsr(9)]);
}

#[test]
fn stacks_wrap_without_crashing() {
    let pushes = " 1".repeat(40);
    let m = run(&format!(": main{pushes} halt ;"));
    assert_eq!(m.t, 1);
}

#[test]
fn listing_shows_names() {
    let source = ": inc 1+ ;\n: main 5 inc halt ;";
    let program = assemble(source).unwrap();
    let listing = program.listing(source);
    assert!(listing.contains("jump main"), "{listing}");
    assert!(listing.contains(": inc"), "{listing}");
    assert!(listing.contains("+ ;"), "{listing}");
    assert!(listing.contains("jump inc") || listing.contains("call inc"), "{listing}");
    let program = assemble("table t 0x0011 end\n: main halt ;").unwrap();
    assert_eq!(program.describe(1), "data 0x0011", "not `jump 0x0011`");
    assert_eq!(program.describe(2), "jump main", "`halt` jumps to itself, at the start of main");
}

#[test]
fn friendly_errors() {
    let e = error(": main drwa halt ;\n: draw ;");
    assert_eq!(e.message, "there's no word named `drwa`");
    assert_eq!(e.label.as_deref(), Some("did you mean `draw`?"));
    assert_eq!(error(": main 1 2 +").message, "`main` is missing its `;`");
    assert_eq!(error(": main 1 if 2 ;").message, "this `if` is missing its `then`");
    assert_eq!(error(": main then ;").message, "`then` without an `if`");
    assert_eq!(error(": main next ;").message, "`next` without a `for`");
    assert_eq!(error(": main ;\n: main ;").message, "`main` is already defined");
    assert_eq!(error(": helper ;").message, "there's no `main` word");
    assert_eq!(error(": main \"hi\" ;").message, "strings only go in a `table`");
    assert_eq!(error(": main 70000 ;").message, "`70000` doesn't fit in 16 bits");
    assert_eq!(error("1 2 +\n: main ;").message, "code has to go inside a definition");
    assert_eq!(error(": dup ;\n: main ;").message, "`dup` is a built-in word, so it can't be redefined");
    assert_eq!(error("const X\n: main ;").message, "`X` needs a value");
    assert_eq!(error("var big[9000]\n: main ;").message, "`big` doesn't fit: RAM holds 8192 words");
    assert_eq!(error(": main 'ab' ;").message, "a character literal holds exactly one character");
    assert_eq!(error(": main \"oops ;").message, "this string is missing its closing quote");
    // Only one error for a stray stretch of code.
    assert_eq!(assemble("1 2 3 4\n: main ;").unwrap_err().len(), 1);
}

#[test]
fn errors_point_at_the_mistake() {
    let source = ": main\n  1 2 frobnicate\n;";
    let e = error(source);
    assert_eq!(&source[e.start..e.end], "frobnicate");
}
