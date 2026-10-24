//! Formats `.chip` files in one consistent style, keeping every comment:
//!
//! - two-space indents, one statement per line, one blank line between
//!   chips and tests;
//! - `in`, `out` and `wire` lines padded so names line up;
//! - `=` aligned within each group of lines, and trailing comments too;
//! - test tables laid out in columns, lined up under a `// a b | out`
//!   header comment when there is one.
//!
//! Files with syntax errors are left alone.

use crate::diag::Diagnostic;
use crate::lexer::{Tok, Token, lex_with_comments};
use crate::parser::parse;

const INDENT: &str = "  ";

pub fn format(text: &str) -> Result<String, String> {
    let mut diags = Vec::new();
    parse(0, text, &mut diags);
    if diags.iter().any(Diagnostic::is_error) {
        return Err("fix the syntax errors first, then format".into());
    }
    let toks = lex_with_comments(0, text, &mut Vec::new());
    let mut f = Formatter { text, toks, pos: 0 };
    let items = f.top_items();
    Ok(print(&items))
}

enum TopItem {
    Blank,
    Comment(String),
    Block(Block),
}

struct Block {
    keyword: String,
    name: String,
    header_comment: Option<String>,
    lines: Vec<BodyLine>,
    closing_comment: Option<String>,
}

enum BodyLine {
    Blank,
    Comment(String),
    Code { parts: Vec<String>, kind: CodeKind, comment: Option<String> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CodeKind {
    /// `in a, b`: parts are [keyword, names].
    Decl,
    /// `target = value`: parts are [target, operator, value].
    Stmt,
    /// A test row or header: one part per cell.
    Row,
    /// Anything else, printed as written.
    Other,
}

struct Formatter<'a> {
    text: &'a str,
    toks: Vec<Token>,
    pos: usize,
}

impl Formatter<'_> {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if t.tok != Tok::Eof {
            self.pos += 1;
        }
        t
    }

    fn slice(&self, t: &Token) -> &str {
        &self.text[t.span.start..t.span.end]
    }

    fn top_items(&mut self) -> Vec<TopItem> {
        let mut items = Vec::new();
        let mut newlines = 0;
        loop {
            match self.peek().clone() {
                Tok::Eof => return items,
                Tok::Newline => {
                    self.bump();
                    newlines += 1;
                    if newlines == 2 && !items.is_empty() {
                        items.push(TopItem::Blank);
                    }
                }
                Tok::Comment => {
                    let t = self.bump();
                    items.push(TopItem::Comment(self.slice(&t).to_string()));
                    newlines = 0;
                }
                Tok::Ident(word) if word == "chip" || word == "test" => {
                    let block = self.block();
                    items.push(TopItem::Block(block));
                    newlines = 0;
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn block(&mut self) -> Block {
        let keyword = self.bump();
        let name = self.bump();
        self.bump(); // `{`
        let mut block = Block {
            keyword: self.slice(&keyword).to_string(),
            name: self.slice(&name).to_string(),
            header_comment: None,
            lines: Vec::new(),
            closing_comment: None,
        };
        if *self.peek() == Tok::Comment {
            let t = self.bump();
            block.header_comment = Some(self.slice(&t).to_string());
        }
        let is_test = block.keyword == "test";

        let mut current: Vec<Token> = Vec::new();
        let mut comment: Option<String> = None;
        let mut depth = 0;
        let mut line_open = false;
        let mut blank_run = 0;
        loop {
            match self.peek().clone() {
                Tok::RBrace if depth == 0 => {
                    self.bump();
                    self.finish_line(&mut block.lines, &mut current, &mut comment, is_test);
                    break;
                }
                Tok::Eof => {
                    self.finish_line(&mut block.lines, &mut current, &mut comment, is_test);
                    break;
                }
                Tok::Newline => {
                    self.bump();
                    if depth > 0 {
                        continue;
                    }
                    if line_open {
                        self.finish_line(&mut block.lines, &mut current, &mut comment, is_test);
                        line_open = false;
                        blank_run = 0;
                    } else {
                        blank_run += 1;
                        if blank_run == 1 && !block.lines.is_empty() {
                            block.lines.push(BodyLine::Blank);
                        }
                    }
                }
                Tok::Semi if depth == 0 => {
                    self.bump();
                    self.finish_line(&mut block.lines, &mut current, &mut comment, is_test);
                }
                Tok::Comment => {
                    let t = self.bump();
                    let text = self.slice(&t).to_string();
                    if current.is_empty() && depth == 0 && !line_open {
                        block.lines.push(BodyLine::Comment(text));
                    } else {
                        comment = Some(match comment.take() {
                            Some(earlier) => format!("{earlier} {}", text.trim_start_matches('/').trim()),
                            None => text,
                        });
                    }
                    line_open = true;
                }
                tok => {
                    match tok {
                        Tok::LParen | Tok::LBrace | Tok::LBracket => depth += 1,
                        Tok::RParen | Tok::RBrace | Tok::RBracket => depth -= 1,
                        _ => {}
                    }
                    let t = self.bump();
                    current.push(t);
                    line_open = true;
                }
            }
        }
        while matches!(block.lines.last(), Some(BodyLine::Blank)) {
            block.lines.pop();
        }
        if *self.peek() == Tok::Comment {
            let t = self.bump();
            block.closing_comment = Some(self.slice(&t).to_string());
        }
        block
    }

    fn finish_line(
        &self,
        lines: &mut Vec<BodyLine>,
        current: &mut Vec<Token>,
        comment: &mut Option<String>,
        is_test: bool,
    ) {
        if current.is_empty() {
            // A comment after `;` belongs to the statement before it.
            if let Some(text) = comment.take() {
                match lines.last_mut() {
                    Some(BodyLine::Code { comment: slot @ None, .. }) => *slot = Some(text),
                    _ => lines.push(BodyLine::Comment(text)),
                }
            }
            return;
        }
        let toks = std::mem::take(current);
        let is_step = matches!(&toks[0].tok, Tok::Ident(w) if matches!(w.as_str(), "set" | "tick" | "expect"));
        let line = if is_test && is_step {
            BodyLine::Code { parts: vec![self.join(&toks)], kind: CodeKind::Other, comment: comment.take() }
        } else if is_test {
            BodyLine::Code {
                parts: toks.iter().map(|t| self.slice(t).to_string()).collect(),
                kind: CodeKind::Row,
                comment: comment.take(),
            }
        } else {
            self.code_line(&toks, comment.take())
        };
        lines.push(line);
    }

    fn code_line(&self, toks: &[Token], comment: Option<String>) -> BodyLine {
        let first_is_keyword = matches!(&toks[0].tok, Tok::Ident(w) if matches!(w.as_str(), "in" | "out" | "wire"));
        if first_is_keyword && matches!(toks.get(1).map(|t| &t.tok), Some(Tok::Ident(_))) {
            return BodyLine::Code {
                parts: vec![self.slice(&toks[0]).to_string(), self.join(&toks[1..])],
                kind: CodeKind::Decl,
                comment,
            };
        }
        if let Some(eq) = toks.iter().position(|t| matches!(t.tok, Tok::Eq | Tok::Arrow)) {
            let parts = vec![self.join(&toks[..eq]), self.slice(&toks[eq]).to_string(), self.join(&toks[eq + 1..])];
            return BodyLine::Code { parts, kind: CodeKind::Stmt, comment };
        }
        BodyLine::Code { parts: vec![self.join(toks)], kind: CodeKind::Other, comment }
    }

    /// Prints tokens with the house spacing: `Xor(a, b)`, `a[7:4]`,
    /// `{hi, lo}`, `Mux(sel: s)`, `h1.sum`.
    fn join(&self, toks: &[Token]) -> String {
        let mut out = String::new();
        let mut brackets = 0;
        let mut prev: Option<&Tok> = None;
        for t in toks {
            let space = match (prev, &t.tok) {
                (None, _) => false,
                (Some(Tok::LParen | Tok::LBracket | Tok::LBrace | Tok::Dot), _) => false,
                (
                    _,
                    Tok::Comma | Tok::RParen | Tok::RBracket | Tok::RBrace | Tok::Dot | Tok::LParen | Tok::LBracket,
                ) => false,
                (_, Tok::Colon) => false,
                (Some(Tok::Colon), _) => brackets == 0,
                _ => true,
            };
            if space {
                out.push(' ');
            }
            out.push_str(self.slice(t));
            match t.tok {
                Tok::LBracket => brackets += 1,
                Tok::RBracket => brackets -= 1,
                _ => {}
            }
            prev = Some(&t.tok);
        }
        out
    }
}

fn print(items: &[TopItem]) -> String {
    let mut out = String::new();
    let mut after_block = false;
    for item in items {
        match item {
            TopItem::Blank => {
                if !out.is_empty() && !out.ends_with("\n\n") {
                    out.push('\n');
                }
                after_block = false;
            }
            TopItem::Comment(text) => {
                if after_block {
                    out.push('\n');
                }
                out.push_str(text);
                out.push('\n');
                after_block = false;
            }
            TopItem::Block(block) => {
                if after_block {
                    out.push('\n');
                }
                print_block(&mut out, block);
                after_block = true;
            }
        }
    }
    let trimmed = out.trim_matches('\n');
    if trimmed.is_empty() { String::new() } else { format!("{trimmed}\n") }
}

fn print_block(out: &mut String, block: &Block) {
    out.push_str(&format!("{} {} {{", block.keyword, block.name));
    if let Some(comment) = &block.header_comment {
        out.push_str(&format!(" {comment}"));
    }
    out.push('\n');
    if block.keyword == "test" {
        print_test_body(out, &block.lines);
    } else {
        print_chip_body(out, &block.lines);
    }
    out.push('}');
    if let Some(comment) = &block.closing_comment {
        out.push_str(&format!(" {comment}"));
    }
    out.push('\n');
}

/// Prints code lines with their trailing comments lined up two spaces past
/// the longest line in the group.
fn emit_group(out: &mut String, group: &[(String, Option<String>)]) {
    let width = group.iter().filter(|(_, c)| c.is_some()).map(|(code, _)| code.len()).max().unwrap_or(0);
    for (code, comment) in group {
        match comment {
            Some(comment) => out.push_str(&format!("{code:<width$}  {comment}\n")),
            None => out.push_str(&format!("{code}\n")),
        }
    }
}

fn print_chip_body(out: &mut String, lines: &[BodyLine]) {
    let mut i = 0;
    while i < lines.len() {
        match &lines[i] {
            BodyLine::Blank => {
                out.push('\n');
                i += 1;
            }
            BodyLine::Comment(text) => {
                out.push_str(&format!("{INDENT}{text}\n"));
                i += 1;
            }
            BodyLine::Code { .. } => {
                let start = i;
                while matches!(lines.get(i), Some(BodyLine::Code { .. })) {
                    i += 1;
                }
                let run = &lines[start..i];
                let target_width = run
                    .iter()
                    .filter_map(|l| match l {
                        BodyLine::Code { parts, kind: CodeKind::Stmt, .. } => Some(parts[0].len()),
                        _ => None,
                    })
                    .max()
                    .unwrap_or(0);
                let group: Vec<(String, Option<String>)> = run
                    .iter()
                    .filter_map(|l| {
                        let BodyLine::Code { parts, kind, comment } = l else { return None };
                        let code = match kind {
                            CodeKind::Decl => format!("{INDENT}{:<3} {}", parts[0], parts[1]),
                            CodeKind::Stmt => format!("{INDENT}{:<target_width$} {} {}", parts[0], parts[1], parts[2]),
                            CodeKind::Row | CodeKind::Other => format!("{INDENT}{}", parts.join(" ")),
                        };
                        Some((code, comment.clone()))
                    })
                    .collect();
                emit_group(out, &group);
            }
        }
    }
}

fn print_test_body(out: &mut String, lines: &[BodyLine]) {
    let rows: Vec<&Vec<String>> = lines
        .iter()
        .filter_map(|l| match l {
            BodyLine::Code { parts, kind: CodeKind::Row, .. } => Some(parts),
            _ => None,
        })
        .collect();
    let cells = rows.first().map_or(0, |r| r.len());

    // A `// a b | sum` comment right above the first row names the columns.
    let first_row = lines.iter().position(|l| matches!(l, BodyLine::Code { kind: CodeKind::Row, .. }));
    let header = first_row.and_then(|r| r.checked_sub(1)).and_then(|h| match &lines[h] {
        BodyLine::Comment(text) => {
            let words: Vec<String> = text.trim_start_matches('/').split_whitespace().map(str::to_string).collect();
            (words.len() == cells && cells > 0).then_some((h, words))
        }
        _ => None,
    });

    let mut widths = vec![0usize; rows.iter().map(|r| r.len()).max().unwrap_or(0)];
    for row in rows.iter().map(|r| r.as_slice()).chain(header.as_ref().map(|(_, w)| w.as_slice())) {
        for (c, cell) in row.iter().enumerate() {
            if c < widths.len() {
                widths[c] = widths[c].max(cell.len());
            }
        }
    }
    let lay_out =
        |cells: &[String]| -> String {
            let last = cells.len().saturating_sub(1);
            cells
                .iter()
                .enumerate()
                .map(|(c, cell)| {
                    if c == last {
                        cell.clone()
                    } else {
                        format!("{cell:<w$}", w = widths.get(c).copied().unwrap_or(0))
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
    let row_indent = if header.is_some() { format!("{INDENT}   ") } else { INDENT.to_string() };

    let mut group: Vec<(String, Option<String>)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let flush = |out: &mut String, group: &mut Vec<(String, Option<String>)>| {
            emit_group(out, group);
            group.clear();
        };
        match line {
            BodyLine::Code { parts, comment, kind: CodeKind::Row } => {
                group.push((format!("{row_indent}{}", lay_out(parts)), comment.clone()))
            }
            BodyLine::Code { parts, comment, .. } => {
                group.push((format!("{INDENT}{}", parts.join(" ")), comment.clone()))
            }
            BodyLine::Comment(_) if header.as_ref().is_some_and(|(h, _)| *h == i) => {
                flush(out, &mut group);
                let words = &header.as_ref().expect("checked").1;
                out.push_str(&format!("{INDENT}// {}\n", lay_out(words)));
            }
            BodyLine::Comment(text) => {
                flush(out, &mut group);
                out.push_str(&format!("{INDENT}{text}\n"));
            }
            BodyLine::Blank => {
                flush(out, &mut group);
                out.push('\n');
            }
        }
    }
    emit_group(out, &group);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidies_a_messy_chip() {
        let messy = "// top\n\n\nchip  HalfAdder{ // adds\nin a,b\n  out   sum ,carry\n\n\n  sum=Xor( a , b )   // s\n  carry = And(a:a,b : b)\n  wide = {a , b}; bit = wide[ 1 : 0 ]\n}\ntest HalfAdder {\n  // a b | sum carry\n  0 0|0 0\n  1   1 | 0 1 // both\n}";
        let tidy = format(messy).unwrap();
        assert_eq!(
            tidy,
            "// top\n\nchip HalfAdder { // adds\n  in  a, b\n  out sum, carry\n\n  sum   = Xor(a, b)  // s\n  carry = And(a: a, b: b)\n  wide  = {a, b}\n  bit   = wide[1:0]\n}\n\ntest HalfAdder {\n  // a b | sum carry\n     0 0 | 0   0\n     1 1 | 0   1  // both\n}\n"
        );
        assert_eq!(format(&tidy).unwrap(), tidy, "formatting twice changes nothing");
    }

    #[test]
    fn example_chips_are_stable_and_keep_their_meaning() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../chips");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "chip") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let once = format(&text).unwrap();
            assert_eq!(format(&once).unwrap(), once, "{} isn't stable", path.display());
            let comments =
                |t: &str| lex_with_comments(0, t, &mut Vec::new()).iter().filter(|t| t.tok == Tok::Comment).count();
            assert_eq!(comments(&text), comments(&once), "{} lost a comment", path.display());
            let mut diags = Vec::new();
            parse(0, &once, &mut diags);
            assert!(diags.is_empty(), "{} doesn't parse after formatting", path.display());
        }
    }

    #[test]
    fn formats_test_steps() {
        let messy = "chip C {\n  in r\n  out y\n  y = Not(r)\n}\ntest C {\n  set r=1 ;tick\n  expect y=0   // checked\n  tick  5\n}\n";
        let tidy = format(messy).unwrap();
        assert_eq!(
            tidy,
            "chip C {\n  in  r\n  out y\n  y = Not(r)\n}\n\ntest C {\n  set r = 1\n  tick\n  expect y = 0  // checked\n  tick 5\n}\n"
        );
        assert_eq!(format(&tidy).unwrap(), tidy);
    }

    #[test]
    fn refuses_broken_files() {
        assert!(format("chip A {\n  in a\n  y = \n").is_err());
    }
}
