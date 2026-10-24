//! Checks chips for mistakes and works out how wide every signal is.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::ast::{self, Expr, ExprKind, Select, StmtValue, TestValueKind};
use crate::diag::{Diagnostic, Span, suggest};
use crate::gate::{GateKind, PortWidth};
use crate::logic::{Logic, bits_from_u64};
use crate::project::Project;

/// Everything Wafer knows about a project after checking it.
pub struct Design {
    pub chips: BTreeMap<String, ChipInfo>,
    pub tests: Vec<TestInfo>,
    /// Mistakes found while checking (syntax errors live on the [`Project`]).
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct Port {
    pub name: String,
    pub width: usize,
    pub span: Span,
}

/// What a name inside a chip refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Name {
    Input(usize),
    Output(usize),
    Wire(usize),
    Instance(usize),
}

#[derive(Clone, Debug)]
pub struct ChipInfo {
    pub name: String,
    pub span: Span,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    /// Declared wires, plus the ones made by lines like `x = a[3]`.
    pub wires: Vec<Port>,
    /// One per gate or chip line, in file order.
    pub instances: Vec<Instance>,
    /// Lines that connect wires without adding a box, like `sum[0] = s0`.
    pub assigns: Vec<Assign>,
    pub names: HashMap<String, Name>,
    /// True when this chip has a mistake of its own.
    pub has_errors: bool,
}

/// A box on the canvas: one built-in gate or one use of another chip.
#[derive(Clone, Debug)]
pub struct Instance {
    pub name: String,
    pub callee: Callee,
    /// What's connected to each of the callee's inputs, in its port order.
    pub inputs: Vec<Option<Expr>>,
    /// The output or wire this box drives directly, when its line is named
    /// after one (`sum = Xor(a, b)` where `sum` is an output).
    pub drives: Option<Name>,
    /// One of several drivers of a shared wire (`data <- TriBuf(x, en)`).
    pub shared: bool,
    /// Which statement in the chip it comes from.
    pub line: usize,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Callee {
    /// A built-in part, repeated once per bit when its inputs are buses.
    /// `addr` is a `Ram`'s address width (0 for everything else).
    Gate {
        kind: GateKind,
        width: usize,
        addr: usize,
    },
    Chip(String),
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Assign {
    pub target: Name,
    pub select: Option<Select>,
    pub value: Expr,
    pub span: Span,
}

/// A chip's ports as the chips using it see them.
#[derive(Clone, Debug, Default)]
pub struct Signature {
    pub inputs: Vec<(String, usize)>,
    pub outputs: Vec<(String, usize)>,
}

#[derive(Clone, Debug)]
pub struct TestInfo {
    pub chip: String,
    pub span: Span,
    pub steps: Vec<TestStepInfo>,
}

/// One checked test step. Inputs are (input port, value); checks are
/// (signal path, expected value), where `None` means "don't care".
#[derive(Clone, Debug)]
pub enum TestStepInfo {
    /// A truth-table row: set the inputs, settle, check the outputs.
    Row {
        span: Span,
        inputs: Vec<(usize, Vec<Logic>)>,
        checks: Vec<(String, Option<Vec<Logic>>)>,
    },
    Set {
        span: Span,
        inputs: Vec<(usize, Vec<Logic>)>,
    },
    Tick {
        span: Span,
        count: u64,
    },
    Expect {
        span: Span,
        checks: Vec<(String, Option<Vec<Logic>>)>,
    },
}

impl Design {
    pub fn check(project: &Project) -> Design {
        let mut diags = Vec::new();

        let mut defs: BTreeMap<String, &ast::ChipDef> = BTreeMap::new();
        for def in project.chips() {
            if let Some(first) = defs.get(&def.name.name) {
                diags.push(
                    Diagnostic::error(def.name.span, format!("there's already a chip named `{}`", def.name.name))
                        .note(format!("the first one is at {}", project.sources.location(first.name.span))),
                );
            } else {
                defs.insert(def.name.name.clone(), def);
            }
        }

        let mut chips: BTreeMap<String, ChipInfo> =
            defs.iter().map(|(name, def)| (name.clone(), declare(def, &mut diags))).collect();
        let sigs: HashMap<String, Signature> = chips
            .iter()
            .map(|(name, info)| {
                let ports = |ports: &[Port]| ports.iter().map(|p| (p.name.clone(), p.width)).collect();
                (name.clone(), Signature { inputs: ports(&info.inputs), outputs: ports(&info.outputs) })
            })
            .collect();
        for (name, def) in &defs {
            let info = chips.remove(name).expect("declared above");
            let errors_before = count_errors(&diags);
            let mut checker = BodyCheck { wire_widths: Vec::new(), info, sigs: &sigs, diags: &mut diags };
            checker.check(def);
            let mut info = checker.info;
            info.has_errors |= count_errors(&diags) > errors_before;
            chips.insert(name.clone(), info);
        }
        find_recursion(&mut chips, &mut diags);

        let tests = project.tests().filter_map(|def| check_test(def, &chips, &mut diags)).collect();
        Design { chips, tests, diagnostics: diags }
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }

    pub fn signature(&self, callee: &Callee) -> Signature {
        match callee {
            Callee::Gate { kind, width, addr } => gate_signature(*kind, *width, *addr),
            Callee::Chip(name) => {
                let info = &self.chips[name];
                let ports = |ports: &[Port]| ports.iter().map(|p| (p.name.clone(), p.width)).collect();
                Signature { inputs: ports(&info.inputs), outputs: ports(&info.outputs) }
            }
            Callee::Unknown => Signature::default(),
        }
    }

    /// True when the chip and every chip inside it are free of mistakes.
    pub fn is_buildable(&self, chip: &str) -> bool {
        fn walk<'a>(design: &'a Design, chip: &'a str, seen: &mut HashSet<&'a str>) -> bool {
            if !seen.insert(chip) {
                return true;
            }
            let Some(info) = design.chips.get(chip) else { return false };
            !info.has_errors
                && info.instances.iter().all(|inst| match &inst.callee {
                    Callee::Chip(name) => walk(design, name, seen),
                    Callee::Gate { .. } => true,
                    Callee::Unknown => false,
                })
        }
        walk(self, chip, &mut HashSet::new())
    }

    /// How many levels of chips are inside this one (built-in gates are level 0).
    pub fn depth(&self, chip: &str) -> usize {
        fn walk(design: &Design, chip: &str, guard: usize) -> usize {
            let Some(info) = design.chips.get(chip) else { return 0 };
            if guard == 0 {
                return 0;
            }
            1 + info
                .instances
                .iter()
                .map(|inst| match &inst.callee {
                    Callee::Chip(name) => walk(design, name, guard - 1),
                    _ => 0,
                })
                .max()
                .unwrap_or(0)
        }
        walk(self, chip, 64)
    }
}

fn gate_signature(kind: GateKind, width: usize, addr: usize) -> Signature {
    let port = |i: usize| match kind.port_width(i) {
        PortWidth::Bus => width,
        PortWidth::Bit => 1,
        PortWidth::Address => addr,
    };
    Signature {
        inputs: kind.inputs().iter().enumerate().map(|(i, &n)| (n.to_string(), port(i))).collect(),
        outputs: vec![("out".to_string(), width)],
    }
}

fn count_errors(diags: &[Diagnostic]) -> usize {
    diags.iter().filter(|d| d.is_error()).count()
}

/// Reads the `in`, `out` and `wire` lines.
fn declare(def: &ast::ChipDef, diags: &mut Vec<Diagnostic>) -> ChipInfo {
    let mut info = ChipInfo {
        name: def.name.name.clone(),
        span: def.name.span,
        inputs: Vec::new(),
        outputs: Vec::new(),
        wires: Vec::new(),
        instances: Vec::new(),
        assigns: Vec::new(),
        names: HashMap::new(),
        has_errors: false,
    };
    let groups = [(&def.inputs, 0), (&def.outputs, 1), (&def.wires, 2)];
    for (decls, group) in groups {
        for decl in decls {
            if info.names.contains_key(&decl.name.name) {
                diags.push(Diagnostic::error(decl.name.span, format!("`{}` is declared twice", decl.name.name)));
                info.has_errors = true;
                continue;
            }
            let port = Port { name: decl.name.name.clone(), width: decl.width, span: decl.span };
            let list = match group {
                0 => &mut info.inputs,
                1 => &mut info.outputs,
                _ => &mut info.wires,
            };
            list.push(port);
            let name = match group {
                0 => Name::Input(list.len() - 1),
                1 => Name::Output(list.len() - 1),
                _ => Name::Wire(list.len() - 1),
            };
            info.names.insert(decl.name.name.clone(), name);
        }
    }
    info
}

struct BodyCheck<'a> {
    info: ChipInfo,
    sigs: &'a HashMap<String, Signature>,
    diags: &'a mut Vec<Diagnostic>,
    /// `None` while a wire's width is still being worked out.
    wire_widths: Vec<Option<usize>>,
}

impl BodyCheck<'_> {
    fn error(&mut self, diagnostic: Diagnostic) {
        self.diags.push(diagnostic);
    }

    fn check(&mut self, def: &ast::ChipDef) {
        self.wire_widths = self.info.wires.iter().map(|w| Some(w.width)).collect();
        for (line, stmt) in def.stmts.iter().enumerate() {
            if stmt.drive {
                self.declare_driver(line, stmt);
            } else {
                self.declare_stmt(line, stmt);
            }
        }
        self.infer_widths();
        self.validate();
    }

    /// `data <- TriBuf(x, en)`: adds a driver to a shared wire. Drivers are
    /// named after the wire: `data_d1`, `data_d2`, ...
    fn declare_driver(&mut self, line: usize, stmt: &ast::Stmt) {
        let target = &stmt.target;
        let StmtValue::Call(call) = &stmt.value else { return };
        if let Some(select) = stmt.select {
            self.error(
                Diagnostic::error(select.span, "a shared wire is driven whole")
                    .note(format!("write `{} <- ...`; each driver drives every bit", target.name)),
            );
            return;
        }
        let Some(driven @ (Name::Output(_) | Name::Wire(_))) = self.info.names.get(&target.name).copied() else {
            self.error(
                Diagnostic::error(target.span, format!("`{}` has to be declared before it can be shared", target.name))
                    .note(format!("add `wire {}[N]` to the chip (or make it an output)", target.name)),
            );
            return;
        };
        let n = self.info.instances.iter().filter(|i| i.shared && i.drives == Some(driven)).count() + 1;
        let name = format!("{}_d{n}", target.name);
        let mut instance = self.new_instance(call, &name, Some(driven), stmt.span, line);
        instance.shared = true;
        self.info.instances.push(instance);
        let index = self.info.instances.len() - 1;
        self.info.names.insert(name, Name::Instance(index));
    }

    fn declare_stmt(&mut self, line: usize, stmt: &ast::Stmt) {
        let target = &stmt.target;
        let existing = self.info.names.get(&target.name).copied();
        match (&stmt.value, existing) {
            (_, Some(Name::Input(_))) => self.error(
                Diagnostic::error(target.span, format!("`{}` is an input, so it can't be driven here", target.name))
                    .note("inputs are driven by whatever chip uses this one"),
            ),
            (_, Some(Name::Instance(_))) => self.error(
                Diagnostic::error(target.span, format!("`{}` is already defined", target.name))
                    .note("each name can only be defined once"),
            ),
            (StmtValue::Call(call), Some(driven @ (Name::Output(_) | Name::Wire(_)))) => {
                if let Some(select) = stmt.select {
                    self.error(Diagnostic::error(select.span, "a gate's output can't drive part of a bus").note(
                        format!(
                            "give the gate its own name, then connect it: `s = {}(...)` and `{}{} = s`",
                            call.chip.name,
                            target.name,
                            bit_text(&select)
                        ),
                    ));
                    return;
                }
                let instance = self.new_instance(call, &target.name, Some(driven), stmt.span, line);
                self.info.instances.push(instance);
            }
            (StmtValue::Call(call), None) => {
                if stmt.select.is_some() {
                    self.undeclared_bus(target);
                    return;
                }
                let instance = self.new_instance(call, &target.name, None, stmt.span, line);
                self.info.instances.push(instance);
                let index = self.info.instances.len() - 1;
                self.info.names.insert(target.name.clone(), Name::Instance(index));
            }
            (StmtValue::Expr(value), Some(driven @ (Name::Output(_) | Name::Wire(_)))) => {
                self.info.assigns.push(Assign {
                    target: driven,
                    select: stmt.select,
                    value: value.clone(),
                    span: stmt.span,
                });
            }
            (StmtValue::Expr(value), None) => {
                if stmt.select.is_some() {
                    self.undeclared_bus(target);
                    return;
                }
                // `x = a[3]` makes a new wire `x` whose width comes from `a[3]`.
                self.info.wires.push(Port { name: target.name.clone(), width: 0, span: target.span });
                self.wire_widths.push(None);
                let wire = Name::Wire(self.info.wires.len() - 1);
                self.info.names.insert(target.name.clone(), wire);
                self.info.assigns.push(Assign { target: wire, select: None, value: value.clone(), span: stmt.span });
            }
        }
    }

    fn undeclared_bus(&mut self, target: &ast::Ident) {
        self.error(
            Diagnostic::error(target.span, format!("`{}` isn't declared, so it can't be split into bits", target.name))
                .note(format!("add `wire {}[N]` to the chip", target.name)),
        );
    }

    fn new_instance(
        &mut self,
        call: &ast::Call,
        name: &str,
        drives: Option<Name>,
        span: Span,
        line: usize,
    ) -> Instance {
        let chip = &call.chip.name;
        let (callee, ports): (Callee, Vec<String>) = if let Some(sig) = self.sigs.get(chip) {
            (Callee::Chip(chip.clone()), sig.inputs.iter().map(|(n, _)| n.clone()).collect())
        } else if let Some(kind) = GateKind::from_name(chip) {
            (Callee::Gate { kind, width: 0, addr: 0 }, kind.inputs().iter().map(|n| n.to_string()).collect())
        } else {
            let known = self.sigs.keys().map(String::as_str).chain(GateKind::ALL.iter().map(|g| g.name()));
            let mut d = Diagnostic::error(call.chip.span, format!("there's no chip named `{chip}`"));
            if let Some(close) = suggest(chip, known) {
                d = d.label(format!("did you mean `{close}`?"));
            }
            self.error(d);
            return Instance {
                name: name.into(),
                callee: Callee::Unknown,
                inputs: Vec::new(),
                drives,
                shared: false,
                line,
                span,
            };
        };

        let mut slots: Vec<Option<Expr>> = vec![None; ports.len()];
        let mut named_seen = false;
        for (position, arg) in call.args.iter().enumerate() {
            let slot = match &arg.name {
                Some(port) => {
                    named_seen = true;
                    match ports.iter().position(|p| *p == port.name) {
                        Some(slot) => slot,
                        None => {
                            let mut d = Diagnostic::error(port.span, format!("`{chip}` has no input `{}`", port.name))
                                .note(format!("its inputs are {}", list(&ports)));
                            if let Some(close) = suggest(&port.name, ports.iter().map(String::as_str)) {
                                d = d.label(format!("did you mean `{close}`?"));
                            }
                            self.error(d);
                            continue;
                        }
                    }
                }
                None if named_seen => {
                    self.error(Diagnostic::error(arg.value.span, "unnamed inputs have to come before named ones"));
                    continue;
                }
                None if position >= ports.len() => {
                    self.error(
                        Diagnostic::error(
                            arg.value.span,
                            format!("`{chip}` only has {} input{}", ports.len(), plural(ports.len())),
                        )
                        .note(format!("its inputs are {}", list(&ports))),
                    );
                    continue;
                }
                None => position,
            };
            if slots[slot].is_some() {
                self.error(Diagnostic::error(arg.value.span, format!("input `{}` is connected twice", ports[slot])));
                continue;
            }
            slots[slot] = Some(arg.value.clone());
        }
        let missing: Vec<String> =
            ports.iter().zip(&slots).filter(|(_, s)| s.is_none()).map(|(p, _)| p.clone()).collect();
        // Unconnected inputs are allowed while you're still wiring things up.
        if !missing.is_empty() {
            self.error(
                Diagnostic::warning(call.span, format!("`{name}` has nothing connected to {}", list(&missing)))
                    .note("unconnected inputs read as Z"),
            );
        }
        Instance { name: name.into(), callee, inputs: slots, drives, shared: false, line, span }
    }

    /// Output ports of the box at `index` and their widths (`None` while a
    /// gate's width is still unknown).
    fn instance_outputs(&self, index: usize) -> Vec<(String, Option<usize>)> {
        match &self.info.instances[index].callee {
            Callee::Gate { width, .. } => vec![("out".into(), (*width > 0).then_some(*width))],
            Callee::Chip(chip) => self.sigs[chip].outputs.iter().map(|(n, w)| (n.clone(), Some(*w))).collect(),
            Callee::Unknown => Vec::new(),
        }
    }

    fn name_width(&self, name: Name) -> Option<usize> {
        match name {
            Name::Input(i) => Some(self.info.inputs[i].width),
            Name::Output(i) => Some(self.info.outputs[i].width),
            Name::Wire(i) => self.wire_widths[i],
            Name::Instance(i) => match self.instance_outputs(i).as_slice() {
                [(_, width)] => *width,
                _ => None,
            },
        }
    }

    /// The width of an expression, quietly. Literals count only when
    /// `literals` is true, since a bare `1` takes whatever width it's given.
    fn quiet_width(&self, expr: &Expr, literals: bool) -> Option<usize> {
        match &expr.kind {
            ExprKind::Literal(lit) => literals.then(|| lit.natural_width()),
            ExprKind::Concat(parts) => parts.iter().map(|p| self.quiet_width(p, true)).sum(),
            ExprKind::Ref { base, port, select } => {
                let name = *self.info.names.get(&base.name)?;
                let width = match (name, port) {
                    (Name::Instance(i), Some(port)) => {
                        self.instance_outputs(i).into_iter().find(|(n, _)| *n == port.name)?.1?
                    }
                    (_, Some(_)) => return None,
                    (name, None) => self.name_width(name)?,
                };
                Some(select.map_or(width, |s| s.width()))
            }
        }
    }

    /// Works out the width of built-in gates (from their inputs) and of
    /// wires made by assignment, repeating until nothing changes.
    fn infer_widths(&mut self) {
        loop {
            let mut changed = false;
            for i in 0..self.info.instances.len() {
                let Callee::Gate { kind, width, addr } = self.info.instances[i].callee else { continue };
                // The part's width comes from its bus inputs; one-bit
                // inputs like `sel` don't count.
                let from = |want: PortWidth| {
                    self.info.instances[i]
                        .inputs
                        .iter()
                        .enumerate()
                        .filter(|(p, _)| kind.port_width(*p) == want)
                        .find_map(|(_, e)| e.as_ref().and_then(|e| self.quiet_width(e, false)))
                };
                let new_width = if width == 0 { from(PortWidth::Bus).unwrap_or(0) } else { width };
                let new_addr =
                    if kind == GateKind::Ram && addr == 0 { from(PortWidth::Address).unwrap_or(0) } else { addr };
                if (new_width, new_addr) != (width, addr) {
                    self.info.instances[i].callee = Callee::Gate { kind, width: new_width, addr: new_addr };
                    changed = true;
                }
            }
            for i in 0..self.info.assigns.len() {
                if let Name::Wire(w) = self.info.assigns[i].target
                    && self.wire_widths[w].is_none()
                    && let Some(width) = self.quiet_width(&self.info.assigns[i].value, true)
                {
                    self.wire_widths[w] = Some(width);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        for i in 0..self.info.instances.len() {
            let Callee::Gate { kind, width, addr } = self.info.instances[i].callee else { continue };
            // Ports given only numbers: use the widest number's width.
            let guess = |want: PortWidth| {
                let inputs: Vec<Option<&Expr>> = self.info.instances[i]
                    .inputs
                    .iter()
                    .enumerate()
                    .filter(|(p, _)| kind.port_width(*p) == want)
                    .map(|(_, e)| e.as_ref())
                    .collect();
                let all_literals = inputs.iter().flatten().all(|e| matches!(e.kind, ExprKind::Literal(_)));
                let widest = inputs.iter().flatten().filter_map(|e| self.quiet_width(e, true)).max();
                match widest {
                    Some(w) if all_literals => Ok(w),
                    _ if inputs.iter().all(Option::is_some) && !inputs.is_empty() => Err(()),
                    _ => Ok(1),
                }
            };
            let mut new_width = width;
            let mut new_addr = addr;
            let mut unknown = false;
            if width == 0 {
                new_width = guess(PortWidth::Bus).unwrap_or_else(|()| {
                    unknown = true;
                    1
                });
            }
            if kind == GateKind::Ram && addr == 0 {
                new_addr = guess(PortWidth::Address).unwrap_or_else(|()| {
                    unknown = true;
                    1
                });
            }
            if unknown {
                let name = self.info.instances[i].name.clone();
                let span = self.info.instances[i].span;
                self.error(Diagnostic::error(span, format!("can't work out how many bits `{name}` has")));
            }
            self.info.instances[i].callee = Callee::Gate { kind, width: new_width, addr: new_addr };
        }
        for w in 0..self.wire_widths.len() {
            if self.wire_widths[w].is_none() {
                let wire = &self.info.wires[w];
                let d = Diagnostic::error(wire.span, format!("can't work out how many bits `{}` has", wire.name));
                self.error(d);
                self.wire_widths[w] = Some(1);
            }
            self.info.wires[w].width = self.wire_widths[w].unwrap_or(1);
        }
    }

    /// Checks an expression, reporting mistakes, and returns its width.
    /// `expected` is the width it's going into, which bare numbers adopt.
    fn check_expr(&mut self, expr: &Expr, expected: Option<usize>) -> Option<usize> {
        match &expr.kind {
            ExprKind::Literal(lit) => {
                let width = expected.unwrap_or(lit.natural_width());
                if !lit.fits(width) {
                    self.error(Diagnostic::error(
                        expr.span,
                        format!("{} doesn't fit in {width} bit{}", lit.value, plural(width)),
                    ));
                }
                Some(width)
            }
            ExprKind::Concat(parts) => {
                let mut total = Some(0);
                for part in parts {
                    let width = self.check_expr(part, None);
                    total = total.zip(width).map(|(a, b)| a + b);
                }
                total
            }
            ExprKind::Ref { base, port, select } => {
                let Some(&name) = self.info.names.get(&base.name) else {
                    let known = self.info.names.keys().map(String::as_str);
                    let mut d = Diagnostic::error(base.span, format!("there's no signal named `{}`", base.name));
                    if let Some(close) = suggest(&base.name, known) {
                        d = d.label(format!("did you mean `{close}`?"));
                    }
                    self.error(d);
                    return None;
                };
                let width = match (name, port) {
                    (Name::Instance(i), Some(port)) => {
                        let outputs = self.instance_outputs(i);
                        match outputs.iter().find(|(n, _)| *n == port.name) {
                            Some((_, width)) => (*width)?,
                            None => {
                                let callee = self.callee_name(i);
                                let names: Vec<String> = outputs.iter().map(|(n, _)| n.clone()).collect();
                                let mut d =
                                    Diagnostic::error(port.span, format!("`{callee}` has no output `{}`", port.name))
                                        .note(format!("its outputs are {}", list(&names)));
                                if let Some(close) = suggest(&port.name, names.iter().map(String::as_str)) {
                                    d = d.label(format!("did you mean `{close}`?"));
                                }
                                self.error(d);
                                return None;
                            }
                        }
                    }
                    (Name::Instance(i), None) => {
                        let outputs = self.instance_outputs(i);
                        match outputs.as_slice() {
                            [(_, width)] => (*width)?,
                            [] => return None,
                            _ => {
                                let names: Vec<String> = outputs.iter().map(|(n, _)| n.clone()).collect();
                                self.error(
                                    Diagnostic::error(
                                        base.span,
                                        format!("`{}` has {} outputs", base.name, names.len()),
                                    )
                                    .label(format!("pick one, like `{}.{}`", base.name, names[0]))
                                    .note(format!("its outputs are {}", list(&names))),
                                );
                                return None;
                            }
                        }
                    }
                    (_, Some(port)) => {
                        self.error(Diagnostic::error(
                            port.span,
                            format!("`{}` is a wire, not a chip, so it has no `.{}`", base.name, port.name),
                        ));
                        return None;
                    }
                    (name, None) => self.name_width(name)?,
                };
                match select {
                    Some(select) if select.hi >= width => {
                        let range = if width == 1 {
                            format!("`{}` is a single bit", base.name)
                        } else {
                            format!("`{}` only has bits {}..0", base.name, width - 1)
                        };
                        self.error(
                            Diagnostic::error(select.span, format!("bit {} is out of range", select.hi)).note(range),
                        );
                        None
                    }
                    Some(select) => Some(select.width()),
                    None => Some(width),
                }
            }
        }
    }

    fn callee_name(&self, index: usize) -> String {
        match &self.info.instances[index].callee {
            Callee::Gate { kind, .. } => kind.name().into(),
            Callee::Chip(name) => name.clone(),
            Callee::Unknown => "?".into(),
        }
    }

    fn validate(&mut self) {
        let mut drivers: HashMap<Name, Vec<bool>> = HashMap::new();
        for (i, port) in self.info.outputs.iter().enumerate() {
            drivers.insert(Name::Output(i), vec![false; port.width]);
        }
        for (i, wire) in self.info.wires.iter().enumerate() {
            drivers.insert(Name::Wire(i), vec![false; wire.width]);
        }

        let shared: HashSet<Name> = self.info.instances.iter().filter(|i| i.shared).filter_map(|i| i.drives).collect();
        let mixed = |me: &Self, target: Name, span: Span| {
            let name = me.target_name(target);
            Diagnostic::error(
                span,
                format!("`{name}` is a shared wire (it has `<-` drivers), so this line needs `<-` too"),
            )
            .note(format!("every driver of a shared wire uses `<-`, like `{name} <- TriBuf(x, en)`"))
        };

        for i in 0..self.info.instances.len() {
            let instance = self.info.instances[i].clone();
            let sig = match &instance.callee {
                Callee::Gate { kind, width, addr } => gate_signature(*kind, *width, *addr),
                Callee::Chip(chip) => self.sigs[chip].clone(),
                Callee::Unknown => {
                    // Already reported; count its output as driven so the
                    // typo doesn't also show up as "never driven".
                    if let Some(target) = instance.drives {
                        let all = 0..drivers[&target].len();
                        self.mark_driven(target, all, instance.span, &mut drivers);
                    }
                    continue;
                }
            };
            let callee = self.callee_name(i);
            for ((port, expected), expr) in sig.inputs.iter().zip(&instance.inputs) {
                let Some(expr) = expr else { continue };
                if let Some(width) = self.check_expr(expr, Some(*expected))
                    && width != *expected
                {
                    self.error(Diagnostic::error(
                        expr.span,
                        format!(
                            "`{callee}` input `{port}` is {expected} bit{}, but this is {width}",
                            plural(*expected)
                        ),
                    ));
                }
            }
            if let Some(target) = instance.drives {
                let target_name = &if instance.shared { self.target_name(target) } else { instance.name.clone() };
                let target_width = drivers[&target].len();
                match sig.outputs.as_slice() {
                    [(_, width)] if *width != target_width => {
                        self.error(Diagnostic::error(
                            instance.span,
                            format!(
                                "`{target_name}` is {target_width} bit{}, but `{callee}` outputs {width}",
                                plural(target_width)
                            ),
                        ));
                    }
                    [_] => {}
                    outputs => {
                        let first = outputs.first().map_or("out", |(n, _)| n.as_str());
                        let short = callee.to_lowercase();
                        self.error(
                            Diagnostic::error(
                                instance.span,
                                format!("`{callee}` has {} outputs, so it needs a name of its own", outputs.len()),
                            )
                            .note(format!("try `{short}1 = {callee}(...)`, then `{target_name} = {short}1.{first}`")),
                        );
                    }
                }
                if instance.shared {
                    drivers.get_mut(&target).expect("outputs and wires are registered").fill(true);
                } else if shared.contains(&target) {
                    let d = mixed(self, target, instance.span);
                    self.error(d);
                } else {
                    self.mark_driven(target, 0..target_width, instance.span, &mut drivers);
                }
            }
        }

        for i in 0..self.info.assigns.len() {
            let assign = self.info.assigns[i].clone();
            let full = drivers[&assign.target].len();
            let target_name = self.target_name(assign.target);
            let bits = match assign.select {
                Some(select) if select.hi >= full => {
                    self.error(
                        Diagnostic::error(select.span, format!("bit {} is out of range", select.hi))
                            .note(format!("`{target_name}` only has bits {}..0", full - 1)),
                    );
                    continue;
                }
                Some(select) => select.lo..select.hi + 1,
                None => 0..full,
            };
            let expected = bits.len();
            if let Some(width) = self.check_expr(&assign.value, Some(expected))
                && width != expected
            {
                self.error(Diagnostic::error(
                    assign.value.span,
                    format!(
                        "`{target_name}{}` is {expected} bit{}, but this is {width}",
                        assign.select.map(|s| bit_text(&s)).unwrap_or_default(),
                        plural(expected)
                    ),
                ));
            }
            if shared.contains(&assign.target) {
                let d = mixed(self, assign.target, assign.span);
                self.error(d);
                continue;
            }
            self.mark_driven(assign.target, bits, assign.span, &mut drivers);
        }

        for (i, port) in self.info.outputs.clone().iter().enumerate() {
            let missing = undriven(&drivers[&Name::Output(i)]);
            if !missing.is_empty() {
                let what = if missing == format!("{}..0", port.width - 1) || port.width == 1 {
                    format!("output `{}` is never driven", port.name)
                } else {
                    format!("bits {missing} of output `{}` are never driven", port.name)
                };
                self.error(Diagnostic::warning(port.span, what).note("it reads as Z until something drives it"));
            }
        }
        for (i, wire) in self.info.wires.clone().iter().enumerate() {
            if !undriven(&drivers[&Name::Wire(i)]).is_empty() {
                self.error(Diagnostic::warning(
                    wire.span,
                    format!("wire `{}` isn't fully driven, so those bits read as Z", wire.name),
                ));
            }
        }
    }

    fn target_name(&self, name: Name) -> String {
        match name {
            Name::Input(i) => self.info.inputs[i].name.clone(),
            Name::Output(i) => self.info.outputs[i].name.clone(),
            Name::Wire(i) => self.info.wires[i].name.clone(),
            Name::Instance(i) => self.info.instances[i].name.clone(),
        }
    }

    fn mark_driven(
        &mut self,
        target: Name,
        bits: std::ops::Range<usize>,
        span: Span,
        drivers: &mut HashMap<Name, Vec<bool>>,
    ) {
        let flags = drivers.get_mut(&target).expect("outputs and wires are registered");
        let doubled: Vec<usize> = bits.clone().filter(|&b| flags[b]).collect();
        for b in bits {
            flags[b] = true;
        }
        if !doubled.is_empty() {
            let name = self.target_name(target);
            let what = if doubled.len() == flags.len() {
                format!("`{name}` is driven twice")
            } else {
                format!("`{name}[{}]` is driven twice", ranges(&doubled))
            };
            self.error(Diagnostic::error(span, what).label("second driver").note("a wire can only have one driver"));
        }
    }
}

/// Bits nobody drives, like `7..4, 1`.
fn undriven(flags: &[bool]) -> String {
    let missing: Vec<usize> = (0..flags.len()).filter(|&b| !flags[b]).collect();
    ranges(&missing)
}

/// Formats bit numbers as ranges, highest first: `7..4, 1`.
fn ranges(bits: &[usize]) -> String {
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for &b in bits {
        match groups.last_mut() {
            Some((_, hi)) if *hi + 1 == b => *hi = b,
            _ => groups.push((b, b)),
        }
    }
    groups
        .iter()
        .rev()
        .map(|&(lo, hi)| if lo == hi { lo.to_string() } else { format!("{hi}..{lo}") })
        .collect::<Vec<_>>()
        .join(", ")
}

fn bit_text(select: &Select) -> String {
    if select.hi == select.lo { format!("[{}]", select.hi) } else { format!("[{}:{}]", select.hi, select.lo) }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// `` `a`, `b` and `cin` ``
fn list(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
    match quoted.as_slice() {
        [] => "none".into(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A chip can't contain itself, directly or through other chips.
fn find_recursion(chips: &mut BTreeMap<String, ChipInfo>, diags: &mut Vec<Diagnostic>) {
    fn visit(
        chip: &str,
        chips: &BTreeMap<String, ChipInfo>,
        stack: &mut Vec<String>,
        done: &mut HashSet<String>,
        cycles: &mut Vec<(Vec<String>, Span)>,
    ) {
        if done.contains(chip) {
            return;
        }
        stack.push(chip.to_string());
        for inst in &chips[chip].instances {
            let Callee::Chip(callee) = &inst.callee else { continue };
            if let Some(start) = stack.iter().position(|c| c == callee) {
                let mut path = stack[start..].to_vec();
                path.push(callee.clone());
                cycles.push((path, inst.span));
            } else {
                visit(callee, chips, stack, done, cycles);
            }
        }
        stack.pop();
        done.insert(chip.to_string());
    }

    let mut cycles = Vec::new();
    let mut done = HashSet::new();
    let names: Vec<String> = chips.keys().cloned().collect();
    for name in &names {
        visit(name, chips, &mut Vec::new(), &mut done, &mut cycles);
    }
    for (path, span) in cycles {
        let message = if path.len() == 2 {
            format!("`{}` can't contain itself", path[0])
        } else {
            format!("`{}` ends up containing itself", path[0])
        };
        diags.push(Diagnostic::error(span, message).note(path.join(" → ")));
        for chip in &path {
            if let Some(info) = chips.get_mut(chip) {
                info.has_errors = true;
            }
        }
    }
}

/// Turns a value written in a test into bits for a signal of `width`.
/// `Some(None)` is "don't care".
fn test_bits(
    v: &ast::TestValue,
    name: &str,
    width: usize,
    is_input: bool,
    diags: &mut Vec<Diagnostic>,
) -> Option<Option<Vec<Logic>>> {
    match v.kind {
        TestValueKind::Number(lit) => {
            if !lit.fits(width) {
                diags.push(Diagnostic::error(
                    v.span,
                    format!("{} doesn't fit in `{name}` ({width} bit{})", lit.value, plural(width)),
                ));
                return None;
            }
            Some(Some(bits_from_u64(lit.value, width)))
        }
        TestValueKind::X => Some(Some(vec![Logic::X; width])),
        TestValueKind::Z => Some(Some(vec![Logic::Z; width])),
        TestValueKind::Any if is_input => {
            diags.push(Diagnostic::error(v.span, "inputs need a value").note("`*` (don't care) only works for checks"));
            None
        }
        TestValueKind::Any => Some(None),
    }
}

fn check_test(def: &ast::TestDef, chips: &BTreeMap<String, ChipInfo>, diags: &mut Vec<Diagnostic>) -> Option<TestInfo> {
    let Some(chip) = chips.get(&def.chip.name) else {
        let mut d = Diagnostic::error(def.chip.span, format!("there's no chip named `{}` to test", def.chip.name));
        if let Some(close) = suggest(&def.chip.name, chips.keys().map(String::as_str)) {
            d = d.label(format!("did you mean `{close}`?"));
        }
        diags.push(d);
        return None;
    };
    let errors_before = count_errors(diags);

    let (input_ports, output_ports): (Vec<usize>, Vec<usize>) = match &def.header {
        None => ((0..chip.inputs.len()).collect(), (0..chip.outputs.len()).collect()),
        Some(header) => {
            let mut columns = |idents: &[ast::Ident], ports: &[Port], kind: &str| -> Vec<usize> {
                let mut found = Vec::new();
                for ident in idents {
                    match ports.iter().position(|p| p.name == ident.name) {
                        Some(i) if found.contains(&i) => {
                            diags.push(Diagnostic::error(ident.span, format!("`{}` is listed twice", ident.name)))
                        }
                        Some(i) => found.push(i),
                        None => {
                            let names: Vec<String> = ports.iter().map(|p| p.name.clone()).collect();
                            let mut d = Diagnostic::error(
                                ident.span,
                                format!("`{}` has no {kind} `{}`", chip.name, ident.name),
                            )
                            .note(format!("its {kind}s are {}", list(&names)));
                            if let Some(close) = suggest(&ident.name, names.iter().map(String::as_str)) {
                                d = d.label(format!("did you mean `{close}`?"));
                            }
                            diags.push(d);
                        }
                    }
                }
                found
            };
            (columns(&header.inputs, &chip.inputs, "input"), columns(&header.outputs, &chip.outputs, "output"))
        }
    };

    // How wide a name `expect` can check is: any port, wire or box output.
    let checkable = |name: &str| -> Option<usize> {
        match chip.names.get(name)? {
            Name::Input(i) => Some(chip.inputs[*i].width),
            Name::Output(i) => Some(chip.outputs[*i].width),
            Name::Wire(i) => Some(chip.wires[*i].width),
            Name::Instance(k) => match &chip.instances[*k].callee {
                Callee::Gate { width, .. } => Some(*width),
                Callee::Chip(c) => match chips.get(c)?.outputs.as_slice() {
                    [only] => Some(only.width),
                    _ => None,
                },
                Callee::Unknown => None,
            },
        }
    };

    let mut steps = Vec::new();
    for step in &def.steps {
        match step {
            ast::TestStep::Row(row) => {
                for (values, ports, kind) in
                    [(&row.inputs, &input_ports, "input"), (&row.outputs, &output_ports, "output")]
                {
                    if values.len() != ports.len() {
                        let names: Vec<String> = ports
                            .iter()
                            .map(|&p| {
                                if kind == "input" { chip.inputs[p].name.clone() } else { chip.outputs[p].name.clone() }
                            })
                            .collect();
                        diags.push(Diagnostic::error(
                            row.span,
                            format!(
                                "expected {} {kind} value{} ({}), found {}",
                                names.len(),
                                plural(names.len()),
                                names.join(" "),
                                values.len()
                            ),
                        ));
                    }
                }
                let inputs = row
                    .inputs
                    .iter()
                    .zip(&input_ports)
                    .filter_map(|(v, &p)| {
                        test_bits(v, &chip.inputs[p].name, chip.inputs[p].width, true, diags).flatten().map(|b| (p, b))
                    })
                    .collect();
                let checks = row
                    .outputs
                    .iter()
                    .zip(&output_ports)
                    .filter_map(|(v, &p)| {
                        let port = &chip.outputs[p];
                        test_bits(v, &port.name, port.width, false, diags).map(|b| (port.name.clone(), b))
                    })
                    .collect();
                steps.push(TestStepInfo::Row { span: row.span, inputs, checks });
            }
            ast::TestStep::Set { values, span } => {
                let mut inputs = Vec::new();
                for (ident, v) in values {
                    match chip.inputs.iter().position(|p| p.name == ident.name) {
                        Some(p) => {
                            if let Some(Some(bits)) = test_bits(v, &ident.name, chip.inputs[p].width, true, diags) {
                                inputs.push((p, bits));
                            }
                        }
                        None => {
                            let names: Vec<String> = chip.inputs.iter().map(|p| p.name.clone()).collect();
                            let mut d =
                                Diagnostic::error(ident.span, format!("`{}` has no input `{}`", chip.name, ident.name))
                                    .note(format!("`set` changes inputs; its inputs are {}", list(&names)));
                            if let Some(close) = suggest(&ident.name, names.iter().map(String::as_str)) {
                                d = d.label(format!("did you mean `{close}`?"));
                            }
                            diags.push(d);
                        }
                    }
                }
                steps.push(TestStepInfo::Set { span: *span, inputs });
            }
            ast::TestStep::Tick { count, span } => steps.push(TestStepInfo::Tick { span: *span, count: *count }),
            ast::TestStep::Expect { values, span } => {
                let mut checks = Vec::new();
                for (ident, v) in values {
                    match checkable(&ident.name) {
                        Some(width) => {
                            if let Some(expected) = test_bits(v, &ident.name, width, false, diags) {
                                checks.push((ident.name.clone(), expected));
                            }
                        }
                        None => {
                            let mut d = Diagnostic::error(
                                ident.span,
                                format!("there's nothing named `{}` to check", ident.name),
                            )
                            .note("`expect` checks outputs, inputs, wires, or a box with one output");
                            if let Some(close) = suggest(&ident.name, chip.names.keys().map(String::as_str)) {
                                d = d.label(format!("did you mean `{close}`?"));
                            }
                            diags.push(d);
                        }
                    }
                }
                steps.push(TestStepInfo::Expect { span: *span, checks });
            }
        }
    }

    if count_errors(diags) > errors_before {
        return None;
    }
    Some(TestInfo { chip: chip.name.clone(), span: def.span, steps })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(text: &str) -> (Design, Vec<String>) {
        let project = Project::from_sources(vec![("test.chip".into(), text.into())]);
        assert!(project.diagnostics.is_empty(), "{:#?}", project.diagnostics);
        let design = Design::check(&project);
        let messages = design.diagnostics.iter().map(|d| d.message.clone()).collect();
        (design, messages)
    }

    #[test]
    fn half_adder_is_clean() {
        let (design, messages) = check(
            "chip HalfAdder {\n in a, b\n out sum, carry\n sum = Xor(a, b)\n carry = And(a, b)\n}\n\
             test HalfAdder {\n 0 0 | 0 0\n 1 1 | 0 1\n}",
        );
        assert!(messages.is_empty(), "{messages:?}");
        assert!(design.is_buildable("HalfAdder"));
        assert_eq!(design.tests[0].steps.len(), 2);
    }

    #[test]
    fn gates_take_the_width_of_their_inputs() {
        let (design, messages) = check("chip A {\n in a[8], b[8]\n out y[8]\n y = And(a, b)\n}");
        assert!(messages.is_empty(), "{messages:?}");
        assert_eq!(design.chips["A"].instances[0].callee, Callee::Gate { kind: GateKind::And, width: 8, addr: 0 });
    }

    #[test]
    fn reports_common_mistakes() {
        let (_, messages) = check(
            "chip A {\n in a, b\n out y, z\n y = Xro(a, b)\n z = And(a, q)\n}\n\
             chip B {\n in a[4]\n out y\n y = Not(a)\n}\n\
             chip C {\n in a\n out y\n y = Not(a)\n y = a\n}",
        );
        assert!(messages.iter().any(|m| m == "there's no chip named `Xro`"), "{messages:?}");
        assert!(messages.iter().any(|m| m == "there's no signal named `q`"), "{messages:?}");
        assert!(messages.iter().any(|m| m == "`y` is 1 bit, but `Not` outputs 4"), "{messages:?}");
        assert!(messages.iter().any(|m| m == "`y` is driven twice"), "{messages:?}");
    }

    #[test]
    fn multi_output_chips_need_a_port() {
        let (_, messages) = check(
            "chip Two {\n in a\n out p, q\n p = Not(a)\n q = a\n}\n\
             chip User {\n in a\n out y\n t = Two(a)\n y = t\n}",
        );
        assert!(messages.iter().any(|m| m == "`t` has 2 outputs"), "{messages:?}");
    }

    #[test]
    fn recursion_is_caught() {
        let (design, messages) = check("chip A {\n in a\n out y\n y = A(a)\n}");
        assert!(messages.iter().any(|m| m == "`A` can't contain itself"), "{messages:?}");
        assert!(!design.is_buildable("A"));
    }

    #[test]
    fn undriven_outputs_are_reported() {
        let (_, messages) = check("chip A {\n in a\n out y[4]\n y[1:0] = {a, a}\n}");
        assert!(messages.iter().any(|m| m == "bits 3..2 of output `y` are never driven"), "{messages:?}");
    }
}
