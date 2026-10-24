//! Flattens a chip, and every chip inside it, into one-bit gates connected
//! by numbered nets. This is what the simulator runs.

use std::fmt;

use crate::ast::{Expr, ExprKind};
use crate::check::{Callee, ChipInfo, Design, Name};
use crate::gate::GateKind;
use crate::logic::{Logic, bits_from_u64};

pub type NetId = u32;

/// A one-bit gate. `Not` only reads `a`.
#[derive(Clone, Copy, Debug)]
pub struct Gate {
    pub kind: GateKind,
    pub a: NetId,
    pub b: NetId,
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
        let mut b =
            Builder { design, parent: Vec::new(), gates: Vec::new(), constants: Vec::new(), signals: Vec::new() };
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
            gates: b.gates.iter().map(|g| Gate { a: map(g.a), b: map(g.b), out: map(g.out), ..*g }).collect(),
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
            let nets = match inst.drives {
                Some(Name::Output(i)) => vec![outputs[i].clone()],
                Some(Name::Wire(i)) => vec![wires[i].clone()],
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
            match &inst.callee {
                Callee::Gate { kind, width } => {
                    for bit in 0..*width {
                        let a = args[0][bit];
                        let b = args.get(1).map_or(a, |arg| arg[bit]);
                        self.gates.push(Gate { kind: *kind, a, b, out: instance_outputs[k][0][bit] });
                    }
                }
                Callee::Chip(callee) => {
                    let inner = format!("{prefix}{}.", inst.name);
                    self.instantiate(callee, &inner, &args, &instance_outputs[k], depth + 1)?;
                }
                Callee::Unknown => unreachable!("unbuildable chips are rejected up front"),
            }
        }
        Ok(())
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
            if inst.drives.is_some() {
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
