//! Changes to `.chip` text made from the canvas. Each edit rewrites only the
//! lines it touches, so comments, blank lines and formatting elsewhere stay
//! exactly as they were.

use crate::ast::{self, Expr, ExprKind, StmtValue};
use crate::check::{ChipInfo, Design, Name};
use crate::diag::FileId;
use crate::lexer::{Tok, lex};
use crate::project::Project;

/// The input end of a wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sink {
    /// An input pin of a gate or chip box.
    Instance { name: String, port: String },
    /// One of the chip's own outputs.
    Output(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortDir {
    In,
    Out,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    /// Adds `name = Callee()` at the end of the chip.
    AddInstance {
        name: String,
        callee: String,
    },
    /// Removes a box and disconnects everything it fed.
    RemoveInstance {
        name: String,
    },
    /// Wires `source` (as you'd write it: `a`, `n1`, `h1.sum`) to a pin.
    Connect {
        sink: Sink,
        source: String,
    },
    Disconnect {
        sink: Sink,
    },
    AddPort {
        dir: PortDir,
        name: String,
        width: usize,
    },
    /// Removes a port and disconnects everything it fed.
    RemovePort {
        dir: PortDir,
        name: String,
    },
    /// Renames a port, wire or box everywhere inside the chip.
    Rename {
        old: String,
        new: String,
    },
}

/// The new text for the file that holds the chip.
pub struct EditResult {
    pub file: FileId,
    pub text: String,
    /// Boxes the edit had to rename, as (old, new). Happens when a box
    /// named after an output lets go of it.
    pub renamed: Vec<(String, String)>,
}

pub fn apply(project: &Project, design: &Design, chip: &str, edit: &Edit) -> Result<EditResult, String> {
    let (file, def) = project
        .files
        .iter()
        .find_map(|(id, f)| f.chips.iter().find(|c| c.name.name == chip).map(|c| (*id, c)))
        .ok_or_else(|| format!("there's no chip named `{chip}`"))?;
    let info = design.chips.get(chip).ok_or_else(|| format!("`{chip}` couldn't be checked"))?;
    let text = project.sources.get(file).text.as_str();
    let mut editor =
        Editor { text, def, info, design, project, changes: Vec::new(), appended: Vec::new(), renamed: Vec::new() };
    match edit {
        Edit::AddInstance { name, callee } => {
            editor.check_new_name(name)?;
            editor.appended.push(format!("{name} = {callee}()"));
        }
        Edit::RemoveInstance { name } => editor.remove_instance(name)?,
        Edit::Connect { sink, source } => editor.connect(sink, Some(source))?,
        Edit::Disconnect { sink } => editor.connect(sink, None)?,
        Edit::AddPort { dir, name, width } => editor.add_port(*dir, name, *width)?,
        Edit::RemovePort { dir, name } => editor.remove_port(*dir, name)?,
        Edit::Rename { old, new } => editor.rename(old, new)?,
    }
    let renamed = editor.renamed.clone();
    Ok(EditResult { file, text: editor.finish()?, renamed })
}

/// What [`extract_chip`] produces.
pub struct Extracted {
    /// The file holding the parent chip, and its new text.
    pub parent_file: FileId,
    pub parent_text: String,
    /// The new chip's file text.
    pub new_text: String,
    /// The box that stands in for the selection in the parent.
    pub instance: String,
}

/// "Make Chip": moves the `selected` boxes of `chip` into a new chip named
/// `new_name`, and puts one box of the new chip in their place.
///
/// Every outside signal the boxes read becomes an input of the new chip.
/// Every box output that's still used outside (or that drove one of the
/// parent's outputs) becomes an output. The parent then reads those
/// through the new box, so the circuit behaves exactly as before.
pub fn extract_chip(
    project: &Project,
    design: &Design,
    chip: &str,
    selected: &[String],
    new_name: &str,
) -> Result<Extracted, String> {
    if !is_valid_name(new_name) {
        return Err(format!("`{new_name}` isn't a valid chip name (use letters, digits and _)"));
    }
    if design.chips.contains_key(new_name) || crate::gate::GateKind::from_name(new_name).is_some() {
        return Err(format!("there's already a chip named `{new_name}`"));
    }
    let (file, def) = project
        .files
        .iter()
        .find_map(|(id, f)| f.chips.iter().find(|c| c.name.name == chip).map(|c| (*id, c)))
        .ok_or_else(|| format!("there's no chip named `{chip}`"))?;
    let info = design.chips.get(chip).ok_or_else(|| format!("`{chip}` couldn't be checked"))?;
    let text = project.sources.get(file).text.as_str();

    let mut picked = Vec::new();
    for name in selected {
        let k = info
            .instances
            .iter()
            .position(|i| &i.name == name)
            .ok_or_else(|| format!("`{name}` isn't a box in `{chip}`"))?;
        if matches!(info.instances[k].callee, crate::check::Callee::Unknown) {
            return Err(format!("fix the chip name on `{name}`'s line first"));
        }
        picked.push(k);
    }
    if picked.is_empty() {
        return Err("select the boxes to put in the new chip first".into());
    }
    let inside: std::collections::HashSet<&str> = picked.iter().map(|&k| info.instances[k].name.as_str()).collect();
    let stmt_of = |name: &str| info.instances.iter().find(|i| i.name == name).and_then(|i| def.stmts.get(i.line));

    // Outside signals the boxes read become inputs: (as written outside, port name, width).
    let mut inputs: Vec<(String, String, usize)> = Vec::new();
    for &k in &picked {
        for expr in info.instances[k].inputs.iter().flatten() {
            for (base, port) in refs(expr) {
                if inside.contains(base.name.as_str()) {
                    continue;
                }
                let outer = match port {
                    Some(p) => format!("{}.{}", base.name, p.name),
                    None => base.name.clone(),
                };
                if inputs.iter().any(|(o, _, _)| *o == outer) {
                    continue;
                }
                let name = outer.replace('.', "_");
                let width = ref_width(design, info, &base.name, port.map(|p| p.name.as_str()))?;
                inputs.push((outer, name, width));
            }
        }
    }

    // Which box outputs are read outside the selection.
    let mut read_outside: std::collections::HashSet<(String, Option<String>)> = std::collections::HashSet::new();
    let mut note_reads = |expr: &Expr| {
        for (base, port) in refs(expr) {
            read_outside.insert((base.name.clone(), port.map(|p| p.name.clone())));
        }
    };
    for (k, inst) in info.instances.iter().enumerate() {
        if !picked.contains(&k) {
            inst.inputs.iter().flatten().for_each(&mut note_reads);
        }
    }
    info.assigns.iter().for_each(|a| note_reads(&a.value));

    // Outputs: (port name in the new chip, how it's driven inside, width,
    // the parent output it drove directly, the box and output it comes from).
    struct Out {
        port: String,
        width: usize,
        drove: Option<String>,
        from: (String, Option<String>),
    }
    let mut outputs: Vec<Out> = Vec::new();
    for &k in &picked {
        let inst = &info.instances[k];
        let sig = design.signature(&inst.callee);
        let multi = sig.outputs.len() > 1;
        for (o, width) in &sig.outputs {
            let read = if multi {
                read_outside.contains(&(inst.name.clone(), Some(o.clone())))
            } else {
                read_outside.contains(&(inst.name.clone(), None))
                    || read_outside.contains(&(inst.name.clone(), Some(o.clone())))
            };
            let drove = match inst.drives {
                Some(Name::Output(i)) => Some(info.outputs[i].name.clone()),
                Some(Name::Wire(i)) => Some(info.wires[i].name.clone()),
                _ => None,
            };
            if read || drove.is_some() {
                let port = if multi { format!("{}_{o}", inst.name) } else { inst.name.clone() };
                outputs.push(Out { port, width: *width, drove, from: (inst.name.clone(), multi.then(|| o.clone())) });
            }
        }
    }
    let taken: Vec<&str> =
        inputs.iter().map(|(_, n, _)| n.as_str()).chain(outputs.iter().map(|o| o.port.as_str())).collect();
    if let Some(dup) = taken.iter().enumerate().find(|(i, n)| taken[..*i].contains(n)).map(|(_, n)| n) {
        return Err(format!("two connections would both be called `{dup}`; rename one first"));
    }

    // The new chip: the selected lines, with outside `box.port` reads renamed to their input ports.
    let decl = |name: &str, width: usize| if width == 1 { name.to_string() } else { format!("{name}[{width}]") };
    let mut body = String::new();
    if !inputs.is_empty() {
        body += &format!("  in  {}\n", inputs.iter().map(|(_, n, w)| decl(n, *w)).collect::<Vec<_>>().join(", "));
    }
    if !outputs.is_empty() {
        body += &format!("  out {}\n", outputs.iter().map(|o| decl(&o.port, o.width)).collect::<Vec<_>>().join(", "));
    }
    body.push('\n');
    for &k in &picked {
        let stmt = stmt_of(&info.instances[k].name).ok_or("a selected box has no line of its own")?;
        let mut line = text[stmt.span.start..stmt.span.end].to_string();
        let mut renames: Vec<(usize, usize, String)> = Vec::new();
        if let StmtValue::Call(call) = &stmt.value {
            for arg in &call.args {
                for (base, port) in refs(&arg.value) {
                    if let (Some(p), false) = (port, inside.contains(base.name.as_str())) {
                        renames.push((base.span.start, p.span.end, format!("{}_{}", base.name, p.name)));
                    }
                }
            }
        }
        renames.sort_by_key(|r| std::cmp::Reverse(r.0));
        for (start, end, new) in renames {
            line.replace_range(start - stmt.span.start..end - stmt.span.start, &new);
        }
        body += &format!("  {line}\n");
    }
    for out in &outputs {
        if let (inst, Some(o)) = &out.from {
            body += &format!("  {} = {inst}.{o}\n", out.port);
        }
    }
    let raw = format!("chip {new_name} {{\n{body}}}\n");
    let new_text = crate::format::format(&raw).unwrap_or(raw);

    // The parent: drop the selected lines, add the new box, and read its outputs.
    let instance = fresh_name(info, new_name);
    let through = |port: &str| if outputs.len() == 1 { instance.clone() } else { format!("{instance}.{port}") };
    let mut ed =
        Editor { text, def, info, design, project, changes: Vec::new(), appended: Vec::new(), renamed: Vec::new() };
    for &k in &picked {
        if let Some(stmt) = stmt_of(&info.instances[k].name) {
            ed.remove_stmt(stmt);
        }
    }
    let moved_lines: Vec<usize> = picked.iter().map(|&k| info.instances[k].line).collect();
    for (line, stmt) in def.stmts.iter().enumerate() {
        if moved_lines.contains(&line) {
            continue;
        }
        let exprs: Vec<&Expr> = match &stmt.value {
            StmtValue::Call(call) => call.args.iter().map(|a| &a.value).collect(),
            StmtValue::Expr(e) => vec![e],
        };
        for expr in exprs {
            for (base, port) in refs(expr) {
                // Names of boxes that drove an output mean the output, which stays.
                if !inside.contains(base.name.as_str())
                    || !matches!(info.names.get(&base.name), Some(Name::Instance(_)))
                {
                    continue;
                }
                let wanted = (base.name.clone(), port.map(|p| p.name.clone()));
                let found = outputs.iter().find(|o| o.from == wanted || (o.from.1.is_none() && o.from.0 == base.name));
                if let Some(out) = found {
                    let end = port.map_or(base.span.end, |p| p.span.end);
                    ed.changes.push((base.span.start, end, through(&out.port)));
                }
            }
        }
    }
    ed.appended.push(format!(
        "{instance} = {new_name}({})",
        inputs.iter().map(|(o, _, _)| o.as_str()).collect::<Vec<_>>().join(", ")
    ));
    for out in &outputs {
        if let Some(target) = &out.drove {
            ed.appended.push(format!("{target} = {}", through(&out.port)));
        }
    }
    let parent_text = ed.finish()?;
    Ok(Extracted { parent_file: file, parent_text, new_text, instance })
}

/// Every `name` or `name.port` an expression reads.
fn refs(expr: &Expr) -> Vec<(&ast::Ident, Option<&ast::Ident>)> {
    match &expr.kind {
        ExprKind::Ref { base, port, .. } => vec![(base, port.as_ref())],
        ExprKind::Literal(_) | ExprKind::Str(_) => Vec::new(),
        ExprKind::Concat(parts) => parts.iter().flat_map(refs).collect(),
    }
}

fn ref_width(design: &Design, info: &ChipInfo, base: &str, port: Option<&str>) -> Result<usize, String> {
    let unknown = || format!("can't work out how wide `{base}` is");
    match info.names.get(base).ok_or_else(unknown)? {
        Name::Input(i) => Ok(info.inputs[*i].width),
        Name::Output(i) => Ok(info.outputs[*i].width),
        Name::Wire(i) => Ok(info.wires[*i].width),
        Name::Instance(k) => {
            let sig = design.signature(&info.instances[*k].callee);
            let out = match port {
                Some(p) => sig.outputs.iter().find(|(n, _)| n == p),
                None => sig.outputs.first(),
            };
            out.map(|(_, w)| *w).ok_or_else(unknown)
        }
    }
}

/// The first unused name like `nand1`, `nand2`, ... (or `add4_1` when the
/// name already ends in a digit).
pub fn fresh_name(info: &ChipInfo, base: &str) -> String {
    let base = base.to_lowercase();
    let sep = if base.ends_with(|c: char| c.is_ascii_digit()) { "_" } else { "" };
    (1..).map(|n| format!("{base}{sep}{n}")).find(|n| !info.names.contains_key(n)).expect("names run out eventually")
}

pub fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !matches!(name, "chip" | "test" | "in" | "wire")
}

struct Editor<'a> {
    text: &'a str,
    def: &'a ast::ChipDef,
    info: &'a ChipInfo,
    design: &'a Design,
    project: &'a Project,
    /// Byte ranges to replace: (start, end, replacement).
    changes: Vec<(usize, usize, String)>,
    /// Lines to add at the end of the chip body.
    appended: Vec<String>,
    renamed: Vec<(String, String)>,
}

impl<'a> Editor<'a> {
    fn check_new_name(&self, name: &str) -> Result<(), String> {
        if !is_valid_name(name) {
            return Err(format!("`{name}` isn't a valid name (use letters, digits and _)"));
        }
        if self.info.names.contains_key(name) {
            return Err(format!("`{name}` is already used in `{}`", self.info.name));
        }
        Ok(())
    }

    fn slice(&self, expr: &Expr) -> &str {
        &self.text[expr.span.start..expr.span.end]
    }

    /// The line a box is written on. (A shared-wire driver's line is named
    /// after the wire, not the box, so this goes through the checker.)
    fn call_stmt(&self, name: &str) -> Option<(&'a ast::Stmt, &'a ast::Call)> {
        let def: &'a ast::ChipDef = self.def;
        let inst = self.info.instances.iter().find(|i| i.name == name)?;
        match def.stmts.get(inst.line) {
            Some(stmt @ ast::Stmt { value: StmtValue::Call(call), .. }) => Some((stmt, call)),
            _ => None,
        }
    }

    /// The callee's input ports and what's wired to each, as text.
    fn call_args(&self, name: &str) -> Result<(Vec<String>, Vec<Option<String>>), String> {
        let inst = self
            .info
            .instances
            .iter()
            .find(|i| i.name == name)
            .ok_or_else(|| format!("there's no box named `{name}`"))?;
        let ports: Vec<String> = self.design.signature(&inst.callee).inputs.into_iter().map(|(n, _)| n).collect();
        if ports.is_empty() && !inst.inputs.is_empty() || matches!(inst.callee, crate::check::Callee::Unknown) {
            return Err(format!("fix the chip name on `{name}`'s line first"));
        }
        let args = inst.inputs.iter().map(|e| e.as_ref().map(|e| self.slice(e).to_string())).collect();
        Ok((ports, args))
    }

    /// Rewrites a call's arguments: unnamed for the leading connected pins,
    /// named for the rest, so `Nand(b: x)` and `Nand(a, x)` both come out
    /// naturally.
    fn rewrite_call(&mut self, name: &str, ports: &[String], args: &[Option<String>]) {
        let Some((_, call)) = self.call_stmt(name) else { return };
        let leading = args.iter().take_while(|a| a.is_some()).count();
        let mut parts = Vec::new();
        for (i, (port, arg)) in ports.iter().zip(args).enumerate() {
            if let Some(arg) = arg {
                parts.push(if i < leading { arg.clone() } else { format!("{port}: {arg}") });
            }
        }
        // Text settings like a Rom's `file: "pong.asm"` aren't wires; keep them.
        for arg in call.args.iter().filter(|a| matches!(a.value.kind, ExprKind::Str(_))) {
            let start = arg.name.as_ref().map_or(arg.value.span.start, |n| n.span.start);
            parts.push(self.text[start..arg.value.span.end].to_string());
        }
        let new = format!("{}({})", call.chip.name, parts.join(", "));
        self.changes.push((call.span.start, call.span.end, new));
    }

    /// The byte range of a statement's whole line, if it's alone on it
    /// (a trailing comment goes with it). Otherwise just the statement.
    fn stmt_removal(&self, start: usize, end: usize) -> (usize, usize) {
        let line_start = self.text[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = self.text[end..].find('\n').map_or(self.text.len(), |i| end + i + 1);
        let before = &self.text[line_start..start];
        let after = self.text[end..line_end].trim();
        if before.trim().is_empty() && (after.is_empty() || after.starts_with("//")) {
            (line_start, line_end)
        } else {
            let semi = self.text[end..].starts_with(';') as usize;
            (start, end + semi)
        }
    }

    fn remove_stmt(&mut self, stmt: &ast::Stmt) {
        let (start, end) = self.stmt_removal(stmt.span.start, stmt.span.end);
        self.changes.push((start, end, String::new()));
    }

    /// Renames a statement's target, used when a box that drives an output
    /// directly has to let go of it.
    fn release_output(&mut self, port: &str) -> Result<(), String> {
        if let Some((stmt, call)) = self.call_stmt(port) {
            let fresh = fresh_name(self.info, &call.chip.name);
            self.changes.push((stmt.target.span.start, stmt.target.span.end, fresh.clone()));
            self.renamed.push((port.to_string(), fresh));
        }
        Ok(())
    }

    fn connect(&mut self, sink: &Sink, source: Option<&String>) -> Result<(), String> {
        match sink {
            Sink::Instance { name, port } => {
                let (ports, mut args) = self.call_args(name)?;
                let slot =
                    ports.iter().position(|p| p == port).ok_or_else(|| format!("`{name}` has no input `{port}`"))?;
                args[slot] = source.cloned();
                self.rewrite_call(name, &ports, &args);
            }
            Sink::Output(port) => {
                let Some(Name::Output(index)) = self.info.names.get(port).copied() else {
                    return Err(format!("`{}` has no output `{port}`", self.info.name));
                };
                let mut replaced = false;
                for stmt in &self.def.stmts {
                    let StmtValue::Expr(value) = &stmt.value else { continue };
                    if stmt.target.name != *port {
                        continue;
                    }
                    match (source, stmt.select, replaced) {
                        (Some(source), None, false) => {
                            self.changes.push((value.span.start, value.span.end, source.clone()));
                            replaced = true;
                        }
                        _ => self.remove_stmt(stmt),
                    }
                }
                let driven_by_box = self.info.instances.iter().any(|i| i.drives == Some(Name::Output(index)));
                if driven_by_box {
                    self.release_output(port)?;
                }
                if let (Some(source), false) = (source, replaced) {
                    self.appended.push(format!("{port} = {source}"));
                }
            }
        }
        Ok(())
    }

    /// Disconnects every pin that reads `name`, and removes connection
    /// lines that read it.
    fn disconnect_readers(&mut self, name: &str, skip: Option<&str>) -> Result<(), String> {
        for (line, stmt) in self.def.stmts.iter().enumerate() {
            match &stmt.value {
                StmtValue::Call(_) => {
                    let Some(inst) = self.info.instances.iter().find(|i| i.line == line) else { continue };
                    if Some(inst.name.as_str()) == skip {
                        continue;
                    }
                    let Ok((ports, args)) = self.call_args(&inst.name) else { continue };
                    let reads = |e: &Option<String>| e.is_some();
                    let kept: Vec<Option<String>> = inst
                        .inputs
                        .iter()
                        .zip(&args)
                        .map(|(expr, arg)| match expr {
                            Some(expr) if mentions(expr, name) => None,
                            _ => arg.clone(),
                        })
                        .collect();
                    if kept.iter().filter(|a| reads(a)).count() != args.iter().filter(|a| reads(a)).count() {
                        let target = inst.name.clone();
                        self.rewrite_call(&target, &ports, &kept);
                    }
                }
                StmtValue::Expr(value) => {
                    if mentions(value, name) {
                        self.remove_stmt(stmt);
                    }
                }
            }
        }
        Ok(())
    }

    fn remove_instance(&mut self, name: &str) -> Result<(), String> {
        let (stmt, _) = self.call_stmt(name).ok_or_else(|| format!("there's no box named `{name}`"))?;
        self.remove_stmt(stmt);
        // A box named after an output drives that output; reads of the name
        // mean the output, which stays. Otherwise disconnect its readers.
        if matches!(self.info.names.get(name), Some(Name::Instance(_))) {
            self.disconnect_readers(name, Some(name))?;
        }
        Ok(())
    }

    fn decls(&self, dir: PortDir) -> &[ast::PortDecl] {
        match dir {
            PortDir::In => &self.def.inputs,
            PortDir::Out => &self.def.outputs,
        }
    }

    fn line_start(&self, pos: usize) -> usize {
        self.text[..pos].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self, pos: usize) -> usize {
        self.text[pos..].find('\n').map_or(self.text.len(), |i| pos + i + 1)
    }

    fn indent(&self) -> String {
        let first =
            self.def.inputs.first().map(|d| d.span.start).or_else(|| self.def.stmts.first().map(|s| s.span.start));
        let Some(pos) = first else { return "  ".into() };
        let start = self.line_start(pos);
        let line = &self.text[start..];
        line[..line.len() - line.trim_start().len()].to_string()
    }

    fn add_port(&mut self, dir: PortDir, name: &str, width: usize) -> Result<(), String> {
        self.check_new_name(name)?;
        let decl = if width == 1 { name.to_string() } else { format!("{name}[{width}]") };
        if let Some(last) = self.decls(dir).last() {
            self.changes.push((last.span.end, last.span.end, format!(", {decl}")));
            return Ok(());
        }
        let keyword = match dir {
            PortDir::In => "in ",
            PortDir::Out => "out",
        };
        let line = format!("{}{keyword} {decl}\n", self.indent());
        // Outputs go right after the inputs; otherwise at the top of the body.
        let anchor = match (dir, self.def.inputs.last()) {
            (PortDir::Out, Some(last_in)) => self.line_end(last_in.span.end),
            _ => {
                let open = self.text[self.def.name.span.end..].find('{').map(|i| self.def.name.span.end + i);
                let open = open.ok_or("the chip is missing its `{`")?;
                self.line_end(open)
            }
        };
        self.changes.push((anchor, anchor, line));
        Ok(())
    }

    fn remove_port(&mut self, dir: PortDir, name: &str) -> Result<(), String> {
        let decls = self.decls(dir);
        let i =
            decls.iter().position(|d| d.name.name == name).ok_or_else(|| format!("there's no port named `{name}`"))?;
        let same_line = |a: usize, b: usize| !self.text[a.min(b)..a.max(b)].contains('\n');
        let this = &decls[i];
        let prev = i.checked_sub(1).map(|p| &decls[p]).filter(|p| same_line(p.span.end, this.span.start));
        let next = decls.get(i + 1).filter(|n| same_line(this.span.end, n.span.start));
        let range = match (prev, next) {
            (Some(prev), _) => (prev.span.end, this.span.end),
            (None, Some(next)) => (this.span.start, next.span.start),
            (None, None) => (self.line_start(this.span.start), self.line_end(this.span.end)),
        };
        self.changes.push((range.0, range.1, String::new()));

        self.disconnect_readers(name, None)?;
        if dir == PortDir::Out {
            for stmt in &self.def.stmts {
                if stmt.target.name == name && matches!(stmt.value, StmtValue::Expr(_)) {
                    self.remove_stmt(stmt);
                }
            }
            self.release_output(name)?;
        }
        Ok(())
    }

    fn rename(&mut self, old: &str, new: &str) -> Result<(), String> {
        if !self.info.names.contains_key(old) {
            return Err(format!("there's nothing named `{old}` in `{}`", self.info.name));
        }
        if old == new {
            return Ok(());
        }
        self.check_new_name(new)?;
        let start =
            self.text[self.def.name.span.end..].find('{').map_or(self.def.span.start, |i| self.def.name.span.end + i);
        let body = &self.text[start..self.def.span.end];
        let mut diags = Vec::new();
        let toks = lex(0, body, &mut diags);
        for (i, tok) in toks.iter().enumerate() {
            let Tok::Ident(word) = &tok.tok else { continue };
            if word != old {
                continue;
            }
            let prev = i.checked_sub(1).map(|p| &toks[p].tok);
            let next = toks.get(i + 1).map(|t| &t.tok);
            let line_first = matches!(prev, None | Some(Tok::Newline) | Some(Tok::LBrace) | Some(Tok::Semi));
            let is_keyword =
                line_first && matches!(word.as_str(), "in" | "out" | "wire") && matches!(next, Some(Tok::Ident(_)));
            let is_member = matches!(prev, Some(Tok::Dot));
            let is_arg_name = matches!(next, Some(Tok::Colon));
            let is_chip = matches!(next, Some(Tok::LParen));
            if !(is_keyword || is_member || is_arg_name || is_chip) {
                self.changes.push((start + tok.span.start, start + tok.span.end, new.to_string()));
            }
        }
        // Column names in this chip's test headers.
        for test in self.project.tests().filter(|t| t.chip.name == self.def.name.name) {
            let in_this_file = test.chip.span.file == self.def.name.span.file;
            if let (Some(header), true) = (&test.header, in_this_file) {
                for ident in header.inputs.iter().chain(&header.outputs).filter(|i| i.name == old) {
                    self.changes.push((ident.span.start, ident.span.end, new.to_string()));
                }
            }
        }
        Ok(())
    }

    fn finish(mut self) -> Result<String, String> {
        if !self.appended.is_empty() {
            let close = self.def.span.end.saturating_sub(1);
            if self.text.as_bytes().get(close) != Some(&b'}') {
                return Err("the chip is missing its closing `}`".into());
            }
            let indent = self.indent();
            let line_start = self.line_start(close);
            let lines: String = self.appended.iter().map(|l| format!("{indent}{l}\n")).collect();
            if self.text[line_start..close].trim().is_empty() {
                self.changes.push((line_start, line_start, lines));
            } else {
                self.changes.push((close, close, format!("\n{lines}")));
            }
        }
        self.changes.sort_by_key(|c| (c.0, c.1));
        self.changes.dedup();
        for pair in self.changes.windows(2) {
            if pair[0].1 > pair[1].0 {
                return Err("those changes overlap; try them one at a time".into());
            }
        }
        let mut out = self.text.to_string();
        for (start, end, replacement) in self.changes.iter().rev() {
            out.replace_range(*start..*end, replacement);
        }
        Ok(out)
    }
}

/// True if the expression reads `name`.
fn mentions(expr: &Expr, name: &str) -> bool {
    match &expr.kind {
        ExprKind::Ref { base, .. } => base.name == name,
        ExprKind::Literal(_) | ExprKind::Str(_) => false,
        ExprKind::Concat(parts) => parts.iter().any(|p| mentions(p, name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HALF: &str = "// A comment that must survive.\nchip H {\n  in  a, b\n  out sum, carry\n\n  sum   = Xor(a, b)   // the sum\n  carry = And(a, b)\n}\n";

    fn run(text: &str, edit: Edit) -> Result<String, String> {
        let project = Project::from_sources(vec![("H.chip".into(), text.into())]);
        assert!(project.diagnostics.is_empty(), "{:#?}", project.diagnostics);
        let design = Design::check(&project);
        let chip = project.chips().next().unwrap().name.name.clone();
        apply(&project, &design, &chip, &edit).map(|r| r.text)
    }

    fn clean(text: &str) -> Design {
        let project = Project::from_sources(vec![("H.chip".into(), text.into())]);
        assert!(project.diagnostics.is_empty(), "{text}\n{:#?}", project.diagnostics);
        let design = Design::check(&project);
        assert!(!design.has_errors(), "{text}\n{:#?}", design.diagnostics);
        design
    }

    #[test]
    fn builds_a_half_adder_from_nothing() {
        let mut text = "chip H {\n}\n".to_string();
        let steps = [
            Edit::AddPort { dir: PortDir::In, name: "a".into(), width: 1 },
            Edit::AddPort { dir: PortDir::In, name: "b".into(), width: 1 },
            Edit::AddPort { dir: PortDir::Out, name: "sum".into(), width: 1 },
            Edit::AddPort { dir: PortDir::Out, name: "carry".into(), width: 1 },
            Edit::AddInstance { name: "xor1".into(), callee: "Xor".into() },
            Edit::AddInstance { name: "and1".into(), callee: "And".into() },
            Edit::Connect { sink: Sink::Instance { name: "xor1".into(), port: "b".into() }, source: "b".into() },
            Edit::Connect { sink: Sink::Instance { name: "xor1".into(), port: "a".into() }, source: "a".into() },
            Edit::Connect { sink: Sink::Instance { name: "and1".into(), port: "a".into() }, source: "a".into() },
            Edit::Connect { sink: Sink::Instance { name: "and1".into(), port: "b".into() }, source: "b".into() },
            Edit::Connect { sink: Sink::Output("sum".into()), source: "xor1".into() },
            Edit::Connect { sink: Sink::Output("carry".into()), source: "and1".into() },
        ];
        for step in steps {
            text = run(&text, step.clone()).unwrap_or_else(|e| panic!("{step:?}: {e}"));
        }
        assert_eq!(
            text,
            "chip H {\n  in  a, b\n  out sum, carry\n  xor1 = Xor(a, b)\n  and1 = And(a, b)\n  sum = xor1\n  carry = and1\n}\n"
        );
        clean(&text);
    }

    #[test]
    fn partial_connections_use_named_arguments() {
        let text =
            run(HALF, Edit::Disconnect { sink: Sink::Instance { name: "sum".into(), port: "a".into() } }).unwrap();
        assert!(text.contains("sum   = Xor(b: b)   // the sum"), "{text}");
        assert!(text.starts_with("// A comment that must survive."));
    }

    #[test]
    fn removing_a_box_disconnects_its_readers() {
        let text = "chip H {\n  in a\n  out y\n  n1 = Not(a)\n  n2 = Nand(n1, a)\n  y = n1\n}\n";
        let out = run(text, Edit::RemoveInstance { name: "n1".into() }).unwrap();
        assert_eq!(out, "chip H {\n  in a\n  out y\n  n2 = Nand(b: a)\n}\n");
    }

    #[test]
    fn connecting_an_output_driven_by_a_box_frees_the_box() {
        let out = run(HALF, Edit::Connect { sink: Sink::Output("sum".into()), source: "a".into() }).unwrap();
        assert!(out.contains("  xor1   = Xor(a, b)   // the sum\n"), "{out}");
        assert!(out.contains("  sum = a\n}"), "{out}");
        clean(&out);
    }

    #[test]
    fn renames_everywhere_but_port_names_of_other_chips() {
        let text = "chip H {\n  in a, b\n  out out\n  out = Nand(a: a, b: b)\n}\ntest H {\n  a b | out\n  0 0 | 1\n}\n";
        let out = run(text, Edit::Rename { old: "a".into(), new: "x".into() }).unwrap();
        assert_eq!(
            out,
            "chip H {\n  in x, b\n  out out\n  out = Nand(a: x, b: b)\n}\ntest H {\n  x b | out\n  0 0 | 1\n}\n"
        );
        let out = run(text, Edit::Rename { old: "out".into(), new: "y".into() }).unwrap();
        assert!(out.contains("  out y\n  y = Nand(a: a, b: b)"), "{out}");
    }

    #[test]
    fn removes_ports_from_shared_lines() {
        let out = run(HALF, Edit::RemovePort { dir: PortDir::In, name: "a".into() }).unwrap();
        assert!(out.contains("  in  b\n"), "{out}");
        assert!(out.contains("sum   = Xor(b: b)"), "{out}");
        let out = run(HALF, Edit::RemovePort { dir: PortDir::Out, name: "carry".into() }).unwrap();
        assert!(out.contains("  out sum\n"), "{out}");
        assert!(out.contains("and1 = And(a, b)"), "{out}");
    }

    #[test]
    fn rejects_bad_names() {
        assert!(run(HALF, Edit::AddInstance { name: "a".into(), callee: "Not".into() }).is_err());
        assert!(run(HALF, Edit::AddPort { dir: PortDir::In, name: "1x".into(), width: 1 }).is_err());
    }

    /// Extracts boxes from an example chip, then checks the original tests
    /// still pass with the new chip in the project.
    fn extract_and_test(chip: &str, boxes: &[&str], new_name: &str) -> (String, String) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../chips");
        let project = Project::load(&dir).unwrap();
        let design = Design::check(&project);
        let selected: Vec<String> = boxes.iter().map(|b| b.to_string()).collect();
        let out = extract_chip(&project, &design, chip, &selected, new_name).unwrap();
        let mut sources: Vec<(std::path::PathBuf, String)> = project
            .files
            .iter()
            .map(|(id, _)| {
                let f = project.sources.get(*id);
                let text = if *id == out.parent_file { out.parent_text.clone() } else { f.text.clone() };
                (f.path.clone(), text)
            })
            .collect();
        sources.push((format!("{new_name}.chip").into(), out.new_text.clone()));
        // Rom programs come along unchanged.
        sources.extend(project.programs.iter().map(|&id| {
            let f = project.sources.get(id);
            (f.path.clone(), f.text.clone())
        }));
        let after = Project::from_sources(sources);
        assert!(after.diagnostics.is_empty(), "{:#?}\n{}", after.diagnostics, out.parent_text);
        let design = Design::check(&after);
        let errors: Vec<_> = design.diagnostics.iter().filter(|d| d.is_error()).map(|d| d.message.clone()).collect();
        assert!(errors.is_empty(), "{errors:?}\n{}\n{}", out.parent_text, out.new_text);
        for test in design.tests.iter().filter(|t| t.chip == chip) {
            assert!(crate::testing::run_test(&design, test).passed(), "{chip} tests fail after extracting");
        }
        (out.parent_text, out.new_text)
    }

    #[test]
    fn extracts_part_of_an_xor() {
        let (parent, new) = extract_and_test("Xor", &["n2", "n3"], "Pair");
        assert_eq!(new, "chip Pair {\n  in  a, n1, b\n  out n2, n3\n\n  n2 = Nand(a, n1)\n  n3 = Nand(b, n1)\n}\n");
        assert!(parent.contains("out = Nand(pair1.n2, pair1.n3)"), "{parent}");
        assert!(parent.contains("pair1 = Pair(a, n1, b)"), "{parent}");
        assert!(!parent.contains("n2  = Nand"), "{parent}");
    }

    #[test]
    fn extracts_boxes_that_drive_outputs() {
        let (parent, new) = extract_and_test("HalfAdder", &["sum", "carry"], "Core");
        assert!(new.contains("out sum, carry"), "{new}");
        assert!(parent.contains("core1 = Core(a, b)\n  sum = core1.sum\n  carry = core1.carry\n"), "{parent}");
    }

    #[test]
    fn extracts_a_box_with_several_outputs() {
        let (parent, new) = extract_and_test("FullAdder", &["h1"], "Front");
        assert!(new.contains("out h1_sum, h1_carry"), "{new}");
        assert!(new.contains("h1_sum   = h1.sum"), "{new}");
        assert!(parent.contains("h2   = HalfAdder(front1.h1_sum, cin)"), "{parent}");
        assert!(parent.contains("Or(front1.h1_carry, h2.carry)"), "{parent}");
    }

    #[test]
    fn rewiring_a_rom_keeps_its_file() {
        let text = "chip H {\n  in  a[4]\n  out y[16]\n\n  y = Rom(file: \"p.hex\")\n}\n";
        let edit = Edit::Connect { sink: Sink::Instance { name: "y".into(), port: "addr".into() }, source: "a".into() };
        let out = run(text, edit).unwrap();
        assert!(out.contains("y = Rom(a, file: \"p.hex\")"), "{out}");
    }
}
