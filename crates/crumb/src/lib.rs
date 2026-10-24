//! Crumb, the 16-bit stack CPU of our game console. `docs/crumb.md` is the
//! spec; this crate is its executable version:
//!
//! - [`isa`]: the instruction encoding, shared by everything else here.
//! - [`asm`]: the Forth-style assembler that turns `.asm` source into ROM words.
//! - [`cpu`]: the reference Crumb, a fast model of the whole console that the
//!   gate-level CPU gets checked against.

pub mod asm;
pub mod cpu;
pub mod isa;

pub use asm::{Program, assemble};
pub use cpu::Machine;
