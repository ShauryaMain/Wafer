//! `wafer asm` and `wafer run`: Crumb programs from the terminal.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crumb::Machine;
use crumb::isa::{BUTTONS, RAM_WORDS, ROM_WORDS, SCREEN_HEIGHT, SCREEN_WIDTH, disasm_with};
use wafer_core::SourceMap;
use wafer_core::diag::display_path;
use wafer_core::program::{self, RomProgram};

use super::Ui;

/// Instructions `wafer run` allows when nothing says when to stop.
const DEFAULT_STEPS: u64 = 10_000_000;

pub struct RunOptions {
    pub steps: Option<u64>,
    pub frames: Option<u64>,
    pub pad: Option<String>,
    pub screen: bool,
    pub png: Option<PathBuf>,
}

/// Reads and assembles (or parses) a program, printing any mistakes.
fn load(file: &Path, ui: &Ui) -> Result<(RomProgram, String), ExitCode> {
    let text = std::fs::read_to_string(file).map_err(|e| {
        eprintln!("{} can't read {}: {e}", ui.fail("error:"), file.display());
        ExitCode::FAILURE
    })?;
    if !program::is_program(file) {
        eprintln!("{} {} isn't a program: use a .asm or .hex file", ui.fail("error:"), display_path(file));
        return Err(ExitCode::FAILURE);
    }
    let mut sources = SourceMap::default();
    let id = sources.add(file.to_path_buf(), text.clone());
    match program::load(&sources, id) {
        Ok(loaded) if loaded.words.len() > ROM_WORDS => {
            eprintln!(
                "{} {} is {} words, but the ROM holds {ROM_WORDS}",
                ui.fail("error:"),
                display_path(file),
                loaded.words.len()
            );
            Err(ExitCode::FAILURE)
        }
        Ok(loaded) => Ok((loaded, text)),
        Err(errors) => {
            for e in &errors {
                eprintln!("{}", e.render(&sources, ui.color));
            }
            let n = errors.len();
            eprintln!("{} {n} mistake{} in {}", ui.fail("failed:"), if n == 1 { "" } else { "s" }, display_path(file));
            Err(ExitCode::FAILURE)
        }
    }
}

pub fn asm(file: &Path, list: bool, output: Option<&Path>, ui: &Ui) -> ExitCode {
    let (loaded, source) = match load(file, ui) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let Some(program) = &loaded.assembled else {
        eprintln!("{} {} is already machine code (.hex)", ui.fail("error:"), display_path(file));
        return ExitCode::FAILURE;
    };
    if list {
        print!("{}", program.listing(&source));
    }
    if let Some(output) = output {
        let mut hex = format!("// {}, assembled by `wafer asm`\n", display_path(file));
        for (addr, &word) in program.words.iter().enumerate() {
            let text = disasm_with(word, |a| program.name_at(a).map(str::to_string));
            let _ = writeln!(hex, "{word:04x}  // {addr:04x}  {text}");
        }
        if let Err(e) = std::fs::write(output, hex) {
            eprintln!("{} can't write {}: {e}", ui.fail("error:"), output.display());
            return ExitCode::FAILURE;
        }
        println!("{} {}", ui.dim("wrote"), display_path(output));
    }
    let percent = |used: usize, of: usize| used as f64 * 100.0 / of as f64;
    println!(
        "{} {}: {} words of ROM ({:.1}%), {} of RAM ({:.1}%)",
        ui.pass("assembled"),
        display_path(file),
        program.words.len(),
        percent(program.words.len(), ROM_WORDS),
        program.ram_used,
        percent(program.ram_used, RAM_WORDS),
    );
    ExitCode::SUCCESS
}

/// `up,a` or `0x11`.
fn parse_pad(text: &str) -> Result<u16, String> {
    let number = |s: &str| {
        let lower = s.to_ascii_lowercase();
        if let Some(hex) = lower.strip_prefix("0x") {
            u16::from_str_radix(hex, 16).ok()
        } else if let Some(bin) = lower.strip_prefix("0b") {
            u16::from_str_radix(bin, 2).ok()
        } else {
            lower.parse().ok()
        }
    };
    if let Some(value) = number(text) {
        return Ok(value);
    }
    let mut pad = 0;
    for name in text.split(',').map(str::trim) {
        match BUTTONS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            Some((_, bit)) => pad |= bit,
            None => {
                let names: Vec<&str> = BUTTONS.iter().map(|(n, _)| *n).collect();
                return Err(format!("there's no button `{name}`; the buttons are {}", names.join(", ")));
            }
        }
    }
    Ok(pad)
}

pub fn run(file: &Path, options: &RunOptions, ui: &Ui) -> ExitCode {
    let (loaded, _) = match load(file, ui) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let words: Vec<u16> = loaded.words.iter().map(|&w| w as u16).collect();
    let mut m = Machine::new(&words);
    if let Some(pad) = &options.pad {
        match parse_pad(pad) {
            Ok(pad) => m.pad = pad,
            Err(message) => {
                eprintln!("{} {message}", ui.fail("error:"));
                return ExitCode::FAILURE;
            }
        }
    }
    match (options.frames, options.steps) {
        (Some(frames), _) => m.run_frames(frames),
        (None, steps) => {
            m.run(steps.unwrap_or(DEFAULT_STEPS));
        }
    }

    let name_of = |addr: u16| loaded.assembled.as_ref().and_then(|p| p.name_at(addr)).map(str::to_string);
    let place = name_of(m.pc).map_or_else(|| format!("{:#06x}", m.pc), |n| format!("{:#06x} ({n})", m.pc));
    let summary = format!("{} instructions, {} ticks, frame {}", m.instructions, m.ticks, m.frame());
    if m.halted {
        println!("{} after {summary}", ui.pass("halted"));
    } else if options.frames.is_some() || options.steps.is_some() {
        println!("{} after {summary}, at {place}", ui.pass("stopped"));
    } else {
        println!(
            "{} after {summary}, at {place}",
            ui.fail(&format!("still running after {DEFAULT_STEPS} instructions:")),
        );
        println!("{}", ui.dim("  (use --steps or --frames to say how long to run)"));
    }
    if !m.console.is_empty() {
        println!("console:");
        for line in m.console_text().lines() {
            println!("  {line}");
        }
    }
    let stack: Vec<String> = m.stack().iter().map(|v| format!("{v:#x}")).collect();
    println!("number   {:#06x} ({})", m.number, m.number);
    println!("leds     {:016b}", m.leds);
    println!("stack    [{}]", stack.join(" "));
    if options.screen {
        print!("{}", screen_text(&m));
    }
    if let Some(path) = &options.png {
        if let Err(e) = save_png(&m, path) {
            eprintln!("{} can't save {}: {e}", ui.fail("error:"), path.display());
            return ExitCode::FAILURE;
        }
        println!("{} {}", ui.dim("saved"), display_path(path));
    }
    ExitCode::SUCCESS
}

/// Two pixel rows per line of text, with half-block characters.
fn screen_text(m: &Machine) -> String {
    let mut out = format!("┌{}┐\n", "─".repeat(SCREEN_WIDTH));
    for y in (0..SCREEN_HEIGHT).step_by(2) {
        out.push('│');
        for x in 0..SCREEN_WIDTH {
            out.push(match (m.pixel(x, y), m.pixel(x, y + 1)) {
                (false, false) => ' ',
                (true, false) => '▀',
                (false, true) => '▄',
                (true, true) => '█',
            });
        }
        out.push_str("│\n");
    }
    out.push_str(&format!("└{}┘\n", "─".repeat(SCREEN_WIDTH)));
    out
}

/// The screen at 4× size, in Wafer's colors.
fn save_png(m: &Machine, path: &Path) -> Result<(), String> {
    use resvg::tiny_skia::Pixmap;
    const SCALE: usize = 4;
    const LIT: [u8; 3] = [0x4d, 0xff, 0x88];
    const DARK: [u8; 3] = [0x0f, 0x14, 0x11];
    let (w, h) = (SCREEN_WIDTH * SCALE, SCREEN_HEIGHT * SCALE);
    let mut pixmap = Pixmap::new(w as u32, h as u32).ok_or("the image is too big")?;
    for (i, px) in pixmap.data_mut().as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i % w / SCALE, i / w / SCALE);
        let [r, g, b] = if m.pixel(x, y) { LIT } else { DARK };
        px.copy_from_slice(&[r, g, b, 255]);
    }
    pixmap.save_png(path).map_err(|e| e.to_string())
}
