//! Flattens a chip, and every chip inside it, into one-bit gates connected
//! by numbered nets. This is what the simulator runs.

use std::fmt;
use std::sync::Arc;

use crate::ast::{Expr, ExprKind};
use crate::check::{Callee, ChipInfo, Design, Name};
use crate::gate::{GateKind, PortWidth};
use crate::logic::{Logic, bits_from_u64};

pub type NetId = u32;

/// A one-bit combinational gate. Gates with fewer inputs repeat `a`; only
/// `Mux` reads `c` (its select).
#[derive(Clone, Copy, Debug)]
pub struct Gate {
    pub kind: GateKind,
    pub a: NetId,
    pub b: NetId,
    pub c: NetId,
    pub out: NetId,
}

/// A one-bit flip-flop: `q` takes `d`'s value at each clock tick.
#[derive(Clone, Copy, Debug)]
pub struct Flop {
    pub d: NetId,
    pub q: NetId,
}

/// A block of memory. `out` always shows the word at `addr`; on a clock
/// tick with `write` at 1, `data` is stored there.
#[derive(Clone, Debug)]
pub struct RamBlock {
    /// Where it is, like `cpu.ram`.
    pub path: String,
    pub addr: Vec<NetId>,
    /// Its width comes from here. A Rom's are nets nothing drives.
    pub data: Vec<NetId>,
    /// A Rom's is a constant 0.
    pub write: NetId,
    pub out: Vec<NetId>,
    /// For a Rom: the words it starts with (and keeps, since nothing writes).
    pub rom: Option<Arc<Vec<u64>>>,
}

/// A shared wire bit with several drivers (`data <- TriBuf(...)` lines).
/// Drivers letting go (Z) don't count; two driving at once makes X.
#[derive(Clone, Debug)]
pub struct Wired {
    pub drivers: Vec<NetId>,
    pub out: NetId,
}

/// A named signal and its nets, lowest bit first. Paths look like `sum` or
/// `fa0.ab` for signals inside other chips.
#[derive(Clone, Debug)]
pub struct Signal {
    pub path: String,
    pub nets: Vec<NetId>,
    /// How many chips deep it is: 0 for the chip being simulated.
    pub depth: usize,
}

#[derive(Clone, Debug)]
pub struct Netlist {
    pub chip: String,
    pub net_count: usize,
    pub gates: Vec<Gate>,
    pub flops: Vec<Flop>,
    pub rams: Vec<RamBlock>,
    pub wired: Vec<Wired>,
    pub constants: Vec<(NetId, Logic)>,
    pub inputs: Vec<Signal>,
    pub outputs: Vec<Signal>,
    /// Every named signal in the hierarchy, outer chips first. A chip's
    /// ports appear twice: named from outside (`h1.sum`, at the outer
    /// chip's depth) and from inside (its own port `sum`, one level deeper),
    /// so each level can list exactly its own signals.
    pub signals: Vec<Signal>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildError {
    UnknownChip(String),
    HasErrors(String),
    TooDeep(String),
    /// Two drivers ended up on the same net. The checker should prevent this.
    ShortCircuit(String),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::UnknownChip(chip) => write!(f, "there's no chip named `{chip}`"),
            BuildError::HasErrors(chip) => {
                write!(f, "`{chip}` (or a chip inside it) has errors; `wafer check` lists them")
            }
            BuildError::TooDeep(chip) => write!(f, "`{chip}` nests chips more than 64 levels deep"),
            BuildError::ShortCircuit(signal) => write!(f, "`{signal}` has more than one driver"),
        }
    }
}

impl std::error::Error for BuildError {}

impl Netlist {
    pub fn build(design: &Design, chip: &str) -> Result<Netlist, BuildError> {
        let Some(info) = design.chips.get(chip) else {
            return Err(BuildError::UnknownChip(chip.into()));
        };
        if !design.is_buildable(chip) {
            return Err(BuildError::HasErrors(chip.into()));
        }
        let mut b = Builder {
            design,
            parent: Vec::new(),
            gates: Vec::new(),
            flops: Vec::new(),
            rams: Vec::new(),
            wired: Vec::new(),
            constants: Vec::new(),
            signals: Vec::new(),
        };
        let inputs: Vec<Vec<NetId>> = info.inputs.iter().map(|p| b.fresh(p.width)).collect();
        let outputs: Vec<Vec<NetId>> = info.outputs.iter().map(|p| b.fresh(p.width)).collect();
        b.instantiate(chip, "", &inputs, &outputs, 0)?;

        // Merge connected nets and renumber them 0..n.
        let roots: Vec<NetId> = (0..b.parent.len() as NetId).map(|n| b.find(n)).collect();
        let mut compact = vec![NetId::MAX; roots.len()];
        let mut net_count = 0;
        for &root in &roots {
            if compact[root as usize] == NetId::MAX {
                compact[root as usize] = net_count;
                net_count += 1;
            }
        }
        let map = |net: NetId| compact[roots[net as usize] as usize];
        let map_signal = |s: &Signal| Signal { nets: s.nets.iter().map(|&n| map(n)).collect(), ..s.clone() };
        let signal = |name: &str, nets: &[NetId]| Signal {
            path: name.into(),
            nets: nets.iter().map(|&n| map(n)).collect(),
            depth: 0,
        };

        let netlist = Netlist {
            chip: chip.into(),
            net_count: net_count as usize,
            gates: b
                .gates
                .iter()
                .map(|g| Gate { a: map(g.a), b: map(g.b), c: map(g.c), out: map(g.out), ..*g })
                .collect(),
            flops: b.flops.iter().map(|f| Flop { d: map(f.d), q: map(f.q) }).collect(),
            rams: b
                .rams
                .iter()
                .map(|r| RamBlock {
                    path: r.path.clone(),
                    addr: r.addr.iter().map(|&n| map(n)).collect(),
                    data: r.data.iter().map(|&n| map(n)).collect(),
                    write: map(r.write),
                    out: r.out.iter().map(|&n| map(n)).collect(),
                    rom: r.rom.clone(),
                })
                .collect(),
            wired: b
                .wired
                .iter()
                .map(|w| Wired { drivers: w.drivers.iter().map(|&n| map(n)).collect(), out: map(w.out) })
                .collect(),
            constants: b.constants.iter().map(|&(n, v)| (map(n), v)).collect(),
            inputs: info.inputs.iter().zip(&inputs).map(|(p, nets)| signal(&p.name, nets)).collect(),
            outputs: info.outputs.iter().zip(&outputs).map(|(p, nets)| signal(&p.name, nets)).collect(),
            signals: b.signals.iter().map(map_signal).collect(),
        };
        netlist.check_drivers()?;
        Ok(netlist)
    }

    /// The signals one level of the hierarchy names: `prefix` is `""` for
    /// the chip itself or like `fa0.h1.` for a chip inside it.
    pub fn signals_at<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a Signal> + 'a {
        let depth = prefix.matches('.').count();
        self.signals.iter().filter(move |s| s.depth == depth && s.path.starts_with(prefix))
    }

    pub fn signal(&self, path: &str) -> Option<&Signal> {
        self.signals.iter().find(|s| s.path == path)
    }

    fn check_drivers(&self) -> Result<(), BuildError> {
        let mut drivers = vec![0u8; self.net_count];
        let driven = self
            .gates
            .iter()
            .map(|g| g.out)
            .chain(self.flops.iter().map(|f| f.q))
            .chain(self.rams.iter().flat_map(|r| r.out.iter().copied()))
            .chain(self.wired.iter().map(|w| w.out))
            .chain(self.constants.iter().map(|&(n, _)| n))
            .chain(self.inputs.iter().flat_map(|s| s.nets.iter().copied()));
        for net in driven {
            drivers[net as usize] = drivers[net as usize].saturating_add(1);
            if drivers[net as usize] > 1 {
                let name = self
                    .signals
                    .iter()
                    .find(|s| s.nets.contains(&net))
                    .map_or_else(|| format!("net {net}"), |s| s.path.clone());
                return Err(BuildError::ShortCircuit(name));
            }
        }
        Ok(())
    }
}

struct Builder<'a> {
    design: &'a Design,
    /// Union-find over raw nets: connecting two names merges their nets.
    parent: Vec<NetId>,
    gates: Vec<Gate>,
    flops: Vec<Flop>,
    rams: Vec<RamBlock>,
    wired: Vec<Wired>,
    constants: Vec<(NetId, Logic)>,
    signals: Vec<Signal>,
}

/// The nets of every name inside one copy of a chip.
struct Scope<'s> {
    design: &'s Design,
    info: &'s ChipInfo,
    inputs: &'s [Vec<NetId>],
    outputs: &'s [Vec<NetId>],
    wires: &'s [Vec<NetId>],
    /// Per box, per output port.
    instance_outputs: &'s [Vec<Vec<NetId>>],
}

impl Scope<'_> {
    fn name_nets(&self, name: Name) -> Vec<NetId> {
        match name {
            Name::Input(i) => self.inputs[i].clone(),
            Name::Output(i) => self.outputs[i].clone(),
            Name::Wire(i) => self.wires[i].clone(),
            Name::Instance(k) => self.instance_outputs[k][0].clone(),
        }
    }

    fn ref_nets(&self, base: &str, port: Option<&str>) -> Vec<NetId> {
        match (self.info.names[base], port) {
            (Name::Instance(k), Some(port)) => {
                let sig = self.design.signature(&self.info.instances[k].callee);
                let index = sig.outputs.iter().position(|(n, _)| n == port).expect("checked");
                self.instance_outputs[k][index].clone()
            }
            (name, _) => self.name_nets(name),
        }
    }
}

impl Builder<'_> {
    fn fresh(&mut self, width: usize) -> Vec<NetId> {
        (0..width)
            .map(|_| {
                let id = self.parent.len() as NetId;
                self.parent.push(id);
                id
            })
            .collect()
    }

    fn find(&mut self, mut net: NetId) -> NetId {
        while self.parent[net as usize] != net {
            let grandparent = self.parent[self.parent[net as usize] as usize];
            self.parent[net as usize] = grandparent;
            net = grandparent;
        }
        net
    }

    fn union(&mut self, a: NetId, b: NetId) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[a as usize] = b;
        }
    }

    fn instantiate(
        &mut self,
        chip: &str,
        prefix: &str,
        inputs: &[Vec<NetId>],
        outputs: &[Vec<NetId>],
        depth: usize,
    ) -> Result<(), BuildError> {
        if depth > 64 {
            return Err(BuildError::TooDeep(chip.into()));
        }
        let design = self.design;
        let info = &design.chips[chip];
        let wires: Vec<Vec<NetId>> = info.wires.iter().map(|w| self.fresh(w.width)).collect();
        let mut instance_outputs = Vec::with_capacity(info.instances.len());
        for inst in &info.instances {
            // A shared wire's drivers each get their own nets, joined below.
            let nets = match inst.drives {
                Some(Name::Output(i)) if !inst.shared => vec![outputs[i].clone()],
                Some(Name::Wire(i)) if !inst.shared => vec![wires[i].clone()],
                _ => design.signature(&inst.callee).outputs.iter().map(|(_, w)| self.fresh(*w)).collect(),
            };
            instance_outputs.push(nets);
        }
        let scope = Scope { design, info, inputs, outputs, wires: &wires, instance_outputs: &instance_outputs };
        self.name_signals(&scope, prefix, depth);

        for assign in &info.assigns {
            let mut target = scope.name_nets(assign.target);
            if let Some(select) = assign.select {
                target = target[select.lo..=select.hi].to_vec();
            }
            let source = self.resolve(&scope, &assign.value, Some(target.len()));
            for (&t, &s) in target.iter().zip(&source) {
                self.union(t, s);
            }
        }

        for (k, inst) in info.instances.iter().enumerate() {
            let sig = design.signature(&inst.callee);
            let args: Vec<Vec<NetId>> = inst
                .inputs
                .iter()
                .zip(&sig.inputs)
                .map(|(expr, (_, width))| match expr {
                    Some(expr) => self.resolve(&scope, expr, Some(*width)),
                    // Unconnected: nets nothing drives, so they read as Z.
                    None => self.fresh(*width),
                })
                .collect();
            let out = &instance_outputs[k];
            match &inst.callee {
                Callee::Gate { kind: GateKind::Dff, width, .. } => {
                    for bit in 0..*width {
                        self.flops.push(Flop { d: args[0][bit], q: out[0][bit] });
                    }
                }
                Callee::Gate { kind: GateKind::Ram, .. } => self.rams.push(RamBlock {
                    path: format!("{prefix}{}", inst.name),
                    addr: args[0].clone(),
                    data: args[1].clone(),
                    write: args[2][0],
                    out: out[0].clone(),
                    rom: None,
                }),
                Callee::Gate { kind: GateKind::Rom, width, .. } => {
                    let words =
                        inst.program.as_ref().and_then(|p| design.programs.get(&p.path)).map(|p| p.words.clone());
                    let write = self.fresh(1)[0];
                    self.constants.push((write, Logic::Zero));
                    let data = self.fresh(*width);
                    self.rams.push(RamBlock {
                        path: format!("{prefix}{}", inst.name),
                        addr: args[0].clone(),
                        data,
                        write,
                        out: out[0].clone(),
                        rom: Some(words.unwrap_or_default()),
                    });
                }
                Callee::Gate { kind, width, .. } => {
                    for bit in 0..*width {
                        // One-bit inputs like `sel` feed every bit.
                        let pick = |port: usize| match args.get(port) {
                            Some(arg) if kind.port_width(port) == PortWidth::Bus => arg[bit],
                            Some(arg) => arg[0],
                            None => args[0][bit],
                        };
                        self.gates.push(Gate { kind: *kind, a: pick(0), b: pick(1), c: pick(2), out: out[0][bit] });
                    }
                }
                Callee::Chip { name: callee, copies } if *copies > 1 => {
                    // One copy per bit. A 1-bit input feeds every copy.
                    let inner = format!("{prefix}{}.", inst.name);
                    let start = self.signals.len();
                    for bit in 0..*copies {
                        let pick = |nets: &Vec<NetId>| vec![nets[if nets.len() == 1 { 0 } else { bit }]];
                        let ins: Vec<Vec<NetId>> = args.iter().map(pick).collect();
                        let outs: Vec<Vec<NetId>> = instance_outputs[k].iter().map(pick).collect();
                        self.instantiate(callee, &inner, &ins, &outs, depth + 1)?;
                    }
                    self.merge_copies(start, *copies);
                }
                Callee::Chip { name: callee, .. } => {
                    let inner = format!("{prefix}{}.", inst.name);
                    self.instantiate(callee, &inner, &args, &instance_outputs[k], depth + 1)?;
                }
                Callee::Unknown => unreachable!("unbuildable chips are rejected up front"),
            }
        }

        // Join each shared wire's drivers, bit by bit.
        let mut shared: Vec<(Name, Vec<usize>)> = Vec::new();
        for (k, inst) in info.instances.iter().enumerate() {
            if let (true, Some(target)) = (inst.shared, inst.drives) {
                match shared.iter_mut().find(|(t, _)| *t == target) {
                    Some((_, drivers)) => drivers.push(k),
                    None => shared.push((target, vec![k])),
                }
            }
        }
        for (target, drivers) in shared {
            let wire = scope.name_nets(target);
            for (bit, &out) in wire.iter().enumerate() {
                let drivers = drivers.iter().map(|&k| instance_outputs[k][0][bit]).collect();
                self.wired.push(Wired { drivers, out });
            }
        }
        Ok(())
    }

    /// The copies of a chip repeated across a bus each named the same
    /// signals inside it; join each one's bits (copy 0 is bit 0) so that
    /// inside, every signal is as wide as the bus.
    fn merge_copies(&mut self, start: usize, copies: usize) {
        let added = self.signals.split_off(start);
        let each = added.len() / copies;
        for i in 0..each {
            let mut signal = added[i].clone();
            for copy in 1..copies {
                signal.nets.extend_from_slice(&added[copy * each + i].nets);
            }
            self.signals.push(signal);
        }
    }

    fn name_signals(&mut self, scope: &Scope, prefix: &str, depth: usize) {
        let info = scope.info;
        let mut add = |path: String, nets: &Vec<NetId>| self.signals.push(Signal { path, nets: nets.clone(), depth });
        let ports = [(&info.inputs, scope.inputs), (&info.outputs, scope.outputs), (&info.wires, scope.wires)];
        for (ports, nets) in ports {
            for (port, nets) in ports.iter().zip(nets) {
                add(format!("{prefix}{}", port.name), nets);
            }
        }
        for (inst, outputs) in info.instances.iter().zip(scope.instance_outputs) {
            // Boxes that drive a port or wire share its name; shared-wire
            // drivers keep their own (`data_d1`) so each driver's value shows.
            if inst.drives.is_some() && !inst.shared {
                continue;
            }
            let sig = scope.design.signature(&inst.callee);
            if let [_] = sig.outputs.as_slice() {
                add(format!("{prefix}{}", inst.name), &outputs[0]);
            } else {
                for ((port, _), nets) in sig.outputs.iter().zip(outputs) {
                    add(format!("{prefix}{}.{port}", inst.name), nets);
                }
            }
        }
    }

    /// The nets an expression reads, lowest bit first. Numbers get fresh
    /// nets driven by constants.
    fn resolve(&mut self, scope: &Scope, expr: &Expr, width: Option<usize>) -> Vec<NetId> {
        match &expr.kind {
            ExprKind::Literal(lit) => {
                let width = width.unwrap_or(lit.natural_width());
                bits_from_u64(lit.value, width)
                    .into_iter()
                    .map(|bit| {
                        let net = self.fresh(1)[0];
                        self.constants.push((net, bit));
                        net
                    })
                    .collect()
            }
            // The checker rejects text anywhere but a Rom's `file:`.
            ExprKind::Str(_) => self.fresh(width.unwrap_or(1)),
            ExprKind::Concat(parts) => {
                let mut nets = Vec::new();
                for part in parts.iter().rev() {
                    nets.extend(self.resolve(scope, part, None));
                }
                nets
            }
            ExprKind::Ref { base, port, select } => {
                let nets = scope.ref_nets(&base.name, port.as_ref().map(|p| p.name.as_str()));
                match select {
                    Some(select) => nets[select.lo..=select.hi].to_vec(),
                    None => nets,
                }
            }
        }
    }
}
