//! Four-valued logic: 0, 1, X (unknown) and Z (disconnected).

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Logic {
    Zero,
    One,
    /// Unknown: an uninitialized gate, or two drivers fighting.
    #[default]
    X,
    /// Disconnected: nothing is driving the wire.
    Z,
}

impl Logic {
    pub fn from_bool(b: bool) -> Logic {
        if b { Logic::One } else { Logic::Zero }
    }

    /// The value as a bool, or `None` for X and Z.
    pub fn to_bool(self) -> Option<bool> {
        match self {
            Logic::Zero => Some(false),
            Logic::One => Some(true),
            Logic::X | Logic::Z => None,
        }
    }

    /// A 0 on either side decides the result even when the other side is unknown.
    pub fn and(self, other: Logic) -> Logic {
        match (self, other) {
            (Logic::Zero, _) | (_, Logic::Zero) => Logic::Zero,
            (Logic::One, Logic::One) => Logic::One,
            _ => Logic::X,
        }
    }

    /// A 1 on either side decides the result even when the other side is unknown.
    pub fn or(self, other: Logic) -> Logic {
        match (self, other) {
            (Logic::One, _) | (_, Logic::One) => Logic::One,
            (Logic::Zero, Logic::Zero) => Logic::Zero,
            _ => Logic::X,
        }
    }

    pub fn xor(self, other: Logic) -> Logic {
        match (self.to_bool(), other.to_bool()) {
            (Some(a), Some(b)) => Logic::from_bool(a != b),
            _ => Logic::X,
        }
    }

    pub fn to_char(self) -> char {
        match self {
            Logic::Zero => '0',
            Logic::One => '1',
            Logic::X => 'x',
            Logic::Z => 'z',
        }
    }
}

impl std::ops::Not for Logic {
    type Output = Logic;

    fn not(self) -> Logic {
        match self {
            Logic::Zero => Logic::One,
            Logic::One => Logic::Zero,
            Logic::X | Logic::Z => Logic::X,
        }
    }
}

impl fmt::Display for Logic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Logic::Zero => "0",
            Logic::One => "1",
            Logic::X => "X",
            Logic::Z => "Z",
        };
        f.write_str(s)
    }
}

/// The bits of `value`, lowest bit first.
pub fn bits_from_u64(value: u64, width: usize) -> Vec<Logic> {
    (0..width).map(|i| Logic::from_bool(i < 64 && (value >> i) & 1 == 1)).collect()
}

/// The number a bus holds, if every bit is 0 or 1 and it fits in 64 bits.
pub fn bits_to_u64(bits: &[Logic]) -> Option<u64> {
    if bits.len() > 64 {
        return None;
    }
    let mut value = 0u64;
    for (i, bit) in bits.iter().enumerate() {
        if bit.to_bool()? {
            value |= 1 << i;
        }
    }
    Some(value)
}

/// The fewest bits that can hold `value` (at least 1).
pub fn min_width(value: u64) -> usize {
    (64 - value.leading_zeros()).max(1) as usize
}

/// Formats a value the way Wafer shows it: `1`, `X`, `0x2A`, or `0b10x1`
/// when only some bits of a bus are unknown.
pub fn format_bits(bits: &[Logic]) -> String {
    match bits {
        [] => String::new(),
        [bit] => bit.to_string(),
        _ => {
            if bits.iter().all(|&b| b == Logic::X) {
                return "X".into();
            }
            if bits.iter().all(|&b| b == Logic::Z) {
                return "Z".into();
            }
            if let Some(value) = bits_to_u64(bits) {
                let digits = bits.len().div_ceil(4);
                return format!("0x{value:0digits$X}");
            }
            let binary: String = bits.iter().rev().map(|b| b.to_char()).collect();
            format!("0b{binary}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Logic::*;
    use super::*;

    #[test]
    fn known_values_decide_through_unknowns() {
        assert_eq!(Zero.and(X), Zero);
        assert_eq!(One.and(X), X);
        assert_eq!(One.or(Z), One);
        assert_eq!(Zero.or(Z), X);
        assert_eq!(One.xor(X), X);
        assert_eq!(!Z, X);
    }

    #[test]
    fn formats_buses() {
        assert_eq!(format_bits(&[One]), "1");
        assert_eq!(format_bits(&bits_from_u64(0x2A, 8)), "0x2A");
        assert_eq!(format_bits(&bits_from_u64(5, 4)), "0x5");
        assert_eq!(format_bits(&[One, X, Zero, One]), "0b10x1");
        assert_eq!(format_bits(&[X, X]), "X");
    }
}
