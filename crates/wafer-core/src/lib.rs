//! Wafer's engine. Everything here is UI-free so the desktop app and the
//! `wafer` command share exactly the same behavior.
//!
//! The pipeline:
//! 1. [`Project::load`] reads and parses every `.chip` file in a folder, plus
//!    the programs (`.asm`, `.hex`) its `Rom`s load.
//! 2. [`Design::check`] resolves names and bus widths and reports mistakes.
//! 3. [`Netlist::build`] flattens one chip into individual bit-level gates.
//! 4. [`Simulator`] runs that netlist with one time step per gate.
//! 5. [`testing`] runs the `test` blocks, and [`schematic`] lays a chip out
//!    for drawing.

// The layout and checker index several parallel arrays at once, where an
// index loop reads more clearly than zipped iterators.
#![allow(clippy::needless_range_loop)]

pub mod ast;
pub mod check;
pub mod complete;
pub mod diag;
pub mod edit;
pub mod format;
pub mod gate;
pub mod layout_file;
pub mod lexer;
pub mod lockstep;
pub mod logic;
pub mod netlist;
pub mod parser;
pub mod program;
pub mod project;
pub mod schematic;
pub mod sim;
pub mod svg;
pub mod testing;
pub mod trace;
pub mod turbo;

pub use check::Design;
pub use diag::{Diagnostic, Severity, SourceMap, Span};
pub use gate::GateKind;
pub use logic::Logic;
pub use netlist::Netlist;
pub use project::Project;
pub use sim::Simulator;
