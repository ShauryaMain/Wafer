//! The `wafer` command: check, test, simulate and draw chips from the
//! terminal. Everything it does goes through `wafer-core`, the same engine
//! the desktop app will use.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use wafer_core::diag::{display_path, suggest};
use wafer_core::layout_file::LayoutFile;
use wafer_core::logic::{Logic, bits_from_u64, format_bits};
use wafer_core::testing::{self, MAX_STEPS};
use wafer_core::{Design, Diagnostic, Netlist, Project, Simulator, schematic, svg};

#[derive(Parser)]
#[command(name = "wafer", version, about = "Design, simulate and test digital chips")]
struct Cli {
    /// Folder of .chip files (or a single .chip file) to load
    #[arg(short, long, global = true, default_value = ".")]
    project: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check every chip for mistakes
    Check,
    /// Run test blocks, and compare home-made gates with the built-in ones
    Test {
        /// Only test these chips
        chips: Vec<String>,
    },
    /// Simulate a chip and print every signal inside it
    Sim {
        chip: String,
        /// Set inputs, like `--set a=1,b=0` or `--set bus=0x2A` (inputs start at 0)
        #[arg(long, value_name = "NAME=VALUE")]
        set: Vec<String>,
        /// Also list signals inside the chips it uses
        #[arg(long)]
        all: bool,
    },
    /// Draw a chip's schematic, colored by its simulated values
    Render {
        chip: String,
        /// Set inputs, like `--set a=1,b=0`
        #[arg(long, value_name = "NAME=VALUE")]
        set: Vec<String>,
        /// Where to save it: .png or .svg (default: renders/<chip>.png)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

struct Ui {
    color: bool,
}

impl Ui {
    fn paint(&self, code: &str, s: &str) -> String {
        if self.color { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
    }
    fn pass(&self, s: &str) -> String {
        self.paint("32", s)
    }
    fn fail(&self, s: &str) -> String {
        self.paint("31", s)
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ui = Ui { color: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() };
    let project = match Project::load(&cli.project) {
        Ok(project) => project,
        Err(e) => {
            eprintln!("{} can't read {}: {e}", ui.fail("error:"), cli.project.display());
            return ExitCode::FAILURE;
        }
    };
    if project.files.is_empty() {
        eprintln!("{} no .chip files in {}", ui.fail("error:"), cli.project.display());
        return ExitCode::FAILURE;
    }
    let design = Design::check(&project);
    let diagnostics: Vec<&Diagnostic> = project.diagnostics.iter().chain(&design.diagnostics).collect();
    for d in &diagnostics {
        eprintln!("{}", d.render(&project.sources, ui.color));
    }
    let errors = diagnostics.iter().filter(|d| d.is_error()).count();
    let warnings = diagnostics.len() - errors;

    match cli.command {
        Command::Check => check(&design, errors, warnings, &ui),
        Command::Test { chips } => test(&project, &design, &chips, errors, &ui),
        Command::Sim { chip, set, all } => sim(&design, &chip, &set, all, &ui),
        Command::Render { chip, set, output } => render(&project, &design, &chip, &set, output, &ui),
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("{n} {word}") } else { format!("{n} {word}s") }
}

fn check(design: &Design, errors: usize, warnings: usize, ui: &Ui) -> ExitCode {
    let summary = format!("{} and {}", plural(design.chips.len(), "chip"), plural(design.tests.len(), "test"));
    if errors > 0 {
        println!("{} {summary}: {}, {}", ui.fail("✗"), plural(errors, "error"), plural(warnings, "warning"));
        ExitCode::FAILURE
    } else {
        let extra = if warnings > 0 { format!(", {}", plural(warnings, "warning")) } else { String::new() };
        println!("{} {summary}: no errors{extra}", ui.pass("✓"));
        ExitCode::SUCCESS
    }
}

fn test(project: &Project, design: &Design, only: &[String], errors: usize, ui: &Ui) -> ExitCode {
    for name in only {
        if !design.chips.contains_key(name) {
            let hint = suggest(name, design.chips.keys().map(String::as_str))
                .map(|s| format!(" (did you mean `{s}`?)"))
                .unwrap_or_default();
            eprintln!("{} there's no chip named `{name}`{hint}", ui.fail("error:"));
            return ExitCode::FAILURE;
        }
    }
    let mut names: Vec<&String> = design.chips.keys().filter(|n| only.is_empty() || only.contains(n)).collect();
    names.sort_by_key(|n| (design.depth(n), n.to_string()));
    let width = names.iter().map(|n| n.len()).max().unwrap_or(0);

    let (mut rows_passed, mut rows_failed, mut chips_failed, mut untested) = (0, 0, 0, 0);
    for name in names {
        let padded = format!("{name:<width$}");
        if !design.is_buildable(name) {
            println!("{} {padded}  {}", ui.fail("✗"), ui.fail("has errors (see above)"));
            chips_failed += 1;
            continue;
        }
        let outcomes: Vec<_> =
            design.tests.iter().filter(|t| &t.chip == name).map(|t| testing::run_test(design, t)).collect();
        let builtin = testing::check_against_builtin(design, name);
        if outcomes.is_empty() && builtin.is_none() {
            println!("{} {}", ui.dim("·"), ui.dim(&format!("{padded}  no tests yet")));
            untested += 1;
            continue;
        }

        let mut summary = Vec::new();
        let mut details = Vec::new();
        if !outcomes.is_empty() {
            let total: usize = outcomes.iter().map(|o| o.rows.len()).sum();
            let passed: usize = outcomes.iter().map(|o| o.passed_rows()).sum();
            rows_passed += passed;
            rows_failed += total - passed;
            summary.push(format!("{passed}/{total} rows"));
        }
        for outcome in &outcomes {
            if let Some(error) = &outcome.error {
                details.push(format!("can't build it: {error}"));
            }
            for row in outcome.rows.iter().filter(|r| !r.passed()) {
                let at = project.sources.location(row.span);
                let inputs: Vec<String> = row.inputs.iter().map(|(n, v)| format!("{n}={v}")).collect();
                let problem = if row.unsettled {
                    format!("still changing after {MAX_STEPS} steps (is something oscillating?)")
                } else {
                    row.failures
                        .iter()
                        .map(|f| format!("expected {}={}, got {}", f.output, f.expected, f.actual))
                        .collect::<Vec<_>>()
                        .join("; ")
                };
                details.push(format!("{}  {} → {problem}", ui.dim(&at), inputs.join(" ")));
            }
        }
        if let Some(check) = &builtin {
            let gate = check.gate.name();
            if let Some(error) = &check.error {
                details.push(format!("can't build it: {error}"));
            } else if check.mismatches.is_empty() {
                summary.push(format!("matches built-in {gate}"));
            } else {
                summary.push(format!(
                    "differs from built-in {gate} in {} of {} cases",
                    check.mismatches.len(),
                    check.cases
                ));
                for m in &check.mismatches {
                    let inputs: Vec<String> = m.inputs.iter().map(Logic::to_string).collect();
                    details.push(format!(
                        "inputs {} → built-in gives {}, yours gives {}",
                        inputs.join(" "),
                        m.expected,
                        m.actual
                    ));
                }
            }
        }

        let ok = outcomes.iter().all(|o| o.passed()) && builtin.as_ref().is_none_or(|c| c.passed());
        if !ok {
            chips_failed += 1;
        }
        let mark = if ok { ui.pass("✓") } else { ui.fail("✗") };
        println!("{mark} {padded}  {}", summary.join(" · "));
        for line in details {
            println!("    {line}");
        }
    }

    let mut tail = vec![format!("{} passed", plural(rows_passed, "row"))];
    if rows_failed > 0 {
        tail.push(ui.fail(&format!("{} failed", plural(rows_failed, "row"))));
    }
    if untested > 0 {
        tail.push(format!("{} without tests", plural(untested, "chip")));
    }
    if errors > 0 {
        tail.push(ui.fail(&format!("{} above", plural(errors, "error"))));
    }
    println!();
    println!("{}", tail.join(" · "));
    if chips_failed > 0 || errors > 0 { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

/// A chip with its `--set` values applied, after it has settled.
struct Run {
    sim: Simulator,
    /// Gate delays until it settled, or `Err` if it was still changing.
    settled: Result<u64, u64>,
    /// The inputs that were set, like `a=1`.
    shown: Vec<String>,
}

fn simulate(design: &Design, chip: &str, sets: &[String]) -> Result<Run, String> {
    let netlist = Netlist::build(design, chip).map_err(|e| e.to_string())?;
    let mut sim = Simulator::new(netlist);
    let mut shown = Vec::new();
    for assignment in sets.iter().flat_map(|s| s.split(',')).map(str::trim).filter(|s| !s.is_empty()) {
        let Some((name, value)) = assignment.split_once('=') else {
            return Err(format!("`{assignment}` should look like name=value"));
        };
        let (name, value) = (name.trim(), value.trim());
        let inputs = &sim.netlist().inputs;
        let Some(index) = inputs.iter().position(|s| s.path == name) else {
            let names: Vec<&str> = inputs.iter().map(|s| s.path.as_str()).collect();
            let hint =
                suggest(name, names.iter().copied()).map(|s| format!(" (did you mean `{s}`?)")).unwrap_or_default();
            return Err(format!("`{chip}` has no input `{name}`{hint}; its inputs are {}", names.join(", ")));
        };
        let width = inputs[index].nets.len();
        let bits = parse_value(value, width)?;
        shown.push(format!("{name}={}", format_bits(&bits)));
        sim.set_input(index, &bits);
    }
    let settled = sim.settle(MAX_STEPS).map_err(|u| u.steps);
    Ok(Run { sim, settled, shown })
}

fn parse_value(text: &str, width: usize) -> Result<Vec<Logic>, String> {
    match text {
        "x" | "X" => return Ok(vec![Logic::X; width]),
        "z" | "Z" => return Ok(vec![Logic::Z; width]),
        _ => {}
    }
    let lower = text.to_ascii_lowercase().replace('_', "");
    let parsed = if let Some(hex) = lower.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
    } else if let Some(binary) = lower.strip_prefix("0b") {
        u64::from_str_radix(binary, 2)
    } else {
        lower.parse()
    };
    let value = parsed.map_err(|_| format!("`{text}` isn't a number, X or Z"))?;
    if width < 64 && value >> width != 0 {
        return Err(format!("{value} doesn't fit in {width} bit{}", if width == 1 { "" } else { "s" }));
    }
    Ok(bits_from_u64(value, width))
}

fn settle_text(settled: Result<u64, u64>) -> String {
    match settled {
        Ok(0) => "nothing changed".into(),
        Ok(1) => "settled after 1 gate delay".into(),
        Ok(steps) => format!("settled after {steps} gate delays"),
        Err(steps) => format!("still changing after {steps} gate delays"),
    }
}

fn sim(design: &Design, chip: &str, sets: &[String], all: bool, ui: &Ui) -> ExitCode {
    let Run { sim, settled, .. } = match simulate(design, chip, sets) {
        Ok(run) => run,
        Err(message) => {
            eprintln!("{} {message}", ui.fail("error:"));
            return ExitCode::FAILURE;
        }
    };
    let status = settle_text(settled);
    println!("{chip}: {}", if settled.is_ok() { ui.pass(&status) } else { ui.fail(&status) });
    let netlist = sim.netlist();
    let is_port = |path: &str| netlist.inputs.iter().chain(&netlist.outputs).any(|p| p.path == path);
    let inside: Vec<_> = netlist.signals.iter().filter(|s| !is_port(&s.path) && (all || s.depth == 0)).collect();
    let width = netlist
        .inputs
        .iter()
        .chain(&netlist.outputs)
        .chain(inside.iter().copied())
        .map(|s| s.path.len())
        .max()
        .unwrap_or(0);
    let line = |path: &str, nets: &[u32]| format!("  {path:<width$}  {}", format_bits(&sim.values(nets)));
    println!("\n{}", ui.dim("inputs"));
    for s in &netlist.inputs {
        println!("{}", line(&s.path, &s.nets));
    }
    println!("\n{}", ui.dim("outputs"));
    for s in &netlist.outputs {
        println!("{}", line(&s.path, &s.nets));
    }
    if !inside.is_empty() {
        println!("\n{}", ui.dim("inside"));
        for s in inside {
            println!("{}", line(&s.path, &s.nets));
        }
    }
    if !all && netlist.signals.iter().any(|s| s.depth > 0) {
        println!("\n{}", ui.dim("(add --all to see signals inside the chips it uses)"));
    }
    if settled.is_ok() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn render(
    project: &Project,
    design: &Design,
    chip: &str,
    sets: &[String],
    output: Option<PathBuf>,
    ui: &Ui,
) -> ExitCode {
    // Use the positions saved by the app, if there are any.
    let saved = design.chips.get(chip).and_then(|info| {
        let path = LayoutFile::path_for(&project.sources.get(info.span.file).path);
        LayoutFile::load(&path).ok()?.positions(chip)
    });
    let layout = match saved {
        Some(positions) => schematic::arrange(design, &project.sources, chip, &positions).map(|mut s| {
            s.fit();
            s
        }),
        None => schematic::layout(design, &project.sources, chip),
    };
    let Some(layout) = layout else {
        let hint = suggest(chip, design.chips.keys().map(String::as_str))
            .map(|s| format!(" (did you mean `{s}`?)"))
            .unwrap_or_default();
        eprintln!("{} there's no chip named `{chip}`{hint}", ui.fail("error:"));
        return ExitCode::FAILURE;
    };
    let (sim, subtitle) = if design.is_buildable(chip) {
        match simulate(design, chip, sets) {
            Ok(Run { sim, settled, shown }) => {
                let inputs = if shown.is_empty() { "all inputs 0".to_string() } else { shown.join(" ") };
                (Some(sim), format!("{inputs} · {}", settle_text(settled)))
            }
            Err(message) => {
                eprintln!("{} {message}", ui.fail("error:"));
                return ExitCode::FAILURE;
            }
        }
    } else {
        (None, "has errors, so this shows the layout without values".to_string())
    };
    let values = |path: &str| sim.as_ref().and_then(|s| s.read(path));
    let svg_text = svg::render(&layout, &svg::TERMINAL_GREEN, &values, &subtitle);

    let output = output.unwrap_or_else(|| PathBuf::from("renders").join(format!("{chip}.png")));
    if let Some(dir) = output.parent().filter(|d| !d.as_os_str().is_empty())
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        eprintln!("{} can't create {}: {e}", ui.fail("error:"), dir.display());
        return ExitCode::FAILURE;
    }
    let saved = if output.extension().is_some_and(|e| e == "svg") {
        std::fs::write(&output, &svg_text).map_err(|e| e.to_string())
    } else {
        write_png(&svg_text, &output, 2.0)
    };
    match saved {
        Ok(()) => {
            println!("{} saved {}", ui.pass("✓"), display_path(&output));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{} can't save {}: {e}", ui.fail("error:"), output.display());
            ExitCode::FAILURE
        }
    }
}

/// Rasterizes the SVG at `scale` times its size (2× keeps text crisp).
fn write_png(svg_text: &str, path: &Path, scale: f32) -> Result<(), String> {
    use resvg::{tiny_skia, usvg};
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(svg_text, &options).map_err(|e| e.to_string())?;
    let size = tree.size().to_int_size().scale_by(scale).ok_or("the image is too big")?;
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height()).ok_or("the image is too big")?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.save_png(path).map_err(|e| e.to_string())
}
