//! The Forth-style assembler: `.asm` source in, ROM words out. The language
//! is described in `docs/crumb.md`.
//!
//! It works in two passes. The first finds every definition (`:`, `const`,
//! `var`, `table`) so words can be used before they're defined. The second
//! lays the program out from address 0 and fills in addresses at the end.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::isa::{
    self, ADDRESS_MASK, Alu, Insn, LITERAL_MAX, RAM_START, RAM_WORDS, ROM_WORDS, STORE, disasm_with, primitive,
};

/// A mistake in the source. `start..end` is a byte range in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub start: usize,
    pub end: usize,
    pub message: String,
    /// Short text to show under the mistake, like "did you mean `draw`?".
    pub label: Option<String>,
    pub notes: Vec<String>,
}

impl Error {
    fn new(start: usize, end: usize, message: impl Into<String>) -> Error {
        Error { start, end, message: message.into(), label: None, notes: Vec::new() }
    }

    fn label(mut self, label: impl Into<String>) -> Error {
        self.label = Some(label.into());
        self
    }

    fn note(mut self, note: impl Into<String>) -> Error {
        self.notes.push(note.into());
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Word,
    Table,
    Var,
    Const,
}

/// A name the program defines: a word's or table's ROM address, a
/// variable's RAM address, or a constant's value.
#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub value: u16,
    /// Where it's defined in the source.
    pub start: usize,
    pub end: usize,
}

/// An assembled program.
#[derive(Clone, Debug, Default)]
pub struct Program {
    /// ROM contents from address 0.
    pub words: Vec<u16>,
    /// For each word, the byte range of the source that produced it.
    pub origins: Vec<(usize, usize)>,
    pub symbols: Vec<Symbol>,
    /// Words of RAM taken by `var`s.
    pub ram_used: usize,
}

impl Program {
    pub fn symbol(&self, name: &str) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.name == name)
    }

    /// The word or table that starts at `addr`.
    pub fn name_at(&self, addr: u16) -> Option<&str> {
        self.symbols
            .iter()
            .find(|s| s.value == addr && matches!(s.kind, SymbolKind::Word | SymbolKind::Table))
            .map(|s| s.name.as_str())
    }

    /// Every word with its address, machine code, disassembly and the source
    /// that made it.
    pub fn listing(&self, source: &str) -> String {
        let mut out = String::new();
        for (addr, &word) in self.words.iter().enumerate() {
            let addr = addr as u16;
            if let Some(name) = self.name_at(addr) {
                let kind = self.symbol(name).map(|s| s.kind);
                let _ = writeln!(out, "{}{name}", if kind == Some(SymbolKind::Table) { "table " } else { ": " });
            }
            let (start, end) = self.origins[addr as usize];
            let from = source.get(start..end).unwrap_or("").lines().next().unwrap_or("");
            let _ = writeln!(out, "  {addr:04x}  {word:04x}  {:<22} {from}", self.describe(addr));
        }
        out
    }

    /// True when `addr` is inside a `table`: data, not an instruction.
    pub fn is_data(&self, addr: u16) -> bool {
        self.symbols
            .iter()
            .filter(|s| matches!(s.kind, SymbolKind::Word | SymbolKind::Table) && s.value <= addr)
            // A word that starts where an empty table does wins.
            .max_by_key(|s| (s.value, s.kind == SymbolKind::Word))
            .is_some_and(|s| s.kind == SymbolKind::Table)
    }

    /// The word at `addr` as text: its instruction with jump targets named,
    /// or `data 0x0009` inside a table.
    pub fn describe(&self, addr: u16) -> String {
        let word = self.words.get(usize::from(addr)).copied().unwrap_or(0);
        if self.is_data(addr) {
            format!("data {word:#06x}")
        } else {
            disasm_with(word, |a| self.name_at(a).map(str::to_string))
        }
    }
}

/// Built-in constants. A program's own `const` can replace them.
pub const BUILTIN_CONSTS: &[(&str, u16)] = &[
    ("SCREEN", isa::SCREEN),
    ("PAD", isa::PAD),
    ("FRAME", isa::FRAME),
    ("RANDOM", isa::RANDOM),
    ("LEDS", isa::LEDS),
    ("SWITCHES", isa::SWITCHES),
    ("NUMBER", isa::NUMBER),
    ("CONSOLE", isa::CONSOLE),
    ("BTN_UP", 1),
    ("BTN_DOWN", 2),
    ("BTN_LEFT", 4),
    ("BTN_RIGHT", 8),
    ("BTN_A", 16),
    ("BTN_B", 32),
    ("BTN_START", 64),
    ("BTN_SELECT", 128),
    ("true", 0xFFFF),
    ("false", 0),
];

/// One step of a word that takes several instructions.
#[derive(Clone, Copy)]
enum Step {
    Prim(&'static str),
    Lit(u16),
    Store,
}

const MACROS: &[(&str, &[Step])] = {
    use Step::*;
    &[
        ("!", &[Store, Prim("drop")]),
        ("1+", &[Lit(1), Prim("+")]),
        ("0=", &[Lit(0), Prim("=")]),
        (">", &[Prim("swap"), Prim("<")]),
        ("u>", &[Prim("swap"), Prim("u<")]),
        ("<>", &[Prim("="), Prim("invert")]),
        ("negate", &[Prim("invert"), Lit(1), Prim("+")]),
        ("2dup", &[Prim("over"), Prim("over")]),
        ("2drop", &[Prim("drop"), Prim("drop")]),
        ("tuck", &[Prim("swap"), Prim("over")]),
        ("rot", &[Prim(">r"), Prim("swap"), Prim("r>"), Prim("swap")]),
        ("+!", &[Prim("dup>r"), Prim("@"), Prim("+"), Prim("r>"), Store, Prim("drop")]),
        ("2*", &[Lit(1), Prim("lshift")]),
        ("2/", &[Lit(1), Prim("rshift")]),
        ("i", &[Prim("r@")]),
    ]
};

const CONTROL: &[&str] = &["if", "else", "then", "begin", "until", "again", "while", "repeat", "for", "next", "halt"];
const KEYWORDS: &[&str] = &[":", ";", "const", "var", "table", "end"];

fn is_builtin_word(name: &str) -> bool {
    primitive(name).is_some()
        || MACROS.iter().any(|(n, _)| *n == name)
        || CONTROL.contains(&name)
        || KEYWORDS.contains(&name)
}

pub fn assemble(source: &str) -> Result<Program, Vec<Error>> {
    let (toks, errors) = lex(source);
    let mut asm = Asm {
        source,
        toks,
        errors,
        names: HashMap::new(),
        items: Vec::new(),
        addrs: Vec::new(),
        ram_used: 0,
        words: Vec::new(),
        origins: Vec::new(),
        fixups: Vec::new(),
        label_at: None,
        def_start: 0,
        rom_full: false,
    };
    asm.collect();
    asm.emit_program();
    if asm.errors.is_empty() { Ok(asm.finish()) } else { Err(asm.errors) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Word,
    Str,
    Char,
}

#[derive(Clone, Copy, Debug)]
struct Tok<'a> {
    kind: Kind,
    /// Exactly as written, quotes included.
    text: &'a str,
    start: usize,
    end: usize,
    line: usize,
}

fn lex(source: &str) -> (Vec<Tok<'_>>, Vec<Error>) {
    let bytes = source.as_bytes();
    let (mut toks, mut errors) = (Vec::new(), Vec::new());
    let (mut i, mut line) = (0, 1);
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if c == b'"' || c == b'\'' {
            i += 1;
            loop {
                match bytes.get(i) {
                    None | Some(b'\n') => {
                        let what = if c == b'"' { "string" } else { "character" };
                        errors.push(Error::new(start, i, format!("this {what} is missing its closing quote")));
                        break;
                    }
                    Some(b'\\') => i += 2,
                    Some(&q) if q == c => {
                        i += 1;
                        break;
                    }
                    Some(_) => i += 1,
                }
            }
            let i_end = i.min(bytes.len());
            let kind = if c == b'"' { Kind::Str } else { Kind::Char };
            toks.push(Tok { kind, text: &source[start..i_end], start, end: i_end, line });
            i = i_end;
            continue;
        }
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let text = &source[start..i];
        if text.starts_with("//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if text.starts_with('(') {
            // A comment runs to the next `)`, across lines if need be.
            let mut j = start + 1;
            while j < bytes.len() && bytes[j] != b')' {
                if bytes[j] == b'\n' {
                    line += 1;
                }
                j += 1;
            }
            if j >= bytes.len() {
                errors.push(Error::new(start, start + 1, "this comment is missing its `)`"));
            }
            i = (j + 1).min(bytes.len());
            continue;
        }
        toks.push(Tok { kind: Kind::Word, text, start, end: i, line });
    }
    (toks, errors)
}

/// The values in a `"string"` or `'c'`, one per character.
fn unescape(tok: &Tok) -> Result<Vec<u16>, Error> {
    let inner = &tok.text[1..tok.text.len().saturating_sub(1).max(1)];
    let mut out = Vec::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        let value = if c == '\\' {
            match chars.next() {
                Some('n') => b'\n',
                Some('t') => b'\t',
                Some('0') => 0,
                Some(c @ ('\\' | '\'' | '"')) => c as u8,
                other => {
                    let shown = other.map_or(String::new(), |c| c.to_string());
                    return Err(Error::new(tok.start, tok.end, format!("`\\{shown}` isn't an escape Crumb knows"))
                        .note("the escapes are \\n, \\t, \\0, \\\\, \\' and \\\""));
                }
            }
        } else if c.is_ascii() {
            c as u8
        } else {
            return Err(Error::new(tok.start, tok.end, format!("`{c}` isn't plain ASCII"))
                .note("the console only has ASCII characters"));
        };
        out.push(u16::from(value));
    }
    Ok(out)
}

/// `42`, `-1`, `0x2A` or `0b1010_0000`. `None` if it isn't a number at all.
fn parse_number(text: &str) -> Option<Result<u16, String>> {
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    if !body.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let lower = body.to_ascii_lowercase();
    let (digits, radix) = if let Some(rest) = lower.strip_prefix("0x") {
        (rest, 16)
    } else if let Some(rest) = lower.strip_prefix("0b") {
        (rest, 2)
    } else {
        (lower.as_str(), 10)
    };
    let digits: String = digits.chars().filter(|&c| c != '_').collect();
    let value = match i64::from_str_radix(&digits, radix) {
        Ok(v) if !digits.is_empty() => v,
        _ => return Some(Err(format!("`{text}` isn't a valid number"))),
    };
    let value = if negative { -value } else { value };
    Some(to_word(value).ok_or_else(|| format!("`{text}` doesn't fit in 16 bits")))
}

/// -32768..=65535 as a 16-bit word.
fn to_word(value: i64) -> Option<u16> {
    (-32768..=65535).contains(&value).then(|| value.rem_euclid(65536) as u16)
}

#[derive(Clone, Copy, Debug)]
enum Name {
    Word(usize),
    Table(usize),
    Var(u16),
    Const(u16),
}

enum Item {
    /// `: name ... ;`: the name's token, the body's tokens, and the `;`.
    Def {
        name: usize,
        body: std::ops::Range<usize>,
        end: usize,
    },
    Table {
        body: std::ops::Range<usize>,
    },
}

#[derive(Clone, Copy)]
enum Fix {
    /// A jump, call or literal whose address goes in the low 13 bits.
    Address,
    /// A table entry: the whole word is the address.
    Raw,
}

struct Fixup {
    at: usize,
    item: usize,
    fix: Fix,
}

/// An open `if`, `begin` or `for`, waiting for its other half.
enum Ctl {
    If { jz: usize, tok: usize },
    Else { jump: usize, tok: usize },
    Begin { addr: u16, tok: usize },
    While { addr: u16, jz: usize, tok: usize },
    For { addr: u16, tok: usize },
}

struct Asm<'a> {
    source: &'a str,
    toks: Vec<Tok<'a>>,
    errors: Vec<Error>,
    names: HashMap<&'a str, (Name, usize)>,
    items: Vec<Item>,
    /// Each item's ROM address, filled in by pass 2.
    addrs: Vec<u16>,
    ram_used: usize,
    words: Vec<u16>,
    origins: Vec<(usize, usize)>,
    fixups: Vec<Fixup>,
    /// The address something jumps to, if one is right here: a return
    /// can't merge into the instruction before a jump target.
    label_at: Option<usize>,
    /// Where the definition being emitted starts.
    def_start: usize,
    rom_full: bool,
}

impl<'a> Asm<'a> {
    fn error_at(&mut self, tok: usize, message: impl Into<String>) -> &mut Error {
        let t = self.toks[tok];
        self.errors.push(Error::new(t.start, t.end, message));
        self.errors.last_mut().expect("just pushed")
    }

    fn is(&self, i: usize, text: &str) -> bool {
        self.toks.get(i).is_some_and(|t| t.kind == Kind::Word && t.text == text)
    }

    /// Pass 1: find every definition.
    fn collect(&mut self) {
        let mut i = 0;
        while i < self.toks.len() {
            let tok = self.toks[i];
            let word = if tok.kind == Kind::Word { tok.text } else { "" };
            i = match word {
                ":" => self.collect_def(i),
                "const" => self.collect_const(i),
                "var" => self.collect_var(i),
                "table" => self.collect_table(i),
                ";" => {
                    self.error_at(i, "`;` without a `:` to start the definition");
                    i + 1
                }
                _ => {
                    self.error_at(i, "code has to go inside a definition").notes.push(
                        "like `: main ... ;`. Outside one, only `:`, `const`, `var` and `table` can start".into(),
                    );
                    // One error per stray stretch, not one per word.
                    let mut j = i + 1;
                    while j < self.toks.len() && ![":", "const", "var", "table"].iter().any(|k| self.is(j, k)) {
                        j += 1;
                    }
                    j
                }
            };
        }
    }

    /// Checks a new name and records it.
    fn define(&mut self, tok: usize, name: Name) {
        let t = self.toks[tok];
        if t.kind != Kind::Word {
            self.error_at(tok, "a name can't be a string or a character");
            return;
        }
        if is_builtin_word(t.text) {
            self.error_at(tok, format!("`{}` is a built-in word, so it can't be redefined", t.text));
            return;
        }
        if parse_number(t.text).is_some() {
            self.error_at(tok, format!("`{}` looks like a number, so it can't be a name", t.text));
            return;
        }
        if let Some(&(_, first)) = self.names.get(t.text) {
            let line = self.toks[first].line;
            self.error_at(tok, format!("`{}` is already defined", t.text))
                .notes
                .push(format!("the first one is on line {line}"));
            return;
        }
        self.names.insert(t.text, (name, tok));
    }

    fn name_after(&mut self, keyword: usize, what: &str) -> Option<usize> {
        let next = keyword + 1;
        let ok = self.toks.get(next).is_some_and(|t| t.line == self.toks[keyword].line && t.kind == Kind::Word)
            && ![";", ":"].iter().any(|k| self.is(next, k));
        if !ok {
            let word = self.toks[keyword].text;
            self.error_at(keyword, format!("`{word}` needs a name after it")).notes.push(format!("like `{what}`"));
            return None;
        }
        Some(next)
    }

    fn collect_def(&mut self, colon: usize) -> usize {
        let Some(name) = self.name_after(colon, ": move_paddle ... ;") else { return colon + 1 };
        let mut j = name + 1;
        while j < self.toks.len() && !self.is(j, ";") {
            if self.is(j, ":") || self.is(j, "const") || self.is(j, "var") || self.is(j, "table") {
                break;
            }
            j += 1;
        }
        let closed = self.is(j, ";");
        if !closed {
            let word = self.toks[name].text;
            self.error_at(name, format!("`{word}` is missing its `;`")).label =
                Some("this definition never ends".into());
        }
        // Defined either way, so calls to it don't pile on more errors.
        self.define(name, Name::Word(self.items.len()));
        self.items.push(Item::Def { name, body: name + 1..j, end: if closed { j } else { name } });
        if closed { j + 1 } else { j }
    }

    fn collect_const(&mut self, keyword: usize) -> usize {
        let Some(name) = self.name_after(keyword, "const SPEED = 2") else { return keyword + 1 };
        let line = self.toks[keyword].line;
        let mut end = name + 1;
        while end < self.toks.len() && self.toks[end].line == line {
            end += 1;
        }
        if !self.is(name + 1, "=") || name + 2 >= end {
            let word = self.toks[name].text;
            self.error_at(name, format!("`{word}` needs a value"))
                .notes
                .push(format!("like `const {word} = 2`, all on one line"));
            return end;
        }
        if let Some(value) = self.const_expr(name + 2..end) {
            self.define(name, Name::Const(value));
        }
        end
    }

    /// `term (op term)*` with `*` before `+` and `-`.
    fn const_expr(&mut self, range: std::ops::Range<usize>) -> Option<u16> {
        let mut sum: i64 = 0;
        let mut product: Option<i64> = None;
        let mut sign = 1;
        let mut expect_term = true;
        for i in range.clone() {
            let tok = self.toks[i];
            if expect_term {
                let value = self.const_term(i)?;
                product = Some(product.map_or(value, |p| p * value));
                expect_term = false;
                continue;
            }
            match tok.text {
                "*" => {}
                "+" | "-" => {
                    sum += sign * product.take().unwrap_or(0);
                    sign = if tok.text == "-" { -1 } else { 1 };
                }
                _ => {
                    self.error_at(i, format!("expected `+`, `-` or `*`, found `{}`", tok.text));
                    return None;
                }
            }
            expect_term = true;
        }
        if expect_term {
            let last = range.end - 1;
            self.error_at(last, "this needs a number after it");
            return None;
        }
        sum += sign * product.unwrap_or(0);
        match to_word(sum) {
            Some(v) => Some(v),
            None => {
                let (first, last) = (self.toks[range.start], self.toks[range.end - 1]);
                self.errors.push(Error::new(first.start, last.end, format!("{sum} doesn't fit in 16 bits")));
                None
            }
        }
    }

    fn const_term(&mut self, i: usize) -> Option<i64> {
        let tok = self.toks[i];
        match tok.kind {
            Kind::Char => return self.char_value(i).map(i64::from),
            Kind::Str => {
                self.error_at(i, "a constant can't be a string");
                return None;
            }
            Kind::Word => {}
        }
        if let Some(&(name, _)) = self.names.get(tok.text) {
            return match name {
                Name::Const(v) => Some(i64::from(v as i16)),
                Name::Var(addr) => Some(i64::from(addr)),
                Name::Word(_) | Name::Table(_) => {
                    self.error_at(i, format!("`{}` doesn't have an address yet", tok.text))
                        .notes
                        .push("constants can use numbers, other constants and variables".into());
                    None
                }
            };
        }
        if let Some(&(_, v)) = BUILTIN_CONSTS.iter().find(|(n, _)| *n == tok.text) {
            return Some(i64::from(v));
        }
        match parse_number(tok.text) {
            // Signed, so `-1 * 2` means -2.
            Some(Ok(_)) => {
                let negative = tok.text.starts_with('-');
                let v = parse_number(tok.text.trim_start_matches('-')).and_then(Result::ok).map(i64::from)?;
                Some(if negative { -v } else { v })
            }
            Some(Err(message)) => {
                self.error_at(i, message);
                None
            }
            None => {
                self.unknown(i);
                None
            }
        }
    }

    fn char_value(&mut self, i: usize) -> Option<u16> {
        match unescape(&self.toks[i]) {
            Ok(values) if values.len() == 1 => Some(values[0]),
            Ok(_) => {
                self.error_at(i, "a character literal holds exactly one character")
                    .notes
                    .push("use \"double quotes\" for a string in a `table`".into());
                None
            }
            Err(e) => {
                self.errors.push(e);
                None
            }
        }
    }

    fn collect_var(&mut self, keyword: usize) -> usize {
        let Some(name) = self.name_after(keyword, "var score  or  var board[200]") else { return keyword + 1 };
        let tok = self.toks[name];
        let (base, count) = match tok.text.split_once('[') {
            Some((base, rest)) => {
                let count = rest.strip_suffix(']').and_then(|n| {
                    parse_number(n).and_then(Result::ok).or_else(|| match self.names.get(n) {
                        Some(&(Name::Const(v), _)) => Some(v),
                        _ => BUILTIN_CONSTS.iter().find(|(c, _)| *c == n).map(|&(_, v)| v),
                    })
                });
                match count {
                    Some(count) if count > 0 => (base, usize::from(count)),
                    _ => {
                        self.error_at(name, format!("`{}` needs a size, like `{base}[10]`", tok.text));
                        return name + 1;
                    }
                }
            }
            None => (tok.text, 1),
        };
        if self.ram_used + count > RAM_WORDS {
            let used = self.ram_used;
            self.error_at(name, format!("`{base}` doesn't fit: RAM holds {RAM_WORDS} words"))
                .notes
                .push(format!("{used} are already taken"));
            return name + 1;
        }
        // Define just the name part: `board`, not `board[200]`.
        let start = tok.start;
        self.toks[name] = Tok { text: base, end: start + base.len(), ..tok };
        self.define(name, Name::Var(RAM_START + self.ram_used as u16));
        self.ram_used += count;
        name + 1
    }

    fn collect_table(&mut self, keyword: usize) -> usize {
        let Some(name) = self.name_after(keyword, "table pieces 0x0F00 0x4444 end") else { return keyword + 1 };
        let mut j = name + 1;
        while j < self.toks.len() && !self.is(j, "end") {
            if self.is(j, ":") || self.is(j, "table") {
                break;
            }
            j += 1;
        }
        if !self.is(j, "end") {
            let word = self.toks[name].text;
            self.error_at(name, format!("table `{word}` is missing its `end`"));
            return j;
        }
        self.define(name, Name::Table(self.items.len()));
        self.items.push(Item::Table { body: name + 1..j });
        j + 1
    }

    /// Pass 2: lay out the program from address 0.
    fn emit_program(&mut self) {
        let main = match self.names.get("main") {
            Some(&(Name::Word(item), tok)) => Some((item, tok)),
            Some(&(_, tok)) => {
                self.error_at(tok, "`main` has to be a word: `: main ... ;`");
                None
            }
            None => {
                let at = self.source.len();
                self.errors.push(
                    Error::new(0, at.min(1), "there's no `main` word")
                        .note("a program starts at address 0, which jumps to `main`: add `: main ... ;`"),
                );
                None
            }
        };
        let origin = main.map_or((0, 0), |(_, tok)| (self.toks[tok].start, self.toks[tok].end));
        self.emit(Insn::Jump(0).encode(), origin);
        if let Some((item, _)) = main {
            self.fixups.push(Fixup { at: 0, item, fix: Fix::Address });
        }

        for index in 0..self.items.len() {
            self.addrs.push(self.here());
            match self.items[index] {
                Item::Def { name, ref body, end } => {
                    let body = body.clone();
                    self.def_start = self.words.len();
                    self.label_at = Some(self.words.len());
                    self.emit_def(name, body, end);
                }
                Item::Table { ref body, .. } => {
                    let body = body.clone();
                    self.emit_table(body);
                }
            }
        }
    }

    fn emit(&mut self, word: u16, origin: (usize, usize)) {
        self.words.push(word);
        self.origins.push(origin);
        if self.words.len() > ROM_WORDS && !self.rom_full {
            self.rom_full = true;
            self.errors.push(
                Error::new(origin.0, origin.1, format!("the program doesn't fit in the {ROM_WORDS}-word ROM"))
                    .label("this is where it runs out"),
            );
        }
    }

    fn here(&self) -> u16 {
        self.words.len() as u16
    }

    fn origin(&self, tok: usize) -> (usize, usize) {
        (self.toks[tok].start, self.toks[tok].end)
    }

    fn emit_insn(&mut self, insn: Insn, tok: usize) {
        self.emit(insn.encode(), self.origin(tok));
    }

    fn emit_alu(&mut self, alu: Alu, tok: usize) {
        self.emit_insn(Insn::Alu(alu), tok);
    }

    fn emit_prim(&mut self, name: &str, tok: usize) {
        self.emit_alu(primitive(name).expect("macros only use primitives"), tok);
    }

    fn emit_lit(&mut self, value: u16, tok: usize) {
        if value <= LITERAL_MAX {
            self.emit_insn(Insn::Lit(value), tok);
        } else {
            self.emit_insn(Insn::Lit(!value), tok);
            self.emit_prim("invert", tok);
        }
    }

    /// Points an earlier jump at the current address.
    fn patch_here(&mut self, at: usize) {
        let here = self.here();
        self.words[at] = (self.words[at] & !ADDRESS_MASK) | here;
        self.label_at = Some(here as usize);
    }

    /// A return, folded into the instruction before it when that's safe,
    /// or left out when nothing can reach it.
    fn emit_exit(&mut self, tok: usize) {
        let here = self.words.len();
        if here > self.def_start && self.label_at != Some(here) {
            let prev = self.words[here - 1];
            match Insn::decode(prev) {
                Insn::Alu(alu) if !alu.ret && alu.rd == 0 && !alu.t_to_r => {
                    self.words[here - 1] = alu.ret().r(-1).encode();
                    return;
                }
                // A tail call: jump there, and let it return for us.
                Insn::Call(_) => {
                    self.words[here - 1] = prev & ADDRESS_MASK;
                    return;
                }
                // After a jump (like `halt` or `again`), nothing reaches it.
                Insn::Jump(_) => return,
                _ => {}
            }
        }
        self.emit_prim("exit", tok);
    }

    fn emit_def(&mut self, name: usize, body: std::ops::Range<usize>, end: usize) {
        let mut ctl: Vec<Ctl> = Vec::new();
        for i in body {
            let tok = self.toks[i];
            match tok.kind {
                Kind::Str => {
                    self.error_at(i, "strings only go in a `table`")
                        .notes
                        .push("like `table greeting \"Hello!\" 0 end`, then use `greeting` to get its address".into());
                }
                Kind::Char => {
                    if let Some(v) = self.char_value(i) {
                        self.emit_lit(v, i);
                    }
                }
                Kind::Word => self.emit_word(i, &mut ctl),
            }
        }
        for open in ctl {
            let (tok, word, closer) = match open {
                Ctl::If { tok, .. } => (tok, "if", "then"),
                Ctl::Else { tok, .. } => (tok, "else", "then"),
                Ctl::Begin { tok, .. } => (tok, "begin", "until`, `again` or `while"),
                Ctl::While { tok, .. } => (tok, "while", "repeat"),
                Ctl::For { tok, .. } => (tok, "for", "next"),
            };
            let def = self.toks[name].text;
            self.error_at(tok, format!("this `{word}` is missing its `{closer}`"))
                .notes
                .push(format!("it has to close before the `;` that ends `{def}`"));
        }
        self.emit_exit(end);
    }

    fn emit_word(&mut self, i: usize, ctl: &mut Vec<Ctl>) {
        let text = self.toks[i].text;
        if let Some(&(name, _)) = self.names.get(text) {
            match name {
                Name::Const(v) | Name::Var(v) => self.emit_lit(v, i),
                Name::Table(item) => {
                    self.fixups.push(Fixup { at: self.words.len(), item, fix: Fix::Address });
                    self.emit_insn(Insn::Lit(0), i);
                }
                Name::Word(item) => {
                    self.fixups.push(Fixup { at: self.words.len(), item, fix: Fix::Address });
                    self.emit_insn(Insn::Call(0), i);
                }
            }
            return;
        }
        let mismatch = |asm: &mut Asm, closer: &str, opener: &str| {
            asm.error_at(i, format!("`{closer}` without {opener}"));
        };
        match text {
            "if" => {
                ctl.push(Ctl::If { jz: self.words.len(), tok: i });
                self.emit_insn(Insn::Jz(0), i);
            }
            "else" => match ctl.pop() {
                Some(Ctl::If { jz, .. }) => {
                    let jump = self.words.len();
                    self.emit_insn(Insn::Jump(0), i);
                    self.patch_here(jz);
                    ctl.push(Ctl::Else { jump, tok: i });
                }
                other => {
                    ctl.extend(other);
                    mismatch(self, "else", "an `if`");
                }
            },
            "then" => match ctl.pop() {
                Some(Ctl::If { jz: at, .. } | Ctl::Else { jump: at, .. }) => self.patch_here(at),
                other => {
                    ctl.extend(other);
                    mismatch(self, "then", "an `if`");
                }
            },
            "begin" => {
                self.label_at = Some(self.words.len());
                ctl.push(Ctl::Begin { addr: self.here(), tok: i });
            }
            "until" | "again" => match ctl.pop() {
                Some(Ctl::Begin { addr, .. }) => {
                    let insn = if text == "until" { Insn::Jz(addr) } else { Insn::Jump(addr) };
                    self.emit_insn(insn, i);
                }
                other => {
                    ctl.extend(other);
                    mismatch(self, text, "a `begin`");
                }
            },
            "while" => match ctl.pop() {
                Some(Ctl::Begin { addr, .. }) => {
                    ctl.push(Ctl::While { addr, jz: self.words.len(), tok: i });
                    self.emit_insn(Insn::Jz(0), i);
                }
                other => {
                    ctl.extend(other);
                    mismatch(self, "while", "a `begin`");
                }
            },
            "repeat" => match ctl.pop() {
                Some(Ctl::While { addr, jz, .. }) => {
                    self.emit_insn(Insn::Jump(addr), i);
                    self.patch_here(jz);
                }
                other => {
                    ctl.extend(other);
                    mismatch(self, "repeat", "a `begin ... while`");
                }
            },
            "for" => {
                self.emit_prim(">r", i);
                self.label_at = Some(self.words.len());
                ctl.push(Ctl::For { addr: self.here(), tok: i });
            }
            "next" => match ctl.pop() {
                Some(Ctl::For { addr, .. }) => {
                    // Count down the copy on the return stack; leave when it hits 0.
                    self.emit_prim("r>", i);
                    self.emit_prim("1-", i);
                    self.emit_prim("dup>r", i);
                    let jz = self.words.len();
                    self.emit_insn(Insn::Jz(0), i);
                    self.emit_insn(Insn::Jump(addr), i);
                    self.patch_here(jz);
                    self.emit_prim("rdrop", i);
                }
                other => {
                    ctl.extend(other);
                    mismatch(self, "next", "a `for`");
                }
            },
            "halt" => self.emit_insn(Insn::Jump(self.here()), i),
            "exit" => self.emit_exit(i),
            ":" | "const" | "var" | "table" | "end" => {
                self.error_at(i, format!("`{text}` can't go inside a definition"))
                    .notes
                    .push("is the definition before it missing its `;`?".into());
            }
            _ => {
                if let Some(alu) = primitive(text) {
                    self.emit_alu(alu, i);
                } else if let Some((_, steps)) = MACROS.iter().find(|(n, _)| *n == text) {
                    for step in *steps {
                        match *step {
                            Step::Prim(p) => self.emit_prim(p, i),
                            Step::Lit(v) => self.emit_lit(v, i),
                            Step::Store => self.emit_alu(STORE, i),
                        }
                    }
                } else if let Some(&(_, v)) = BUILTIN_CONSTS.iter().find(|(n, _)| *n == text) {
                    self.emit_lit(v, i);
                } else {
                    match parse_number(text) {
                        Some(Ok(v)) => self.emit_lit(v, i),
                        Some(Err(message)) => {
                            self.error_at(i, message);
                        }
                        None => self.unknown(i),
                    }
                }
            }
        }
    }

    fn emit_table(&mut self, body: std::ops::Range<usize>) {
        for i in body {
            let tok = self.toks[i];
            match tok.kind {
                Kind::Str => match unescape(&tok) {
                    Ok(values) => {
                        for v in values {
                            self.emit(v, self.origin(i));
                        }
                    }
                    Err(e) => self.errors.push(e),
                },
                Kind::Char => {
                    if let Some(v) = self.char_value(i) {
                        self.emit(v, self.origin(i));
                    }
                }
                Kind::Word => {
                    let value = match self.names.get(tok.text) {
                        Some(&(Name::Const(v) | Name::Var(v), _)) => Some(v),
                        Some(&(Name::Word(item) | Name::Table(item), _)) => {
                            self.fixups.push(Fixup { at: self.words.len(), item, fix: Fix::Raw });
                            Some(0)
                        }
                        None => match BUILTIN_CONSTS.iter().find(|(n, _)| *n == tok.text) {
                            Some(&(_, v)) => Some(v),
                            None => match parse_number(tok.text) {
                                Some(Ok(v)) => Some(v),
                                Some(Err(message)) => {
                                    self.error_at(i, message);
                                    None
                                }
                                None => {
                                    self.unknown(i);
                                    None
                                }
                            },
                        },
                    };
                    if let Some(v) = value {
                        self.emit(v, self.origin(i));
                    }
                }
            }
        }
    }

    fn unknown(&mut self, i: usize) {
        let text = self.toks[i].text;
        let candidates: Vec<&str> = self
            .names
            .keys()
            .copied()
            .chain(isa::PRIMITIVES.iter().map(|(n, _)| *n))
            .chain(MACROS.iter().map(|(n, _)| *n))
            .chain(CONTROL.iter().copied())
            .chain(BUILTIN_CONSTS.iter().map(|(n, _)| *n))
            .collect();
        let close = suggest(text, &candidates);
        let error = self.error_at(i, format!("there's no word named `{text}`"));
        if let Some(close) = close {
            error.label = Some(format!("did you mean `{close}`?"));
        }
    }

    fn finish(self) -> Program {
        let mut words = self.words;
        for fixup in &self.fixups {
            let addr = self.addrs[fixup.item];
            words[fixup.at] = match fixup.fix {
                Fix::Address => (words[fixup.at] & !ADDRESS_MASK) | addr,
                Fix::Raw => addr,
            };
        }
        let mut symbols: Vec<Symbol> = self
            .names
            .iter()
            .map(|(&name, &(kind, tok))| {
                let (kind, value) = match kind {
                    Name::Word(item) => (SymbolKind::Word, self.addrs[item]),
                    Name::Table(item) => (SymbolKind::Table, self.addrs[item]),
                    Name::Var(addr) => (SymbolKind::Var, addr),
                    Name::Const(v) => (SymbolKind::Const, v),
                };
                let t = self.toks[tok];
                Symbol { name: name.to_string(), kind, value, start: t.start, end: t.end }
            })
            .collect();
        symbols.sort_by_key(|s| (s.kind as u8, s.value, s.name.clone()));
        Program { words, origins: self.origins, symbols, ram_used: self.ram_used }
    }
}

/// The closest candidate to a misspelled name, if any is close enough.
fn suggest<'b>(name: &str, candidates: &[&'b str]) -> Option<&'b str> {
    let limit = (name.chars().count() / 3).max(1);
    candidates
        .iter()
        .filter(|c| **c != name)
        .map(|c| (edit_distance(name, c), *c))
        .filter(|&(d, _)| d <= limit)
        .min_by_key(|&(d, c)| (d, c))
        .map(|(_, c)| c)
}

/// Edits needed to turn `a` into `b`, counting a swap of neighbors as one.
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j - 1] + cost).min(d[i - 1][j] + 1).min(d[i][j - 1] + 1);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}
