//! Suggestions for the code editor: what could go at the cursor, and the
//! shape of the call it's inside.

use crate::check::{Callee, ChipInfo, Design, Name};
use crate::gate::GateKind;
use crate::lexer::{Tok, lex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub label: String,
    /// Replaces the word being typed.
    pub insert: String,
    /// A short description, like `gate · a, b → out`.
    pub detail: String,
    /// After inserting, move the cursor back this many characters (to land
    /// between the parentheses of `Xor()`).
    pub cursor_back: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Completions {
    /// Byte offset where the word being completed starts.
    pub start: usize,
    pub suggestions: Vec<Suggestion>,
    /// The call the cursor is inside, like `FullAdder(a, b, cin) → sum, cout`.
    pub signature: Option<String>,
}

/// What's in scope right before the cursor.
struct Context {
    /// `chip` or `test`, and its name.
    block: Option<(String, String)>,
    /// Callee names of the calls the cursor is inside, innermost last.
    calls: Vec<Option<String>>,
    prev: Option<Tok>,
    before_prev: Option<Tok>,
}

pub fn complete(design: &Design, text: &str, cursor: usize) -> Completions {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let line_start = text[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let line = &text[line_start..cursor];
    let word_len = line.bytes().rev().take_while(|b| b.is_ascii_alphanumeric() || *b == b'_').count();
    let start = cursor - word_len;
    let word = &text[start..cursor];
    let mut result = Completions { start, ..Completions::default() };
    if line.contains("//") || word.starts_with(|c: char| c.is_ascii_digit()) {
        return result;
    }
    let ctx = context(&text[..start]);
    if let Some(Some(callee)) = ctx.calls.last() {
        result.signature = signature(design, callee);
    }

    let at_line_start = text[line_start..start].trim().is_empty();
    let declaring = matches!(text[line_start..start].split_whitespace().next(), Some("in" | "out" | "wire"))
        && !text[line_start..start].contains('=');
    let mut items: Vec<Suggestion> = Vec::new();
    match &ctx.block {
        None => {
            if at_line_start {
                items.push(keyword("chip", "a new chip"));
                items.push(keyword("test", "a truth table for a chip"));
            } else if matches!(&ctx.prev, Some(Tok::Ident(w)) if w == "test") {
                items.extend(design.chips.keys().map(|c| plain(c, "chip")));
            }
        }
        Some((kind, chip)) if kind == "test" => {
            if let Some(info) = design.chips.get(chip) {
                items.extend(info.inputs.iter().map(|p| plain(&p.name, &port_detail("input", p.width))));
                items.extend(info.outputs.iter().map(|p| plain(&p.name, &port_detail("output", p.width))));
            }
        }
        Some((_, chip)) => {
            let info = design.chips.get(chip);
            if matches!(ctx.prev, Some(Tok::Dot)) {
                if let (Some(info), Some(Tok::Ident(base))) = (info, &ctx.before_prev) {
                    items.extend(outputs_of(design, info, base));
                }
            } else if declaring {
                // Naming something new: nothing to suggest.
            } else if at_line_start {
                items.push(keyword("in", "declare inputs"));
                items.push(keyword("out", "declare outputs"));
                items.push(keyword("wire", "declare a wire"));
                if let Some(info) = info {
                    items.extend(info.outputs.iter().map(|p| plain(&p.name, &port_detail("output", p.width))));
                }
            } else if let (Some(Some(callee)), Some(Tok::LParen | Tok::Comma)) = (ctx.calls.last(), &ctx.prev) {
                if let Some(info) = info {
                    items.extend(signals(design, info));
                }
                items.extend(named_args(design, callee));
            } else if matches!(ctx.prev, Some(Tok::Eq | Tok::Arrow)) {
                items.extend(callees(design, chip));
                if let Some(info) = info {
                    items.extend(signals(design, info));
                }
            } else if let Some(info) = info {
                items.extend(signals(design, info));
            }
        }
    }

    // Starts-with matches first, then the rest that contain the word.
    let lower = word.to_lowercase();
    let mut ranked: Vec<(bool, Suggestion)> = items
        .into_iter()
        .filter(|s| s.label.to_lowercase().contains(&lower))
        .map(|s| (!s.label.to_lowercase().starts_with(&lower), s))
        .collect();
    ranked.sort_by_key(|r| r.0);
    let mut seen = std::collections::HashSet::new();
    result.suggestions = ranked.into_iter().map(|(_, s)| s).filter(|s| seen.insert(s.label.clone())).take(12).collect();
    if let [only] = result.suggestions.as_slice()
        && only.insert.trim_end() == word
    {
        result.suggestions.clear();
    }
    result
}

fn context(before: &str) -> Context {
    let toks = lex(0, before, &mut Vec::new());
    let mut ctx = Context { block: None, calls: Vec::new(), prev: None, before_prev: None };
    let mut depth = 0usize;
    let significant: Vec<&Tok> = toks.iter().map(|t| &t.tok).filter(|t| !matches!(t, Tok::Eof)).collect();
    for (i, tok) in significant.iter().enumerate() {
        match tok {
            Tok::Ident(word) if depth == 0 && (word == "chip" || word == "test") => {
                if let (Some(Tok::Ident(name)), Some(Tok::LBrace)) = (significant.get(i + 1), significant.get(i + 2)) {
                    ctx.block = Some((word.clone(), name.clone()));
                }
            }
            Tok::LBrace => depth += 1,
            Tok::RBrace => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    ctx.block = None;
                    ctx.calls.clear();
                }
            }
            Tok::LParen => {
                let callee = match i.checked_sub(1).map(|p| significant[p]) {
                    Some(Tok::Ident(name)) => Some(name.clone()),
                    _ => None,
                };
                ctx.calls.push(callee);
            }
            Tok::RParen => {
                ctx.calls.pop();
            }
            _ => {}
        }
    }
    let mut meaningful = significant.iter().rev().filter(|t| !matches!(t, Tok::Newline));
    ctx.prev = meaningful.next().map(|t| (*t).clone());
    ctx.before_prev = meaningful.next().map(|t| (*t).clone());
    ctx
}

fn keyword(word: &str, what: &str) -> Suggestion {
    Suggestion { label: word.into(), insert: format!("{word} "), detail: format!("keyword · {what}"), cursor_back: 0 }
}

fn plain(name: &str, detail: &str) -> Suggestion {
    Suggestion { label: name.into(), insert: name.into(), detail: detail.into(), cursor_back: 0 }
}

fn port_detail(kind: &str, width: usize) -> String {
    if width == 1 { kind.to_string() } else { format!("{kind} · {width} bits") }
}

fn ports(list: &[(String, usize)]) -> String {
    list.iter().map(|(n, w)| if *w > 1 { format!("{n}[{w}]") } else { n.clone() }).collect::<Vec<_>>().join(", ")
}

/// `FullAdder(a, b, cin) → sum, cout`
pub fn signature(design: &Design, callee: &str) -> Option<String> {
    if let Some(info) = design.chips.get(callee) {
        let sig = design.signature(&Callee::Chip(info.name.clone()));
        return Some(format!("{callee}({}) → {}", ports(&sig.inputs), ports(&sig.outputs)));
    }
    let gate = GateKind::from_name(callee)?;
    Some(format!("{callee}({}) → out", gate.args()))
}

fn callees(design: &Design, current: &str) -> Vec<Suggestion> {
    let mut out = Vec::new();
    for gate in GateKind::ALL {
        if !design.chips.contains_key(gate.name()) {
            let detail = format!("gate · {} → out", gate.args());
            out.push(Suggestion {
                label: gate.name().into(),
                insert: format!("{}()", gate.name()),
                detail,
                cursor_back: 1,
            });
        }
    }
    for name in design.chips.keys().filter(|c| c.as_str() != current) {
        let sig = design.signature(&Callee::Chip(name.clone()));
        let detail = format!("chip · {} → {}", ports(&sig.inputs), ports(&sig.outputs));
        out.push(Suggestion { label: name.clone(), insert: format!("{name}()"), detail, cursor_back: 1 });
    }
    out
}

fn named_args(design: &Design, callee: &str) -> Vec<Suggestion> {
    let inputs: Vec<(String, usize)> = if let Some(info) = design.chips.get(callee) {
        info.inputs.iter().map(|p| (p.name.clone(), p.width)).collect()
    } else if let Some(gate) = GateKind::from_name(callee) {
        gate.inputs().iter().map(|n| (n.to_string(), 0)).collect()
    } else {
        Vec::new()
    };
    inputs
        .into_iter()
        .map(|(name, width)| Suggestion {
            label: format!("{name}:"),
            insert: format!("{name}: "),
            detail: if width > 1 { format!("{callee} input · {width} bits") } else { format!("{callee} input") },
            cursor_back: 0,
        })
        .collect()
}

fn signals(design: &Design, info: &ChipInfo) -> Vec<Suggestion> {
    let mut names: Vec<(&String, &Name)> = info.names.iter().collect();
    names.sort_by_key(|(name, _)| name.as_str());
    names
        .into_iter()
        .map(|(name, kind)| {
            let detail = match *kind {
                Name::Input(i) => port_detail("input", info.inputs[i].width),
                Name::Output(i) => port_detail("output", info.outputs[i].width),
                Name::Wire(i) => port_detail("wire", info.wires[i].width),
                Name::Instance(k) => match &info.instances[k].callee {
                    Callee::Gate { kind, .. } => format!("{} gate", kind.name()),
                    Callee::Chip(chip) => {
                        let outs = design.signature(&info.instances[k].callee).outputs;
                        if outs.len() > 1 {
                            format!("{chip} chip · use .{}", outs[0].0)
                        } else {
                            format!("{chip} chip")
                        }
                    }
                    Callee::Unknown => "box".into(),
                },
            };
            plain(name, &detail)
        })
        .collect()
}

fn outputs_of(design: &Design, info: &ChipInfo, base: &str) -> Vec<Suggestion> {
    let Some(Name::Instance(k)) = info.names.get(base) else { return Vec::new() };
    let callee = &info.instances[*k].callee;
    design.signature(callee).outputs.iter().map(|(name, width)| plain(name, &port_detail("output", *width))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    fn design() -> Design {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../chips");
        Design::check(&Project::load(&dir).unwrap())
    }

    /// Completes at the `|` in `text`.
    fn at(design: &Design, text: &str) -> Completions {
        let cursor = text.find('|').unwrap();
        complete(design, &text.replace('|', ""), cursor)
    }

    fn labels(c: &Completions) -> Vec<&str> {
        c.suggestions.iter().map(|s| s.label.as_str()).collect()
    }

    const BODY: &str = "chip Add4 {\n  in  a[4], b[4], cin\n  out sum[4], cout\n  fa0 = FullAdder(a[0], b[0], cin)\n";

    #[test]
    fn suggests_chips_after_equals() {
        let d = design();
        let c = at(&d, &format!("{BODY}  x = Full|\n}}"));
        assert_eq!(labels(&c), ["FullAdder"]);
        assert_eq!(c.suggestions[0].insert, "FullAdder()");
        assert_eq!(c.suggestions[0].cursor_back, 1);
        assert!(c.suggestions[0].detail.contains("a, b, cin → sum, cout"));
    }

    #[test]
    fn suggests_outputs_after_a_dot() {
        let d = design();
        let c = at(&d, &format!("{BODY}  y = fa0.|\n}}"));
        assert_eq!(labels(&c), ["sum", "cout"]);
    }

    #[test]
    fn suggests_signals_and_named_inputs_inside_calls() {
        let d = design();
        let c = at(&d, &format!("{BODY}  fa1 = FullAdder(a[1], b[1], c|\n}}"));
        assert_eq!(c.signature.as_deref(), Some("FullAdder(a, b, cin) → sum, cout"));
        assert!(labels(&c).contains(&"cin"), "{:?}", labels(&c));
        assert!(labels(&c).contains(&"cin:"), "{:?}", labels(&c));
    }

    #[test]
    fn suggests_keywords_at_line_starts_and_nothing_in_comments() {
        let d = design();
        assert!(labels(&at(&d, "ch|")).contains(&"chip"));
        assert!(labels(&at(&d, &format!("{BODY}  w|\n}}"))).contains(&"wire"));
        assert!(at(&d, &format!("{BODY}  // fa|\n}}")).suggestions.is_empty());
        assert!(at(&d, "chip Add4 {\n  in  ca|").suggestions.is_empty(), "naming a new input");
    }
}
