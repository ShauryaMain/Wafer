//! The syntax tree of a `.chip` file, exactly as written.

use crate::diag::Span;
use crate::logic::min_width;

#[derive(Clone, Debug)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct File {
    pub chips: Vec<ChipDef>,
    pub tests: Vec<TestDef>,
}

#[derive(Clone, Debug)]
pub struct ChipDef {
    pub name: Ident,
    pub inputs: Vec<PortDecl>,
    pub outputs: Vec<PortDecl>,
    pub wires: Vec<PortDecl>,
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

/// One name in an `in`, `out` or `wire` line, like `a` or `sum[8]`.
#[derive(Clone, Debug)]
pub struct PortDecl {
    pub name: Ident,
    pub width: usize,
    pub span: Span,
}

/// `target = Chip(...)` or `target[3:0] = expr`.
#[derive(Clone, Debug)]
pub struct Stmt {
    pub target: Ident,
    pub select: Option<Select>,
    pub value: StmtValue,
    /// `target <- Chip(...)`: one of several drivers of a shared wire.
    pub drive: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum StmtValue {
    Call(Call),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct Call {
    pub chip: Ident,
    pub args: Vec<Arg>,
    pub span: Span,
}

/// A positional argument (`a`) or a named one (`cin: c`).
#[derive(Clone, Debug)]
pub struct Arg {
    pub name: Option<Ident>,
    pub value: Expr,
}

/// `[3]` or `[7:4]`. A single bit has `hi == lo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Select {
    pub hi: usize,
    pub lo: usize,
    pub span: Span,
}

impl Select {
    pub fn width(&self) -> usize {
        self.hi - self.lo + 1
    }
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    /// `a`, `fa0.sum`, `a[3]`, `fa0.sum[7:4]`
    Ref {
        base: Ident,
        port: Option<Ident>,
        select: Option<Select>,
    },
    Literal(Literal),
    /// `"pong.asm"`: only a `Rom`'s `file:` takes text.
    Str(String),
    /// `{hi, lo}`: parts are listed most significant first.
    Concat(Vec<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Literal {
    pub value: u64,
    pub digits_width: Option<usize>,
}

impl Literal {
    /// The width a literal has on its own: its digit count for hex and
    /// binary, otherwise just enough bits for the value.
    pub fn natural_width(&self) -> usize {
        self.digits_width.unwrap_or_else(|| min_width(self.value))
    }

    pub fn fits(&self, width: usize) -> bool {
        width >= 64 || self.value >> width == 0
    }
}

#[derive(Clone, Debug)]
pub struct TestDef {
    pub chip: Ident,
    /// An optional first row naming the columns, like `a b | sum`.
    pub header: Option<TestHeader>,
    /// Truth-table rows and `set` / `tick` / `expect` steps, in order.
    pub steps: Vec<TestStep>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TestStep {
    Row(TestRow),
    /// `set a = 1, b = 0x2A`
    Set {
        values: Vec<(Ident, TestValue)>,
        span: Span,
    },
    /// `tick` or `tick 10`
    Tick {
        count: u64,
        span: Span,
    },
    /// `expect count = 3, cout = *`
    Expect {
        values: Vec<(Ident, TestValue)>,
        span: Span,
    },
}

#[derive(Clone, Debug)]
pub struct TestHeader {
    pub inputs: Vec<Ident>,
    pub outputs: Vec<Ident>,
}

#[derive(Clone, Debug)]
pub struct TestRow {
    pub inputs: Vec<TestValue>,
    pub outputs: Vec<TestValue>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug)]
pub struct TestValue {
    pub kind: TestValueKind,
    pub span: Span,
}

#[derive(Clone, Copy, Debug)]
pub enum TestValueKind {
    Number(Literal),
    X,
    Z,
    /// `*`: don't care (outputs only).
    Any,
}
