//! Turns tokens into the syntax tree. On a mistake it reports a diagnostic,
//! skips to the end of the line, and keeps going so one typo doesn't hide
//! every other error in the file.

use crate::ast::*;
use crate::diag::{Diagnostic, FileId, Span};
use crate::lexer::{Tok, Token, lex};

pub fn parse(file: FileId, text: &str, diags: &mut Vec<Diagnostic>) -> File {
    let toks = lex(file, text, diags);
    Parser { toks, pos: 0, diags }.file()
}

/// The error has already been reported when this is `Err`.
type PResult<T> = Result<T, ()>;

struct Parser<'a> {
    toks: Vec<Token>,
    pos: usize,
    diags: &'a mut Vec<Diagnostic>,
}

impl Parser<'_> {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_at(&self, ahead: usize) -> &Tok {
        &self.toks[(self.pos + ahead).min(self.toks.len() - 1)].tok
    }

    fn span(&self) -> Span {
        self.toks[self.pos].span
    }

    fn prev_span(&self) -> Span {
        self.toks[self.pos.saturating_sub(1)].span
    }

    fn bump(&mut self) -> Token {
        let token = self.toks[self.pos].clone();
        if token.tok != Tok::Eof {
            self.pos += 1;
        }
        token
    }

    fn at(&self, tok: &Tok) -> bool {
        self.peek() == tok
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        let found = self.at(tok);
        if found {
            self.bump();
        }
        found
    }

    fn at_ident(&self, word: &str) -> bool {
        matches!(self.peek(), Tok::Ident(name) if name == word)
    }

    fn error_here(&mut self, expected: &str) {
        let found = self.peek().describe();
        self.diags.push(Diagnostic::error(self.span(), format!("expected {expected}, found {found}")));
    }

    fn expect(&mut self, tok: &Tok, expected: &str) -> PResult<Span> {
        if self.at(tok) {
            Ok(self.bump().span)
        } else {
            self.error_here(expected);
            Err(())
        }
    }

    fn ident(&mut self, expected: &str) -> PResult<Ident> {
        if let Tok::Ident(name) = self.peek() {
            let name = name.clone();
            let span = self.bump().span;
            Ok(Ident { name, span })
        } else {
            self.error_here(expected);
            Err(())
        }
    }

    fn number(&mut self, expected: &str) -> PResult<(u64, Span)> {
        if let Tok::Number { value, .. } = *self.peek() {
            Ok((value, self.bump().span))
        } else {
            self.error_here(expected);
            Err(())
        }
    }

    fn skip_newlines(&mut self) {
        while self.at(&Tok::Newline) {
            self.bump();
        }
    }

    /// After a mistake: skip the rest of the line, but stop before a `}` so
    /// the enclosing chip still closes.
    fn recover_to_line_end(&mut self) {
        while !matches!(self.peek(), Tok::Newline | Tok::RBrace | Tok::Eof) {
            self.bump();
        }
    }

    /// After a broken `chip` or `test` header: skip the whole block.
    fn skip_block(&mut self) {
        let mut depth = 0;
        loop {
            match self.peek() {
                Tok::Eof => return,
                Tok::LBrace => depth += 1,
                Tok::RBrace if depth <= 1 => {
                    self.bump();
                    return;
                }
                Tok::RBrace => depth -= 1,
                _ => {}
            }
            self.bump();
        }
    }

    fn file(mut self) -> File {
        let mut file = File::default();
        loop {
            self.skip_newlines();
            if self.at(&Tok::Eof) {
                return file;
            } else if self.at_ident("chip") {
                if let Ok(chip) = self.chip() {
                    file.chips.push(chip);
                }
            } else if self.at_ident("test") {
                if let Ok(test) = self.test() {
                    file.tests.push(test);
                }
            } else {
                self.error_here("`chip` or `test`");
                self.skip_block();
            }
        }
    }

    /// Parses `name {`, skipping the block if the header is broken.
    fn block_header(&mut self, expected_name: &str) -> PResult<Ident> {
        let Ok(name) = self.ident(expected_name) else {
            self.skip_block();
            return Err(());
        };
        if self.expect(&Tok::LBrace, "`{`").is_err() {
            self.skip_block();
            return Err(());
        }
        Ok(name)
    }

    fn chip(&mut self) -> PResult<ChipDef> {
        let keyword = self.bump().span;
        let name = self.block_header("a chip name")?;
        let mut chip = ChipDef {
            name,
            inputs: Vec::new(),
            outputs: Vec::new(),
            wires: Vec::new(),
            stmts: Vec::new(),
            span: keyword,
        };
        loop {
            while matches!(self.peek(), Tok::Newline | Tok::Semi) {
                self.bump();
            }
            match self.peek() {
                Tok::RBrace => {
                    chip.span = keyword.to(self.bump().span);
                    return Ok(chip);
                }
                Tok::Eof => {
                    self.diags.push(
                        Diagnostic::error(chip.name.span, "this chip is missing its closing `}`").label("opened here"),
                    );
                    return Ok(chip);
                }
                _ => {}
            }
            if self.chip_item(&mut chip).is_err() {
                self.recover_to_line_end();
            } else if !matches!(self.peek(), Tok::Newline | Tok::Semi | Tok::RBrace | Tok::Eof) {
                self.error_here("the end of the line");
                self.recover_to_line_end();
            }
        }
    }

    fn chip_item(&mut self, chip: &mut ChipDef) -> PResult<()> {
        // `in`, `out` and `wire` are only keywords at the start of a line and
        // followed by a name, so a port can still be called `out`.
        let keyword = match self.peek() {
            Tok::Ident(word) if matches!(word.as_str(), "in" | "out" | "wire") => word.clone(),
            _ => String::new(),
        };
        if !keyword.is_empty() && matches!(self.peek_at(1), Tok::Ident(_)) {
            self.bump();
            let ports = self.port_list()?;
            match keyword.as_str() {
                "in" => chip.inputs.extend(ports),
                "out" => chip.outputs.extend(ports),
                _ => chip.wires.extend(ports),
            }
            return Ok(());
        }
        let stmt = self.stmt()?;
        chip.stmts.push(stmt);
        Ok(())
    }

    fn port_list(&mut self) -> PResult<Vec<PortDecl>> {
        let mut ports = Vec::new();
        loop {
            let name = self.ident("a name")?;
            let mut width = 1;
            let mut span = name.span;
            if self.eat(&Tok::LBracket) {
                let (bits, bits_span) = self.number("a bus width, like `[8]`")?;
                let close = self.expect(&Tok::RBracket, "`]`")?;
                if bits == 0 || bits > 4096 {
                    self.diags.push(Diagnostic::error(bits_span, "a bus needs between 1 and 4096 bits"));
                    return Err(());
                }
                width = bits as usize;
                span = name.span.to(close);
            }
            ports.push(PortDecl { name, width, span });
            if !self.eat(&Tok::Comma) {
                return Ok(ports);
            }
        }
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let target = self.ident("a signal name")?;
        let select = if self.at(&Tok::LBracket) { Some(self.select()?) } else { None };
        let drive = self.eat(&Tok::Arrow);
        if !drive {
            self.expect(&Tok::Eq, "`=` or `<-`")?;
        }
        let is_call = matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::LParen;
        if drive && !is_call {
            self.diags.push(
                Diagnostic::error(self.span(), "`<-` needs a gate or chip on its right")
                    .note("like `data <- TriBuf(x, en)`: each `<-` line adds one driver to a shared wire"),
            );
            return Err(());
        }
        let value = if is_call { StmtValue::Call(self.call()?) } else { StmtValue::Expr(self.expr()?) };
        let span = target.span.to(self.prev_span());
        Ok(Stmt { target, select, value, drive, span })
    }

    fn call(&mut self) -> PResult<Call> {
        let chip = self.ident("a chip name")?;
        self.expect(&Tok::LParen, "`(`")?;
        let mut args = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&Tok::RParen) {
                break;
            }
            let name = if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Colon {
                let name = self.ident("a port name")?;
                self.bump();
                Some(name)
            } else {
                None
            };
            self.skip_newlines();
            let value = self.expr()?;
            args.push(Arg { name, value });
            self.skip_newlines();
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        let close = self.expect(&Tok::RParen, "`,` or `)`")?;
        Ok(Call { span: chip.span.to(close), chip, args })
    }

    fn expr(&mut self) -> PResult<Expr> {
        let start = self.span();
        match self.peek().clone() {
            Tok::Number { value, digits_width } => {
                self.bump();
                Ok(Expr { kind: ExprKind::Literal(Literal { value, digits_width }), span: start })
            }
            Tok::LBrace => {
                self.bump();
                let mut parts = Vec::new();
                loop {
                    self.skip_newlines();
                    parts.push(self.expr()?);
                    self.skip_newlines();
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                let close = self.expect(&Tok::RBrace, "`,` or `}`")?;
                Ok(Expr { kind: ExprKind::Concat(parts), span: start.to(close) })
            }
            Tok::Ident(_) => {
                let base = self.ident("a signal name")?;
                if self.at(&Tok::LParen) {
                    self.diags.push(
                        Diagnostic::error(base.span, "gates can't be nested inside other gates")
                            .label("give this gate its own line")
                            .note("one line = one box on the canvas"),
                    );
                    return Err(());
                }
                let port = if self.eat(&Tok::Dot) { Some(self.ident("a port name")?) } else { None };
                let select = if self.at(&Tok::LBracket) { Some(self.select()?) } else { None };
                let span = start.to(self.prev_span());
                Ok(Expr { kind: ExprKind::Ref { base, port, select }, span })
            }
            _ => {
                self.error_here("a signal, a number or `{`");
                Err(())
            }
        }
    }

    fn select(&mut self) -> PResult<Select> {
        let open = self.bump().span;
        let (hi, _) = self.number("a bit number")?;
        let lo = if self.eat(&Tok::Colon) { self.number("a bit number")?.0 } else { hi };
        let close = self.expect(&Tok::RBracket, "`]`")?;
        let span = open.to(close);
        if hi < lo {
            self.diags
                .push(Diagnostic::error(span, "bit ranges go from high to low").label(format!("try `[{lo}:{hi}]`")));
            return Err(());
        }
        Ok(Select { hi: hi as usize, lo: lo as usize, span })
    }

    fn test(&mut self) -> PResult<TestDef> {
        let keyword = self.bump().span;
        let chip = self.block_header("the name of the chip to test")?;
        let mut test = TestDef { chip, header: None, steps: Vec::new(), span: keyword };
        loop {
            self.skip_newlines();
            match self.peek() {
                Tok::RBrace => {
                    test.span = keyword.to(self.bump().span);
                    return Ok(test);
                }
                Tok::Eof => {
                    self.diags.push(
                        Diagnostic::error(test.chip.span, "this test is missing its closing `}`").label("opened here"),
                    );
                    return Ok(test);
                }
                _ => {}
            }
            let start = self.pos;
            while !matches!(self.peek(), Tok::Newline | Tok::RBrace | Tok::Eof) {
                self.bump();
            }
            let line = self.toks[start..self.pos].to_vec();
            self.test_line(&mut test, &line);
        }
    }

    /// A test line holds one row or step, or several steps joined by `;`.
    fn test_line(&mut self, test: &mut TestDef, line: &[Token]) {
        for part in line.split(|t| t.tok == Tok::Semi).filter(|p| !p.is_empty()) {
            match &part[0].tok {
                Tok::Ident(word) if matches!(word.as_str(), "set" | "tick" | "expect") => {
                    let word = word.clone();
                    if let Some(step) = self.test_step(&word, part) {
                        test.steps.push(step);
                    }
                }
                _ => self.test_row(test, part),
            }
        }
    }

    fn test_step(&mut self, word: &str, part: &[Token]) -> Option<TestStep> {
        let span = part[0].span.to(part[part.len() - 1].span);
        if word == "tick" {
            let count = match part {
                [_] => 1,
                [_, Token { tok: Tok::Number { value, .. }, .. }] if (1..=1_000_000).contains(value) => *value,
                _ => {
                    self.diags.push(
                        Diagnostic::error(span, "`tick` takes an optional count")
                            .note("like `tick` or `tick 10` (up to a million)"),
                    );
                    return None;
                }
            };
            return Some(TestStep::Tick { count, span });
        }
        // `set a = 1, b = 0` / `expect y = 0x2A`
        let mut values = Vec::new();
        let mut rest = &part[1..];
        loop {
            match rest {
                [Token { tok: Tok::Ident(name), span: name_span }, Token { tok: Tok::Eq, .. }, value, tail @ ..] => {
                    let ident = Ident { name: name.clone(), span: *name_span };
                    values.push((ident, self.test_value(value)?));
                    match tail {
                        [] => break,
                        [Token { tok: Tok::Comma, .. }, more @ ..] if !more.is_empty() => rest = more,
                        _ => {
                            self.diags.push(Diagnostic::error(tail[0].span, "expected `,` or the end of the step"));
                            return None;
                        }
                    }
                }
                _ => {
                    let at = rest.first().map_or(span, |t| t.span);
                    self.diags.push(
                        Diagnostic::error(at, format!("expected `name = value` after `{word}`"))
                            .note(format!("like `{word} a = 1, b = 0x2A`")),
                    );
                    return None;
                }
            }
        }
        Some(if word == "set" { TestStep::Set { values, span } } else { TestStep::Expect { values, span } })
    }

    fn test_value(&mut self, t: &Token) -> Option<TestValue> {
        let kind = match &t.tok {
            Tok::Number { value, digits_width } => {
                TestValueKind::Number(Literal { value: *value, digits_width: *digits_width })
            }
            Tok::Ident(name) if name == "x" || name == "X" => TestValueKind::X,
            Tok::Ident(name) if name == "z" || name == "Z" => TestValueKind::Z,
            Tok::Star => TestValueKind::Any,
            other => {
                self.diags.push(Diagnostic::error(
                    t.span,
                    format!("expected a value (a number, X, Z or *), found {}", other.describe()),
                ));
                return None;
            }
        };
        Some(TestValue { kind, span: t.span })
    }

    fn test_row(&mut self, test: &mut TestDef, line: &[Token]) {
        let span = line[0].span.to(line[line.len() - 1].span);
        let bars: Vec<usize> = (0..line.len()).filter(|&i| line[i].tok == Tok::Pipe).collect();
        let &[bar] = bars.as_slice() else {
            let message = if bars.is_empty() {
                "a test row needs a `|` between the inputs and the outputs (or start the line with set, tick or expect)"
            } else {
                "a test row can only have one `|`"
            };
            self.diags.push(Diagnostic::error(span, message));
            return;
        };
        let (inputs, outputs) = (&line[..bar], &line[bar + 1..]);

        let is_value_word = |name: &str| matches!(name, "x" | "X" | "z" | "Z");
        let is_header = test.header.is_none()
            && !test.steps.iter().any(|s| matches!(s, TestStep::Row(_)))
            && line.iter().all(|t| matches!(t.tok, Tok::Ident(_) | Tok::Pipe))
            && line.iter().any(|t| matches!(&t.tok, Tok::Ident(n) if !is_value_word(n)));
        if is_header {
            let names = |tokens: &[Token]| {
                tokens
                    .iter()
                    .filter_map(|t| match &t.tok {
                        Tok::Ident(name) => Some(Ident { name: name.clone(), span: t.span }),
                        _ => None,
                    })
                    .collect()
            };
            test.header = Some(TestHeader { inputs: names(inputs), outputs: names(outputs) });
            return;
        }

        let inputs = inputs.iter().filter_map(|t| self.test_value(t)).collect();
        let outputs = outputs.iter().filter_map(|t| self.test_value(t)).collect();
        test.steps.push(TestStep::Row(TestRow { inputs, outputs, span }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(text: &str) -> File {
        let mut diags = Vec::new();
        let file = parse(0, text, &mut diags);
        assert!(diags.is_empty(), "{diags:#?}");
        file
    }

    fn parse_errors(text: &str) -> Vec<String> {
        let mut diags = Vec::new();
        parse(0, text, &mut diags);
        diags.into_iter().map(|d| d.message).collect()
    }

    #[test]
    fn parses_a_chip_and_its_test() {
        let file = parse_ok(
            "chip HalfAdder {\n  in a, b\n  out sum, carry\n  sum = Xor(a, b)\n  carry = And(a: a, b: b)\n}\n\
             test HalfAdder {\n  // a b | sum carry\n  0 0 | 0 0\n  1 1 | 0 1\n}\n",
        );
        let chip = &file.chips[0];
        assert_eq!(chip.name.name, "HalfAdder");
        assert_eq!(chip.inputs.len(), 2);
        assert_eq!(chip.outputs.len(), 2);
        assert_eq!(chip.stmts.len(), 2);
        let StmtValue::Call(call) = &chip.stmts[1].value else { panic!() };
        assert_eq!(call.args[1].name.as_ref().unwrap().name, "b");
        assert_eq!(file.tests[0].steps.len(), 2);
    }

    #[test]
    fn port_named_out_is_allowed() {
        let file = parse_ok("chip Xor {\n  in a, b\n  out out\n  out = Nand(a, b)\n}");
        assert_eq!(file.chips[0].outputs[0].name.name, "out");
        assert_eq!(file.chips[0].stmts[0].target.name, "out");
    }

    #[test]
    fn parses_buses_and_header() {
        let file = parse_ok(
            "chip A {\n  in a[4]\n  out y[4], hi\n  y = {a[1:0], a[3:2]}\n  hi = a[3]\n}\n\
             test A {\n  a | y hi\n  0x5 | 0x5 0\n  0xF | * 1\n}",
        );
        assert_eq!(file.chips[0].inputs[0].width, 4);
        let test = &file.tests[0];
        assert_eq!(test.header.as_ref().unwrap().outputs.len(), 2);
        let TestStep::Row(row) = &test.steps[1] else { panic!("a row") };
        assert!(matches!(row.outputs[0].kind, TestValueKind::Any));
    }

    #[test]
    fn parses_test_steps() {
        let file =
            parse_ok("chip A {\n in r\n out y\n y = Not(r)\n}\ntest A {\n set r = 1\n tick; expect y = 0\n tick 5\n}");
        let steps = &file.tests[0].steps;
        assert_eq!(steps.len(), 4);
        assert!(matches!(steps[1], TestStep::Tick { count: 1, .. }));
        assert!(matches!(&steps[2], TestStep::Expect { values, .. } if values[0].0.name == "y"));
        assert!(matches!(steps[3], TestStep::Tick { count: 5, .. }));
    }

    #[test]
    fn friendly_errors() {
        let errors = parse_errors("chip A {\n  in a\n  out y\n  y = And(Not(a), a)\n  y Or(a, a)\n}");
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("nested"));
        assert!(errors[1].contains("expected `=` or `<-`, found `Or`"), "{errors:?}");
    }
}
