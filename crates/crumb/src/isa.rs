//! Crumb's memory map and instruction encoding.

/// Program ROM: `0x0000..0x2000`.
pub const ROM_WORDS: usize = 0x2000;
pub const RAM_START: u16 = 0x2000;
pub const RAM_WORDS: usize = 0x2000;
pub const SCREEN: u16 = 0x4000;
pub const SCREEN_WIDTH: usize = 128;
pub const SCREEN_HEIGHT: usize = 64;
/// Words per screen row: 16 pixels each.
pub const SCREEN_ROW_WORDS: usize = SCREEN_WIDTH / 16;
pub const SCREEN_WORDS: usize = SCREEN_ROW_WORDS * SCREEN_HEIGHT;

/// I/O ports.
pub const PAD: u16 = 0x6000;
pub const FRAME: u16 = 0x6001;
pub const RANDOM: u16 = 0x6002;
pub const LEDS: u16 = 0x6003;
pub const SWITCHES: u16 = 0x6004;
pub const NUMBER: u16 = 0x6005;
pub const CONSOLE: u16 = 0x6006;

/// Gamepad bits, as read from [`PAD`].
pub const BUTTONS: [(&str, u16); 8] =
    [("up", 1), ("down", 2), ("left", 4), ("right", 8), ("a", 16), ("b", 32), ("start", 64), ("select", 128)];

pub const STACK_DEPTH: usize = 32;
pub const TICKS_PER_INSTRUCTION: u64 = 2;
/// 2048 instructions per frame. Chosen after measuring the gate-level Crumb
/// at about 100,000 ticks a second, so games are watchable even in gates.
pub const TICKS_PER_FRAME: u64 = 4096;

/// `PC` and jump targets are 13 bits: the whole ROM.
pub const ADDRESS_MASK: u16 = 0x1FFF;
/// The biggest number a literal can push directly.
pub const LITERAL_MAX: u16 = 0x7FFF;

/// What an ALU instruction computes for the new `T`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    T,
    N,
    Add,
    Sub,
    And,
    Or,
    Xor,
    Not,
    Eq,
    Lt,
    ULt,
    Shl,
    Shr,
    Load,
    R,
    Dec,
}

impl Op {
    pub const ALL: [Op; 16] = [
        Op::T,
        Op::N,
        Op::Add,
        Op::Sub,
        Op::And,
        Op::Or,
        Op::Xor,
        Op::Not,
        Op::Eq,
        Op::Lt,
        Op::ULt,
        Op::Shl,
        Op::Shr,
        Op::Load,
        Op::R,
        Op::Dec,
    ];

    pub fn code(self) -> u16 {
        Op::ALL.iter().position(|&op| op == self).expect("every op is listed") as u16
    }

    pub fn from_code(code: u16) -> Op {
        Op::ALL[(code & 15) as usize]
    }

    /// How the spec writes it, like `T+N`.
    pub fn symbol(self) -> &'static str {
        match self {
            Op::T => "T",
            Op::N => "N",
            Op::Add => "T+N",
            Op::Sub => "N-T",
            Op::And => "T&N",
            Op::Or => "T|N",
            Op::Xor => "T^N",
            Op::Not => "~T",
            Op::Eq => "N==T",
            Op::Lt => "N<T",
            Op::ULt => "Nu<T",
            Op::Shl => "N<<T",
            Op::Shr => "N>>T",
            Op::Load => "[T]",
            Op::R => "R",
            Op::Dec => "T-1",
        }
    }

    /// The result, given `T`, `N`, `R` and the memory word at `T`.
    pub fn eval(self, t: u16, n: u16, r: u16, mem: u16) -> u16 {
        let flag = |b: bool| if b { 0xFFFF } else { 0 };
        match self {
            Op::T => t,
            Op::N => n,
            Op::Add => t.wrapping_add(n),
            Op::Sub => n.wrapping_sub(t),
            Op::And => t & n,
            Op::Or => t | n,
            Op::Xor => t ^ n,
            Op::Not => !t,
            Op::Eq => flag(n == t),
            Op::Lt => flag((n as i16) < (t as i16)),
            Op::ULt => flag(n < t),
            Op::Shl => n << (t & 15),
            Op::Shr => n >> (t & 15),
            Op::Load => mem,
            Op::R => r,
            Op::Dec => t.wrapping_sub(1),
        }
    }
}

/// The fields of an ALU instruction (`011` + 13 bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alu {
    pub op: Op,
    /// `R→PC`: return.
    pub ret: bool,
    /// `T→N`: the old `T` goes into the new `N` slot.
    pub t_to_n: bool,
    /// `T→R`: the old `T` goes into the new `R` slot.
    pub t_to_r: bool,
    /// `N→[T]`: store `N` at address `T`.
    pub store: bool,
    /// Return stack move: -2 to +1.
    pub rd: i8,
    /// Data stack move: -2 to +1.
    pub dd: i8,
}

impl Alu {
    pub const fn new(op: Op) -> Alu {
        Alu { op, ret: false, t_to_n: false, t_to_r: false, store: false, rd: 0, dd: 0 }
    }

    pub const fn d(self, dd: i8) -> Alu {
        Alu { dd, ..self }
    }

    pub const fn r(self, rd: i8) -> Alu {
        Alu { rd, ..self }
    }

    pub const fn t_to_n(self) -> Alu {
        Alu { t_to_n: true, ..self }
    }

    pub const fn t_to_r(self) -> Alu {
        Alu { t_to_r: true, ..self }
    }

    pub const fn store(self) -> Alu {
        Alu { store: true, ..self }
    }

    pub const fn ret(self) -> Alu {
        Alu { ret: true, ..self }
    }

    pub fn encode(self) -> u16 {
        let delta = |d: i8| (d as u16) & 3;
        0x6000
            | u16::from(self.ret) << 12
            | self.op.code() << 8
            | u16::from(self.t_to_n) << 7
            | u16::from(self.t_to_r) << 6
            | u16::from(self.store) << 5
            | delta(self.rd) << 2
            | delta(self.dd)
    }

    fn decode(word: u16) -> Alu {
        // Two-bit signed: 00 = 0, 01 = +1, 10 = -2, 11 = -1.
        let delta = |bits: u16| ((bits as i8) << 6) >> 6;
        Alu {
            op: Op::from_code(word >> 8),
            ret: word & 1 << 12 != 0,
            t_to_n: word & 1 << 7 != 0,
            t_to_r: word & 1 << 6 != 0,
            store: word & 1 << 5 != 0,
            rd: delta(word >> 2 & 3),
            dd: delta(word & 3),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Insn {
    /// Push a number from 0 to [`LITERAL_MAX`].
    Lit(u16),
    Jump(u16),
    /// Pop; jump if it was 0.
    Jz(u16),
    Call(u16),
    Alu(Alu),
}

impl Insn {
    pub fn encode(self) -> u16 {
        match self {
            Insn::Lit(v) => 0x8000 | (v & LITERAL_MAX),
            Insn::Jump(a) => a & ADDRESS_MASK,
            Insn::Jz(a) => 0x2000 | (a & ADDRESS_MASK),
            Insn::Call(a) => 0x4000 | (a & ADDRESS_MASK),
            Insn::Alu(alu) => alu.encode(),
        }
    }

    pub fn decode(word: u16) -> Insn {
        let a = word & ADDRESS_MASK;
        match word >> 13 {
            4..=7 => Insn::Lit(word & LITERAL_MAX),
            0 => Insn::Jump(a),
            1 => Insn::Jz(a),
            2 => Insn::Call(a),
            _ => Insn::Alu(Alu::decode(word)),
        }
    }
}

/// The words that are exactly one ALU instruction, as the assembler and
/// disassembler name them.
pub const PRIMITIVES: &[(&str, Alu)] = &[
    ("noop", Alu::new(Op::T)),
    ("dup", Alu::new(Op::T).t_to_n().d(1)),
    ("over", Alu::new(Op::N).t_to_n().d(1)),
    ("drop", Alu::new(Op::N).d(-1)),
    ("nip", Alu::new(Op::T).d(-1)),
    ("swap", Alu::new(Op::N).t_to_n()),
    ("+", Alu::new(Op::Add).d(-1)),
    ("-", Alu::new(Op::Sub).d(-1)),
    ("and", Alu::new(Op::And).d(-1)),
    ("or", Alu::new(Op::Or).d(-1)),
    ("xor", Alu::new(Op::Xor).d(-1)),
    ("invert", Alu::new(Op::Not)),
    ("=", Alu::new(Op::Eq).d(-1)),
    ("<", Alu::new(Op::Lt).d(-1)),
    ("u<", Alu::new(Op::ULt).d(-1)),
    ("lshift", Alu::new(Op::Shl).d(-1)),
    ("rshift", Alu::new(Op::Shr).d(-1)),
    ("1-", Alu::new(Op::Dec)),
    ("@", Alu::new(Op::Load)),
    (">r", Alu::new(Op::N).t_to_r().d(-1).r(1)),
    ("r>", Alu::new(Op::R).t_to_n().d(1).r(-1)),
    ("r@", Alu::new(Op::R).t_to_n().d(1)),
    ("dup>r", Alu::new(Op::T).t_to_r().r(1)),
    ("rdrop", Alu::new(Op::T).r(-1)),
    ("exit", Alu::new(Op::T).ret().r(-1)),
];

/// The first half of `!`: store `N` at `T`, leaving `N` on top.
pub const STORE: Alu = Alu::new(Op::N).store().d(-1);

pub fn primitive(name: &str) -> Option<Alu> {
    PRIMITIVES.iter().find(|(n, _)| *n == name).map(|&(_, alu)| alu)
}

/// One instruction as text, like `dup`, `+ ;`, `lit 42` or `call 0x0011`.
pub fn disasm(word: u16) -> String {
    disasm_with(word, |_| None)
}

/// Like [`disasm`], naming jump and call targets with `name_of`.
pub fn disasm_with(word: u16, name_of: impl Fn(u16) -> Option<String>) -> String {
    let target = |a: u16| name_of(a).unwrap_or_else(|| format!("{a:#06x}"));
    match Insn::decode(word) {
        Insn::Lit(v) if v < 0x100 => format!("lit {v}"),
        Insn::Lit(v) => format!("lit {v:#06x}"),
        Insn::Jump(a) => format!("jump {}", target(a)),
        Insn::Jz(a) => format!("jz {}", target(a)),
        Insn::Call(a) => format!("call {}", target(a)),
        Insn::Alu(alu) => {
            let named = |alu: Alu| PRIMITIVES.iter().find(|(_, a)| *a == alu).map(|(n, _)| n.to_string());
            if let Some(name) = named(alu) {
                return name;
            }
            if alu.ret && alu.rd == -1 {
                let body = Alu { ret: false, rd: 0, ..alu };
                if let Some(name) = named(body) {
                    return format!("{name} ;");
                }
            }
            if alu == STORE {
                return "store".into();
            }
            let mut parts = vec![format!("alu {}", alu.op.symbol())];
            let flags = [(alu.t_to_n, "T→N"), (alu.t_to_r, "T→R"), (alu.store, "N→[T]"), (alu.ret, "R→PC")];
            parts.extend(flags.iter().filter(|(on, _)| *on).map(|(_, f)| f.to_string()));
            if alu.dd != 0 {
                parts.push(format!("d{:+}", alu.dd));
            }
            if alu.rd != 0 {
                parts.push(format!("r{:+}", alu.rd));
            }
            parts.join(" ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_instruction_round_trips() {
        for word in 0..=u16::MAX {
            let insn = Insn::decode(word);
            // Bit 4 of an ALU instruction is ignored, so it doesn't survive.
            let expected = if word >> 13 == 3 { word & !0x10 } else { word };
            assert_eq!(insn.encode(), expected, "{word:#06x} decoded as {insn:?}");
        }
    }

    #[test]
    fn primitives_have_distinct_encodings() {
        for (i, (a, x)) in PRIMITIVES.iter().enumerate() {
            for (b, y) in &PRIMITIVES[i + 1..] {
                assert_ne!(x.encode(), y.encode(), "`{a}` and `{b}`");
            }
        }
    }

    #[test]
    fn disassembles() {
        assert_eq!(disasm(Insn::Lit(42).encode()), "lit 42");
        assert_eq!(disasm(Insn::Lit(0x6000).encode()), "lit 0x6000");
        assert_eq!(disasm(Insn::Call(0x11).encode()), "call 0x0011");
        assert_eq!(disasm(primitive("dup").unwrap().encode()), "dup");
        assert_eq!(disasm(primitive("+").unwrap().ret().r(-1).encode()), "+ ;");
        assert_eq!(disasm(STORE.encode()), "store");
        assert_eq!(disasm(Alu::new(Op::Add).t_to_r().d(1).encode()), "alu T+N T→R d+1");
        assert_eq!(disasm_with(Insn::Jump(5).encode(), |a| (a == 5).then(|| "main".into())), "jump main");
    }

    #[test]
    fn ops_follow_the_spec() {
        assert_eq!(Op::Sub.eval(3, 10, 0, 0), 7, "N - T");
        assert_eq!(Op::Lt.eval(1, 0xFFFF, 0, 0), 0xFFFF, "-1 < 1 signed");
        assert_eq!(Op::ULt.eval(1, 0xFFFF, 0, 0), 0, "0xFFFF > 1 unsigned");
        assert_eq!(Op::Shl.eval(17, 1, 0, 0), 2, "only the low 4 bits of T count");
        assert_eq!(Op::Shr.eval(4, 0x8000, 0, 0), 0x0800);
        assert_eq!(Op::Dec.eval(0, 0, 0, 0), 0xFFFF);
    }
}
