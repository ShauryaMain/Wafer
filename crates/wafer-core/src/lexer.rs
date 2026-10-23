//! Splits `.chip` source text into tokens.

use crate::diag::{Diagnostic, FileId, Span};

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    /// `digits_width` is the width implied by the digits of a hex or binary
    /// literal (`0x0F` is 8 bits); decimal literals have none.
    Number {
        value: u64,
        digits_width: Option<usize>,
    },
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Semi,
    Dot,
    Eq,
    /// `<-`, reserved for shared buses.
    Arrow,
    Pipe,
    Star,
    Newline,
    Eof,
}

impl Tok {
    /// How the token reads in an error message.
    pub fn describe(&self) -> String {
        match self {
            Tok::Ident(name) => format!("`{name}`"),
            Tok::Number { .. } => "a number".into(),
            Tok::Newline => "the end of the line".into(),
            Tok::Eof => "the end of the file".into(),
            punct => format!("`{}`", punct.symbol()),
        }
    }

    fn symbol(&self) -> &'static str {
        match self {
            Tok::LBrace => "{",
            Tok::RBrace => "}",
            Tok::LParen => "(",
            Tok::RParen => ")",
            Tok::LBracket => "[",
            Tok::RBracket => "]",
            Tok::Comma => ",",
            Tok::Colon => ":",
            Tok::Semi => ";",
            Tok::Dot => ".",
            Tok::Eq => "=",
            Tok::Arrow => "<-",
            Tok::Pipe => "|",
            Tok::Star => "*",
            Tok::Ident(_) | Tok::Number { .. } | Tok::Newline | Tok::Eof => "",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

pub fn lex(file: FileId, text: &str, diags: &mut Vec<Diagnostic>) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let single = match bytes[i] {
            b'{' => Some(Tok::LBrace),
            b'}' => Some(Tok::RBrace),
            b'(' => Some(Tok::LParen),
            b')' => Some(Tok::RParen),
            b'[' => Some(Tok::LBracket),
            b']' => Some(Tok::RBracket),
            b',' => Some(Tok::Comma),
            b':' => Some(Tok::Colon),
            b';' => Some(Tok::Semi),
            b'.' => Some(Tok::Dot),
            b'=' => Some(Tok::Eq),
            b'|' => Some(Tok::Pipe),
            b'*' => Some(Tok::Star),
            b'\n' => Some(Tok::Newline),
            _ => None,
        };
        if let Some(tok) = single {
            i += 1;
            tokens.push(Token { tok, span: Span::new(file, start, i) });
            continue;
        }
        match bytes[i] {
            b' ' | b'\t' | b'\r' => i += 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'<' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                tokens.push(Token { tok: Tok::Arrow, span: Span::new(file, start, i) });
            }
            b'0'..=b'9' => {
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let span = Span::new(file, start, i);
                match parse_number(&text[start..i]) {
                    Ok((value, digits_width)) => tokens.push(Token { tok: Tok::Number { value, digits_width }, span }),
                    Err(message) => diags.push(Diagnostic::error(span, message)),
                }
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                tokens.push(Token { tok: Tok::Ident(text[start..i].to_string()), span: Span::new(file, start, i) });
            }
            _ => {
                let ch = text[i..].chars().next().unwrap_or('?');
                i += ch.len_utf8();
                diags.push(Diagnostic::error(Span::new(file, start, i), format!("unexpected character `{ch}`")));
            }
        }
    }
    tokens.push(Token { tok: Tok::Eof, span: Span::new(file, text.len(), text.len()) });
    tokens
}

/// Parses `42`, `0x2A` or `0b101010` (underscores allowed between digits).
fn parse_number(text: &str) -> Result<(u64, Option<usize>), String> {
    let lower = text.to_ascii_lowercase();
    let (digits, radix, bits_per_digit) = if let Some(rest) = lower.strip_prefix("0x") {
        (rest, 16, Some(4))
    } else if let Some(rest) = lower.strip_prefix("0b") {
        (rest, 2, Some(1))
    } else {
        (lower.as_str(), 10, None)
    };
    let digits: String = digits.chars().filter(|&c| c != '_').collect();
    if digits.is_empty() {
        return Err(format!("`{text}` is missing its digits"));
    }
    let value = u64::from_str_radix(&digits, radix).map_err(|e| match e.kind() {
        std::num::IntErrorKind::PosOverflow => format!("`{text}` is too big (the limit is 64 bits)"),
        _ => format!("`{text}` isn't a valid number"),
    })?;
    Ok((value, bits_per_digit.map(|bits| bits * digits.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(text: &str) -> Vec<Tok> {
        let mut diags = Vec::new();
        let out = lex(0, text, &mut diags).into_iter().map(|t| t.tok).collect();
        assert!(diags.is_empty(), "{diags:?}");
        out
    }

    #[test]
    fn lexes_a_statement() {
        assert_eq!(
            toks("sum = Xor(a, b) // comment"),
            vec![
                Tok::Ident("sum".into()),
                Tok::Eq,
                Tok::Ident("Xor".into()),
                Tok::LParen,
                Tok::Ident("a".into()),
                Tok::Comma,
                Tok::Ident("b".into()),
                Tok::RParen,
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn number_widths_follow_digits() {
        assert_eq!(toks("0x0F")[0], Tok::Number { value: 15, digits_width: Some(8) });
        assert_eq!(toks("0b0010")[0], Tok::Number { value: 2, digits_width: Some(4) });
        assert_eq!(toks("12")[0], Tok::Number { value: 12, digits_width: None });
    }
}
