//! The built-in gates every project gets for free.

use crate::logic::Logic;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GateKind {
    Nand,
    And,
    Or,
    Not,
    Xor,
    Nor,
    Xnor,
}

impl GateKind {
    pub const ALL: [GateKind; 7] =
        [GateKind::Nand, GateKind::And, GateKind::Or, GateKind::Not, GateKind::Xor, GateKind::Nor, GateKind::Xnor];

    pub fn name(self) -> &'static str {
        match self {
            GateKind::Nand => "Nand",
            GateKind::And => "And",
            GateKind::Or => "Or",
            GateKind::Not => "Not",
            GateKind::Xor => "Xor",
            GateKind::Nor => "Nor",
            GateKind::Xnor => "Xnor",
        }
    }

    pub fn from_name(name: &str) -> Option<GateKind> {
        GateKind::ALL.into_iter().find(|g| g.name() == name)
    }

    /// Input port names. Every gate has a single output called `out`.
    pub fn inputs(self) -> &'static [&'static str] {
        match self {
            GateKind::Not => &["a"],
            _ => &["a", "b"],
        }
    }

    /// Evaluates one bit. `Not` ignores `b`.
    pub fn eval(self, a: Logic, b: Logic) -> Logic {
        match self {
            GateKind::Nand => !a.and(b),
            GateKind::And => a.and(b),
            GateKind::Or => a.or(b),
            GateKind::Not => !a,
            GateKind::Xor => a.xor(b),
            GateKind::Nor => !a.or(b),
            GateKind::Xnor => !a.xor(b),
        }
    }
}
