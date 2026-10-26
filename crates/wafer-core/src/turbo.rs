//! Turbo mode: a chip's gates compiled to run fast. The gates are sorted
//! once so that evaluating each one in order settles the whole circuit, and
//! then every clock tick is one pass over them with plain 0s and 1s.
//!
//! Turbo keeps no X or Z: the normal [`Simulator`] is still what finds
//! wiring mistakes. It hands state back and forth with a simulator of the
//! same netlist: the flip-flops, the memories, the inputs and the tick
//! count. Everything else follows from those, so the simulator works the
//! rest out itself. That leaves turbo free to simplify the gates on the
//! way in: numbers are folded in, a Nand followed by a Not becomes one And,
//! and gates nothing reads are skipped.

use crate::gate::GateKind;
use crate::logic::{Logic, bits_from_u64};
use crate::netlist::{NetId, Netlist};
use crate::sim::{Simulator, Unsettled};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Nand,
    And,
    Or,
    Nor,
    Not,
    Xor,
    Xnor,
    Mux,
    /// A memory's read: `a` is which memory.
    Read,
}

impl Kind {
    /// The same gate with its output flipped, if there is one.
    fn inverse(self) -> Option<Kind> {
        Some(match self {
            Kind::Nand => Kind::And,
            Kind::And => Kind::Nand,
            Kind::Or => Kind::Nor,
            Kind::Nor => Kind::Or,
            Kind::Xor => Kind::Xnor,
            Kind::Xnor => Kind::Xor,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct Op {
    kind: Kind,
    a: u32,
    b: u32,
    c: u32,
    out: u32,
}

#[derive(Clone, Debug)]
struct Memory {
    addr: Vec<u32>,
    data: Vec<u32>,
    write: u32,
    out: Vec<u32>,
    words: Vec<u64>,
    /// A Rom: nothing writes it, so it isn't handed back.
    rom: bool,
}

/// A chip compiled for turbo.
#[derive(Clone, Debug)]
pub struct Turbo {
    /// The gates (and memory reads) in an order that settles the circuit.
    ops: Vec<Op>,
    /// (d, q) of every flip-flop. `d` is where its value comes from after
    /// simplifying; `q` is the netlist's own net.
    flops: Vec<(u32, u32)>,
    memories: Vec<Memory>,
    /// One byte per net, 0 or 1, plus two nets that are always 0 and 1.
    values: Vec<u8>,
    /// Where each of the netlist's nets can be read after simplifying.
    alias: Vec<u32>,
    inputs: Vec<Vec<u32>>,
    next: Vec<u8>,
    ticks: u64,
}

/// What a net is, while compiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Term {
    Zero,
    One,
    Net(u32),
}

/// The state turbo hands to a simulator, and takes from one.
#[derive(Clone, Debug)]
pub struct Snapshot {
    ticks: u64,
    /// Inputs and flip-flops.
    nets: Vec<(NetId, Logic)>,
    /// Memories that can be written: (index in the netlist, words).
    memories: Vec<(usize, Vec<u64>)>,
}

impl Snapshot {
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// Puts this state into `sim`, which must be built from the same
    /// netlist, and lets it work out every other wire.
    pub fn apply(&self, sim: &mut Simulator) -> Result<u64, Unsettled> {
        sim.restore(&self.nets, &self.memories, self.ticks);
        sim.settle(crate::testing::MAX_STEPS)
    }
}

impl Turbo {
    /// Compiles `netlist`. Fails when the circuit can't be sorted: a loop of
    /// gates with no flip-flop in it, or a wire with several drivers.
    pub fn compile(netlist: &Netlist) -> Result<Turbo, String> {
        if !netlist.wired.is_empty() {
            return Err("it has shared wires (TriBuf lines), which turbo can't run".into());
        }
        let nets = netlist.net_count;
        let (zero, one) = (nets as u32, nets as u32 + 1);
        let order = sort(netlist)?;

        // Simplify, in order: each net becomes a number, another net, or
        // the output of a (possibly new) op.
        let mut term: Vec<Term> = (0..nets as u32).map(Term::Net).collect();
        for &(net, value) in &netlist.constants {
            term[net as usize] = if value == Logic::One { Term::One } else { Term::Zero };
        }
        // Back from a net turbo uses to what it is (its two number nets).
        let of = |n: u32| match n {
            n if n == zero => Term::Zero,
            n if n == one => Term::One,
            n => Term::Net(n),
        };
        // Which op drives a net, for spotting a Not after a Nand and so on.
        let mut made_by: Vec<u32> = vec![u32::MAX; nets];
        let mut ops: Vec<Op> = Vec::new();
        for node in order {
            let gate = match node {
                Node::Memory(r) => {
                    ops.push(Op { kind: Kind::Read, a: r as u32, b: 0, c: 0, out: 0 });
                    continue;
                }
                Node::Gate(g) => netlist.gates[g],
            };
            let (a, b, c) = (term[gate.a as usize], term[gate.b as usize], term[gate.c as usize]);
            let (kind, x, y, z) = match simplify(gate.kind, a, b, c) {
                Simple::Is(t) => {
                    term[gate.out as usize] = t;
                    continue;
                }
                Simple::Op(kind, x, y, z) => (kind, x, y, z),
            };
            let made = |t: Term| match t {
                Term::Net(n) if made_by[n as usize] != u32::MAX => Some(ops[made_by[n as usize] as usize]),
                _ => None,
            };
            let (kind, x, y, z) = match (kind, made(x), made(y)) {
                // Not(Not(p)) is p.
                (Kind::Not, Some(Op { kind: Kind::Not, a, .. }), _) => {
                    term[gate.out as usize] = of(a);
                    continue;
                }
                // Not(Nand(p, q)) is And(p, q), and so on.
                (Kind::Not, Some(inner), _) if inner.kind.inverse().is_some() => {
                    (inner.kind.inverse().expect("checked"), of(inner.a), of(inner.b), Term::Zero)
                }
                // Nand(Not(p), Not(q)) is Or(p, q).
                (Kind::Nand, Some(Op { kind: Kind::Not, a: p, .. }), Some(Op { kind: Kind::Not, a: q, .. })) => {
                    (Kind::Or, of(p), of(q), Term::Zero)
                }
                _ => (kind, x, y, z),
            };
            made_by[gate.out as usize] = ops.len() as u32;
            ops.push(Op {
                kind,
                a: as_net(x, zero, one),
                b: as_net(y, zero, one),
                c: as_net(z, zero, one),
                out: gate.out,
            });
        }

        // Now every net's final form is known.
        let alias: Vec<u32> = term.iter().map(|&t| as_net(t, zero, one)).collect();
        let at = |nets: &[NetId]| nets.iter().map(|&n| alias[n as usize]).collect::<Vec<u32>>();
        let flops: Vec<(u32, u32)> = netlist.flops.iter().map(|f| (alias[f.d as usize], f.q)).collect();
        let memories: Vec<Memory> = (netlist.rams.iter())
            .map(|ram| Memory {
                addr: at(&ram.addr),
                data: at(&ram.data),
                write: alias[ram.write as usize],
                out: ram.out.clone(),
                words: Vec::new(),
                rom: ram.rom.is_some(),
            })
            .collect();

        // Keep only ops something needs: flip-flops, memories, or the
        // chip's outputs. Memory reads always stay.
        let mut needed = vec![false; nets + 2];
        for &(d, _) in &flops {
            needed[d as usize] = true;
        }
        for m in &memories {
            for &n in m.addr.iter().chain(&m.data).chain([&m.write]) {
                needed[n as usize] = true;
            }
        }
        for output in &netlist.outputs {
            for &n in &output.nets {
                needed[alias[n as usize] as usize] = true;
            }
        }
        let mut keep = vec![false; ops.len()];
        for (i, op) in ops.iter().enumerate().rev() {
            if op.kind != Kind::Read && !needed[op.out as usize] {
                continue;
            }
            keep[i] = true;
            match op.kind {
                Kind::Read => {}
                Kind::Not => needed[op.a as usize] = true,
                Kind::Mux => {
                    for n in [op.a, op.b, op.c] {
                        needed[n as usize] = true;
                    }
                }
                _ => {
                    needed[op.a as usize] = true;
                    needed[op.b as usize] = true;
                }
            }
        }
        let ops: Vec<Op> = ops.into_iter().zip(keep).filter(|(_, k)| *k).map(|(op, _)| op).collect();

        let mut values = vec![0; nets + 2];
        values[one as usize] = 1;
        let inputs = netlist.inputs.iter().map(|s| s.nets.clone()).collect();
        let next = vec![0; flops.len()];
        Ok(Turbo { ops, flops, memories, values, alias, inputs, next, ticks: 0 })
    }

    /// Ops left after simplifying: how much work a tick is.
    pub fn op_count(&self) -> usize {
        self.ops.len()
    }

    /// Takes the state of `sim` (built from the same netlist): inputs,
    /// flip-flops, memories and the tick count. Unknown bits become 0.
    pub fn load(&mut self, sim: &Simulator) {
        let bit = |v: Logic| u8::from(v == Logic::One);
        for nets in &self.inputs {
            for &n in nets {
                self.values[n as usize] = bit(sim.value(n));
            }
        }
        for &(_, q) in &self.flops {
            self.values[q as usize] = bit(sim.value(q));
        }
        for (i, m) in self.memories.iter_mut().enumerate() {
            let width = m.out.len().max(1);
            m.words = sim
                .memory(i)
                .chunks(width)
                .map(|w| w.iter().enumerate().map(|(b, &v)| u64::from(bit(v)) << b).sum())
                .collect();
        }
        self.ticks = sim.ticks();
        self.settle();
    }

    /// The state to hand back to a simulator.
    pub fn snapshot(&self) -> Snapshot {
        let mut nets = Vec::new();
        for input in &self.inputs {
            nets.extend(input.iter().map(|&n| (n, Logic::from_bool(self.values[n as usize] == 1))));
        }
        nets.extend(self.flops.iter().map(|&(_, q)| (q, Logic::from_bool(self.values[q as usize] == 1))));
        let memories =
            (self.memories.iter().enumerate()).filter(|(_, m)| !m.rom).map(|(i, m)| (i, m.words.clone())).collect();
        Snapshot { ticks: self.ticks, nets, memories }
    }

    /// Sets the chip's input `index` (in the netlist's order) to `value`.
    pub fn set_input(&mut self, index: usize, value: u64) {
        let bits = bits_from_u64(value, self.inputs[index].len());
        for (i, &n) in self.inputs[index].iter().enumerate() {
            self.values[n as usize] = u8::from(bits[i] == Logic::One);
        }
    }

    /// Evaluates every op once, in order: afterwards the circuit is settled.
    pub fn settle(&mut self) {
        let Turbo { ops, memories, values, .. } = self;
        for op in ops.iter() {
            let (a, b) = (values[op.a as usize], values[op.b as usize]);
            let v = match op.kind {
                Kind::Nand => 1 ^ (a & b),
                Kind::And => a & b,
                Kind::Or => a | b,
                Kind::Nor => 1 ^ (a | b),
                Kind::Not => 1 ^ a,
                Kind::Xor => a ^ b,
                Kind::Xnor => 1 ^ a ^ b,
                Kind::Mux => {
                    if values[op.c as usize] == 1 {
                        b
                    } else {
                        a
                    }
                }
                Kind::Read => {
                    let m = &memories[op.a as usize];
                    let addr =
                        m.addr.iter().enumerate().fold(0, |acc, (i, &n)| acc | (values[n as usize] as usize) << i);
                    let word = m.words.get(addr).copied().unwrap_or(0);
                    for (i, &n) in m.out.iter().enumerate() {
                        values[n as usize] = (word >> i) as u8 & 1;
                    }
                    continue;
                }
            };
            values[op.out as usize] = v;
        }
    }

    /// The clock edge: memories store writes and flip-flops take their
    /// inputs, all from the settled values.
    pub fn clock_edge(&mut self) {
        let Turbo { memories, values, flops, next, .. } = self;
        for m in memories.iter_mut().filter(|m| values[m.write as usize] == 1) {
            let addr = m.addr.iter().enumerate().fold(0, |acc, (i, &n)| acc | (values[n as usize] as usize) << i);
            let word = m.data.iter().enumerate().fold(0, |acc, (i, &n)| acc | u64::from(values[n as usize]) << i);
            if let Some(slot) = m.words.get_mut(addr) {
                *slot = word;
            }
        }
        for (slot, &(d, _)) in next.iter_mut().zip(flops.iter()) {
            *slot = values[d as usize];
        }
        for (&v, &(_, q)) in next.iter().zip(flops.iter()) {
            values[q as usize] = v;
        }
        self.ticks += 1;
    }

    /// Settles and ticks, like [`Simulator::tick`].
    pub fn tick(&mut self) {
        self.settle();
        self.clock_edge();
    }

    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// A net of the netlist as it was at the last settle.
    pub fn bit(&self, net: NetId) -> bool {
        self.values[self.alias[net as usize] as usize] == 1
    }

    /// Several nets as a number, lowest bit first.
    pub fn read(&self, nets: &[NetId]) -> u64 {
        nets.iter().enumerate().fold(0, |acc, (i, &n)| acc | u64::from(self.bit(n)) << i)
    }

    /// A memory's words, by its index in the netlist.
    pub fn memory(&self, ram: usize) -> Option<&[u64]> {
        Some(&self.memories.get(ram)?.words)
    }
}

fn as_net(t: Term, zero: u32, one: u32) -> u32 {
    match t {
        Term::Zero => zero,
        Term::One => one,
        Term::Net(n) => n,
    }
}

enum Simple {
    /// The gate's output is just this.
    Is(Term),
    Op(Kind, Term, Term, Term),
}

/// A gate with numbers and repeats folded in.
fn simplify(kind: GateKind, a: Term, b: Term, c: Term) -> Simple {
    use Term::{One, Zero};
    let not = |t: Term| match t {
        Zero => Simple::Is(One),
        One => Simple::Is(Zero),
        t => Simple::Op(Kind::Not, t, Zero, Zero),
    };
    match kind {
        GateKind::Not => not(a),
        GateKind::Nand => match (a, b) {
            (Zero, _) | (_, Zero) => Simple::Is(One),
            (One, t) | (t, One) => not(t),
            (x, y) if x == y => not(x),
            (x, y) => Simple::Op(Kind::Nand, x, y, Zero),
        },
        GateKind::And => match (a, b) {
            (Zero, _) | (_, Zero) => Simple::Is(Zero),
            (One, t) | (t, One) => Simple::Is(t),
            (x, y) if x == y => Simple::Is(x),
            (x, y) => Simple::Op(Kind::And, x, y, Zero),
        },
        GateKind::Or => match (a, b) {
            (One, _) | (_, One) => Simple::Is(One),
            (Zero, t) | (t, Zero) => Simple::Is(t),
            (x, y) if x == y => Simple::Is(x),
            (x, y) => Simple::Op(Kind::Or, x, y, Zero),
        },
        GateKind::Nor => match (a, b) {
            (One, _) | (_, One) => Simple::Is(Zero),
            (Zero, t) | (t, Zero) => not(t),
            (x, y) if x == y => not(x),
            (x, y) => Simple::Op(Kind::Nor, x, y, Zero),
        },
        GateKind::Xor => match (a, b) {
            (Zero, t) | (t, Zero) => Simple::Is(t),
            (One, t) | (t, One) => not(t),
            (x, y) if x == y => Simple::Is(Zero),
            (x, y) => Simple::Op(Kind::Xor, x, y, Zero),
        },
        GateKind::Xnor => match (a, b) {
            (One, t) | (t, One) => Simple::Is(t),
            (Zero, t) | (t, Zero) => not(t),
            (x, y) if x == y => Simple::Is(One),
            (x, y) => Simple::Op(Kind::Xnor, x, y, Zero),
        },
        GateKind::Mux => match c {
            Zero => Simple::Is(a),
            One => Simple::Is(b),
            _ if a == b => Simple::Is(a),
            s => match (a, b) {
                // Mux(0, 1, s) is s; Mux(1, 0, s) is Not s.
                (Zero, One) => Simple::Is(s),
                (One, Zero) => not(s),
                (x, y) => Simple::Op(Kind::Mux, x, y, s),
            },
        },
        // A TriBuf with only itself driving the wire just passes `a` on.
        GateKind::TriBuf => Simple::Is(a),
        GateKind::Dff | GateKind::Ram | GateKind::Rom => Simple::Is(Zero),
    }
}

#[derive(Clone, Copy, Debug)]
enum Node {
    Gate(usize),
    Memory(usize),
}

/// The gates and memory reads in an order where each comes after
/// everything it reads.
fn sort(netlist: &Netlist) -> Result<Vec<Node>, String> {
    let (g, r) = (netlist.gates.len(), netlist.rams.len());
    let mut driver = vec![usize::MAX; netlist.net_count];
    for (i, gate) in netlist.gates.iter().enumerate() {
        driver[gate.out as usize] = i;
    }
    for (i, ram) in netlist.rams.iter().enumerate() {
        for &o in &ram.out {
            driver[o as usize] = g + i;
        }
    }
    let reads = |n: usize| -> Vec<NetId> {
        if n < g {
            let gate = netlist.gates[n];
            match gate.kind {
                GateKind::Not | GateKind::TriBuf => vec![gate.a],
                GateKind::Mux => vec![gate.a, gate.b, gate.c],
                _ => vec![gate.a, gate.b],
            }
        } else {
            netlist.rams[n - g].addr.clone()
        }
    };
    let mut users = vec![Vec::new(); g + r];
    let mut waiting = vec![0usize; g + r];
    for n in 0..g + r {
        for net in reads(n) {
            let d = driver[net as usize];
            if d != usize::MAX {
                users[d].push(n);
                waiting[n] += 1;
            }
        }
    }
    let mut order: Vec<usize> = (0..g + r).filter(|&n| waiting[n] == 0).collect();
    let mut i = 0;
    while i < order.len() {
        for &u in &users[order[i]] {
            waiting[u] -= 1;
            if waiting[u] == 0 {
                order.push(u);
            }
        }
        i += 1;
    }
    if order.len() < g + r {
        // Name a signal in the loop, to help find it.
        let stuck = (0..g).find(|&n| waiting[n] > 0).map(|n| netlist.gates[n].out);
        let name = stuck
            .and_then(|net| netlist.signals.iter().find(|s| s.nets.contains(&net)))
            .map_or_else(String::new, |s| format!(" (through `{}`)", s.path));
        return Err(format!("it has a loop of gates with no flip-flop in it{name}, which turbo can't run"));
    }
    Ok(order.into_iter().map(|n| if n < g { Node::Gate(n) } else { Node::Memory(n - g) }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Design, Project};

    fn netlist(text: &str, chip: &str) -> Netlist {
        let project = Project::from_sources(vec![("t.chip".into(), text.into())]);
        let design = Design::check(&project);
        assert!(!design.has_errors(), "{:#?}", design.diagnostics);
        Netlist::build(&design, chip).unwrap()
    }

    #[test]
    fn counts_like_flip_flops_do() {
        let text = "chip C {\n in en\n out q0, q1\n n0 = Xor(q0, en)\n q0 = Dff(n0)\n t1 = And(q0, en)\n n1 = Xor(q1, t1)\n q1 = Dff(n1)\n}\n";
        let netlist = netlist(text, "C");
        let q = |name: &str| netlist.outputs.iter().find(|o| o.path == name).unwrap().nets[0];
        let (q0, q1) = (q("q0"), q("q1"));
        let mut turbo = Turbo::compile(&netlist).unwrap();
        turbo.set_input(0, 1);
        let mut seen = Vec::new();
        for _ in 0..5 {
            turbo.tick();
            turbo.settle();
            seen.push(u8::from(turbo.bit(q0)) + 2 * u8::from(turbo.bit(q1)));
        }
        assert_eq!(seen, [1, 2, 3, 0, 1]);
        assert_eq!(turbo.ticks(), 5);
    }

    #[test]
    fn a_loop_without_a_flip_flop_cant_be_compiled() {
        let latch = "chip L {\n in s, r\n out q\n q = Nand(s, qn)\n qn = Nand(r, q)\n}\n";
        let error = Turbo::compile(&netlist(latch, "L")).unwrap_err();
        assert!(error.contains("loop of gates with no flip-flop"), "{error}");
    }

    #[test]
    fn simplifying_keeps_the_answers() {
        // An And made of two Nands, an Or made of three, a Mux whose select
        // is a number, an Xor with 0, and an unused gate.
        let text = "chip F {\n in a, b\n out x, y, z\n n = Nand(a, b)\n x = Nand(n, n)\n \
                    na = Nand(a, a)\n nb = Nand(b, b)\n z = Nand(na, nb)\n m = Mux(a, b, 1)\n y = Xor(m, 0)\n \
                    spare = Nand(a, b)\n}\n";
        let netlist = netlist(text, "F");
        let mut turbo = Turbo::compile(&netlist).unwrap();
        assert_eq!(turbo.op_count(), 2, "x is And(a, b), z is Or(a, b), y is just b");
        let mut sim = Simulator::new(netlist.clone());
        for (a, b) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            sim.set_input(0, &bits_from_u64(a, 1));
            sim.set_input(1, &bits_from_u64(b, 1));
            sim.settle(1000).unwrap();
            turbo.set_input(0, a);
            turbo.set_input(1, b);
            turbo.settle();
            for name in ["x", "y", "z"] {
                let nets = &netlist.outputs.iter().find(|o| o.path == name).unwrap().nets;
                let want = crate::logic::bits_to_u64(&sim.values(nets));
                assert_eq!(Some(turbo.read(nets)), want, "{name} for a={a} b={b}");
            }
        }
    }

    #[test]
    fn state_goes_back_and_forth_with_a_simulator() {
        // A 2-bit counter with a reset, run partly in each.
        let text = "chip C {\n in en, reset\n out q0, q1\n x0 = Xor(q0, en)\n n0 = Mux(x0, 0, reset)\n q0 = Dff(n0)\n \
                    t1 = And(q0, en)\n x1 = Xor(q1, t1)\n n1 = Mux(x1, 0, reset)\n q1 = Dff(n1)\n}\n";
        let netlist = netlist(text, "C");
        let mut sim = Simulator::new(netlist.clone());
        sim.set_input(1, &bits_from_u64(1, 1));
        sim.tick(1000).unwrap();
        sim.set_input(1, &bits_from_u64(0, 1));
        sim.set_input(0, &bits_from_u64(1, 1));
        sim.tick(1000).unwrap();
        let mut turbo = Turbo::compile(&netlist).unwrap();
        turbo.load(&sim);
        turbo.tick();
        turbo.tick();
        let snapshot = turbo.snapshot();
        assert_eq!(snapshot.ticks(), 4);
        snapshot.apply(&mut sim).unwrap();
        assert_eq!(sim.ticks(), 4);
        let count =
            |sim: &Simulator| crate::logic::bits_to_u64(&[sim.read("q0").unwrap()[0], sim.read("q1").unwrap()[0]]);
        assert_eq!(count(&sim), Some(3), "1 tick in the simulator, then 2 in turbo");
        sim.tick(1000).unwrap();
        assert_eq!(count(&sim), Some(0), "and the simulator carries on from there");
    }
}
