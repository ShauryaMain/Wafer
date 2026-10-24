//! The built-in parts every project gets for free.

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
    /// `a` when `sel` is 0, `b` when it's 1.
    Mux,
    /// Passes `a` when `en` is 1, otherwise lets go of the wire (Z).
    TriBuf,
    /// A flip-flop: takes the value of `d` at each clock tick.
    Dff,
    /// Memory: reads `addr` any time, writes `data` there on a tick when
    /// `write` is 1.
    Ram,
}

/// How wide a built-in's port is compared to the part itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortWidth {
    /// The part's width: a bus `Xor` works on every bit.
    Bus,
    /// Always one bit, like `sel` or `write`.
    Bit,
    /// A `Ram`'s address width.
    Address,
}

impl GateKind {
    pub const ALL: [GateKind; 11] = [
        GateKind::Nand,
        GateKind::And,
        GateKind::Or,
        GateKind::Not,
        GateKind::Xor,
        GateKind::Nor,
        GateKind::Xnor,
        GateKind::Mux,
        GateKind::TriBuf,
        GateKind::Dff,
        GateKind::Ram,
    ];

    pub fn name(self) -> &'static str {
        match self {
            GateKind::Nand => "Nand",
            GateKind::And => "And",
            GateKind::Or => "Or",
            GateKind::Not => "Not",
            GateKind::Xor => "Xor",
            GateKind::Nor => "Nor",
            GateKind::Xnor => "Xnor",
            GateKind::Mux => "Mux",
            GateKind::TriBuf => "TriBuf",
            GateKind::Dff => "Dff",
            GateKind::Ram => "Ram",
        }
    }

    pub fn from_name(name: &str) -> Option<GateKind> {
        GateKind::ALL.into_iter().find(|g| g.name() == name)
    }

    /// Input port names. Every built-in has a single output called `out`.
    pub fn inputs(self) -> &'static [&'static str] {
        match self {
            GateKind::Not => &["a"],
            GateKind::Mux => &["a", "b", "sel"],
            GateKind::TriBuf => &["a", "en"],
            GateKind::Dff => &["d"],
            GateKind::Ram => &["addr", "data", "write"],
            _ => &["a", "b"],
        }
    }

    pub fn port_width(self, port: usize) -> PortWidth {
        match (self, port) {
            (GateKind::Mux, 2) | (GateKind::TriBuf, 1) | (GateKind::Ram, 2) => PortWidth::Bit,
            (GateKind::Ram, 0) => PortWidth::Address,
            _ => PortWidth::Bus,
        }
    }

    /// Parts that remember things between clock ticks.
    pub fn is_sequential(self) -> bool {
        matches!(self, GateKind::Dff | GateKind::Ram)
    }

    /// One line for menus and hints.
    pub fn describe(self) -> &'static str {
        match self {
            GateKind::Nand => "0 only when both inputs are 1",
            GateKind::And => "1 when both inputs are 1",
            GateKind::Or => "1 when either input is 1",
            GateKind::Not => "flips its input",
            GateKind::Xor => "1 when the inputs differ",
            GateKind::Nor => "1 only when both inputs are 0",
            GateKind::Xnor => "1 when the inputs match",
            GateKind::Mux => "a when sel is 0, b when sel is 1",
            GateKind::TriBuf => "a when en is 1, otherwise disconnected (Z)",
            GateKind::Dff => "remembers d at every clock tick",
            GateKind::Ram => "memory: reads addr, writes data on a tick when write is 1",
        }
    }

    /// Evaluates one bit of a combinational part. Unused inputs are ignored;
    /// sequential parts aren't evaluated this way.
    pub fn eval(self, a: Logic, b: Logic, c: Logic) -> Logic {
        // A disconnected input reads as unknown to a gate.
        let solid = |v: Logic| if v == Logic::Z { Logic::X } else { v };
        match self {
            GateKind::Nand => !a.and(b),
            GateKind::And => a.and(b),
            GateKind::Or => a.or(b),
            GateKind::Not => !a,
            GateKind::Xor => a.xor(b),
            GateKind::Nor => !a.or(b),
            GateKind::Xnor => !a.xor(b),
            GateKind::Mux => match c {
                Logic::Zero => solid(a),
                Logic::One => solid(b),
                // Unknown select: still known if both sides agree.
                _ if a == b && a.to_bool().is_some() => a,
                _ => Logic::X,
            },
            GateKind::TriBuf => match b {
                Logic::One => solid(a),
                Logic::Zero => Logic::Z,
                _ => Logic::X,
            },
            GateKind::Dff | GateKind::Ram => Logic::X,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GateKind::*;
    use crate::logic::Logic::*;

    #[test]
    fn mux_and_tribuf() {
        assert_eq!(Mux.eval(Zero, One, Zero), Zero);
        assert_eq!(Mux.eval(Zero, One, One), One);
        assert_eq!(Mux.eval(One, One, X), One, "both sides agree");
        assert_eq!(Mux.eval(Zero, One, X), X);
        assert_eq!(TriBuf.eval(One, One, X), One);
        assert_eq!(TriBuf.eval(One, Zero, X), Z);
        assert_eq!(TriBuf.eval(One, X, X), X);
    }
}
