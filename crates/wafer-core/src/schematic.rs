//! Lays a chip out for drawing. Inputs go on the left and outputs on the
//! right. Each box sits in a column by how many boxes lie between it and the
//! inputs. Wires are routed with right angles through channels between the
//! columns.
//!
//! This is the auto-layout used whenever a chip has no saved positions.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::ast::{Expr, ExprKind, Select};
use crate::check::{Callee, ChipInfo, Design, Name, Signature};
use crate::diag::SourceMap;
use crate::gate::GateKind;

pub const CHAR_W: f32 = 7.2;
pub const SMALL_CHAR_W: f32 = 6.6;
pub const TITLE_H: f32 = 80.0;
pub const MARGIN: f32 = 24.0;
pub const GATE_W: f32 = 66.0;
pub const GATE_H: f32 = 50.0;
pub const PORT_H: f32 = 24.0;
pub const CHIP_HEADER: f32 = 30.0;
const PIN_STEP: f32 = 20.0;
const DUMMY_H: f32 = 12.0;
const NODE_GAP: f32 = 26.0;
const DUMMY_GAP: f32 = 10.0;
const CHANNEL_STEP: f32 = 14.0;
const MIN_GAP: f32 = 64.0;
const LANE_STEP: f32 = 14.0;

#[derive(Clone, Debug)]
pub struct Schematic {
    pub chip: String,
    pub width: f32,
    pub height: f32,
    pub nodes: Vec<Node>,
    pub wires: Vec<Wire>,
    pub tags: Vec<Tag>,
}

#[derive(Clone, Debug)]
pub struct Node {
    /// Stable identity for saved positions: `in:a`, `out:sum`, or a box's name.
    pub key: String,
    pub kind: NodeKind,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub inputs: Vec<Pin>,
    pub outputs: Vec<Pin>,
}

/// A connection point, in absolute coordinates.
#[derive(Clone, Debug)]
pub struct Pin {
    pub name: String,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug)]
pub enum NodeKind {
    Input {
        name: String,
        width: usize,
    },
    Output {
        name: String,
        width: usize,
    },
    /// A built-in part. `label` is what a box-shaped part (Dff, Ram) shows,
    /// like `Ram 16×8`.
    Gate {
        name: String,
        kind: GateKind,
        label: String,
    },
    Chip {
        name: String,
        chip: String,
    },
}

/// Everything drawn for one signal: the segments from its source to every
/// pin it feeds, plus dots where it branches.
#[derive(Clone, Debug)]
pub struct Wire {
    /// The signal whose value colors this wire.
    pub signal: String,
    pub width: usize,
    pub segments: Vec<[f32; 4]>,
    pub dots: Vec<(f32, f32)>,
    /// Where to print a bus's value: just right of its source pin. Inputs
    /// already show their value in their box, so they have none.
    pub label_at: Option<(f32, f32)>,
    /// The output pin it starts at, as (node index, output pin index).
    pub source: (usize, usize),
    /// Every input pin it feeds, as (node index, input pin index).
    pub sinks: Vec<(usize, usize)>,
}

#[derive(Clone, Debug)]
pub struct Tag {
    /// The tag's right edge and vertical center.
    pub x: f32,
    pub y: f32,
    pub text: String,
    pub kind: TagKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagKind {
    /// A number wired straight into a pin.
    Constant,
    /// Which bits of a bus a wire carries, like `a[3]`.
    Bits,
}

/// A node's output pin, where a wire starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Source {
    node: usize,
    pin: usize,
}

/// One thing feeding a pin: a wire from a source or a constant, plus an
/// optional label saying which bits.
#[derive(Clone, Debug)]
struct Part {
    source: Option<Source>,
    constant: Option<String>,
    label: Option<String>,
}

struct Sink {
    node: usize,
    pin: usize,
    parts: Vec<Part>,
}

/// Size and pin offsets (relative to the node's top-left corner).
struct Geom {
    w: f32,
    h: f32,
    in_names: Vec<String>,
    in_off: Vec<f32>,
    out_names: Vec<String>,
    out_off: Vec<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Slot {
    Node(usize),
    /// A placeholder that carries a long wire through a column it skips.
    Dummy(usize),
}

struct Dummy {
    source: Source,
    layer: usize,
}

/// Two slots in neighboring columns joined by a wire, with the pin offsets
/// on each side.
struct Link {
    from: Slot,
    from_off: f32,
    to: Slot,
    to_off: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum End {
    Out(Source),
    In { node: usize, pin: usize },
    DummyLeft(usize),
    DummyRight(usize),
}

struct Trunk {
    source: Source,
    gap: usize,
    from: End,
    to: Vec<End>,
}

/// A chip's boxes, pins and connections, before anything is placed.
struct Graph<'a> {
    resolver: Resolver<'a>,
    kinds: Vec<NodeKind>,
    keys: Vec<String>,
    geoms: Vec<Geom>,
    sinks: Vec<Sink>,
    /// Every (source, sink index) connection, once each.
    pairs: Vec<(Source, usize)>,
    nin: usize,
    out_base: usize,
}

impl<'a> Graph<'a> {
    fn build(design: &'a Design, sources: &'a SourceMap, chip: &str) -> Option<Graph<'a>> {
        let info = design.chips.get(chip)?;
        let nin = info.inputs.len();
        let out_base = nin + info.instances.len();

        let mut kinds = Vec::new();
        let mut keys = Vec::new();
        let mut geoms = Vec::new();
        for port in &info.inputs {
            kinds.push(NodeKind::Input { name: port.name.clone(), width: port.width });
            keys.push(format!("in:{}", port.name));
            geoms.push(port_geom(port.width, true));
        }
        for inst in &info.instances {
            keys.push(inst.name.clone());
            match &inst.callee {
                Callee::Gate { kind, width, addr } => {
                    let label = match kind {
                        GateKind::Ram => format!("Ram {}×{width}", 1u64 << (*addr).min(63)),
                        GateKind::Rom => match &inst.program {
                            Some(program) => format!("Rom {}", program.name),
                            None => "Rom".into(),
                        },
                        GateKind::Dff if *width > 1 => format!("Dff ×{width}"),
                        _ => kind.name().to_string(),
                    };
                    let geom = if kind.is_box() {
                        chip_geom(&label, &design.signature(&inst.callee))
                    } else {
                        gate_geom(*kind)
                    };
                    kinds.push(NodeKind::Gate { name: inst.name.clone(), kind: *kind, label });
                    geoms.push(geom);
                }
                Callee::Chip(chip) => {
                    kinds.push(NodeKind::Chip { name: inst.name.clone(), chip: chip.clone() });
                    geoms.push(chip_geom(chip, &design.signature(&inst.callee)));
                }
                Callee::Unknown => {
                    kinds.push(NodeKind::Chip { name: inst.name.clone(), chip: "?".into() });
                    geoms.push(chip_geom("?", &Signature::default()));
                }
            }
        }
        for port in &info.outputs {
            kinds.push(NodeKind::Output { name: port.name.clone(), width: port.width });
            keys.push(format!("out:{}", port.name));
            geoms.push(port_geom(port.width, false));
        }

        // What feeds each pin.
        let resolver = Resolver { design, info, sources, nin };
        let mut sinks = Vec::new();
        for (k, inst) in info.instances.iter().enumerate() {
            for (pin, expr) in inst.inputs.iter().enumerate() {
                if let Some(expr) = expr {
                    sinks.push(Sink { node: nin + k, pin, parts: resolver.resolve(expr, 0) });
                }
            }
        }
        for j in 0..info.outputs.len() {
            sinks.push(Sink { node: out_base + j, pin: 0, parts: resolver.drivers(Name::Output(j), 0) });
        }
        let mut pairs: Vec<(Source, usize)> = Vec::new();
        for (si, sink) in sinks.iter().enumerate() {
            for part in &sink.parts {
                if let Some(source) = part.source
                    && !pairs.contains(&(source, si))
                {
                    pairs.push((source, si));
                }
            }
        }
        Some(Graph { resolver, kinds, keys, geoms, sinks, pairs, nin, out_base })
    }

    fn out_pin(&self, positions: &[(f32, f32)], source: Source) -> (f32, f32) {
        let (x, y) = positions[source.node];
        let g = &self.geoms[source.node];
        (x + g.w, y + g.out_off[source.pin])
    }

    fn in_pin(&self, positions: &[(f32, f32)], node: usize, pin: usize) -> (f32, f32) {
        let (x, y) = positions[node];
        (x, y + self.geoms[node].in_off[pin])
    }

    /// Turns placed boxes and routed segments into the finished schematic.
    fn assemble(
        &self,
        chip: &str,
        positions: &[(f32, f32)],
        segments: BTreeMap<Source, Vec<[f32; 4]>>,
        width: f32,
        height: f32,
    ) -> Schematic {
        let mut wires = Vec::new();
        for (source, segs) in segments {
            let (signal, bits) = self.resolver.source_signal(source);
            let (segments, dots) = tidy(segs);
            let (sx, sy) = self.out_pin(positions, source);
            let label_at = (source.node >= self.nin).then_some((sx + 6.0, sy - 7.0));
            let mut sinks: Vec<(usize, usize)> = Vec::new();
            for &(s, si) in &self.pairs {
                let target = (self.sinks[si].node, self.sinks[si].pin);
                if s == source && !sinks.contains(&target) {
                    sinks.push(target);
                }
            }
            wires.push(Wire {
                signal,
                width: bits,
                segments,
                dots,
                label_at,
                source: (source.node, source.pin),
                sinks,
            });
        }

        let mut nodes = Vec::new();
        for (v, &(x, y)) in positions.iter().enumerate() {
            let g = &self.geoms[v];
            let pins = |names: &[String], offs: &[f32], px: f32| -> Vec<Pin> {
                names.iter().zip(offs).map(|(name, off)| Pin { name: name.clone(), x: px, y: y + off }).collect()
            };
            nodes.push(Node {
                key: self.keys[v].clone(),
                kind: self.kinds[v].clone(),
                x,
                y,
                w: g.w,
                h: g.h,
                inputs: pins(&g.in_names, &g.in_off, x),
                outputs: pins(&g.out_names, &g.out_off, x + g.w),
            });
        }

        let mut tags = Vec::new();
        for sink in &self.sinks {
            let pin = &nodes[sink.node].inputs[sink.pin];
            let mut stacked = 0.0;
            for part in &sink.parts {
                if let Some(text) = &part.constant {
                    tags.push(Tag { x: pin.x - 2.0, y: pin.y, text: text.clone(), kind: TagKind::Constant });
                } else if let Some(text) = &part.label {
                    tags.push(Tag {
                        x: pin.x - 4.0,
                        y: pin.y - 8.0 - stacked,
                        text: text.clone(),
                        kind: TagKind::Bits,
                    });
                    stacked += 12.0;
                }
            }
        }
        Schematic { chip: chip.into(), width, height, nodes, wires, tags }
    }
}

/// Lays a chip out automatically in columns. Used whenever a chip has no
/// saved positions.
pub fn layout(design: &Design, sources: &SourceMap, chip: &str) -> Option<Schematic> {
    let graph = Graph::build(design, sources, chip)?;
    let info = graph.resolver.info;
    let (nin, out_base) = (graph.nin, graph.out_base);
    let (kinds, geoms, sinks, pairs) = (&graph.kinds, &graph.geoms, &graph.sinks, &graph.pairs);
    let n = kinds.len();

    // Columns: longest path from the inputs, ignoring feedback loops.
    let mut adj = vec![Vec::new(); n];
    for &(source, si) in pairs.iter() {
        let to = sinks[si].node;
        if !adj[source.node].contains(&to) {
            adj[source.node].push(to);
        }
    }
    let (order, back_edges) = depth_first(&adj, nin, n);
    let mut layer = vec![0usize; n];
    for v in nin..out_base {
        layer[v] = 1;
    }
    for &u in &order {
        for &v in &adj[u] {
            if v < out_base && !back_edges.contains(&(u, v)) {
                layer[v] = layer[v].max(layer[u] + 1);
            }
        }
    }
    let last = (nin..out_base).map(|v| layer[v]).max().unwrap_or(0) + 1;
    for v in out_base..n {
        layer[v] = last;
    }

    // Slots, with dummies carrying long wires through skipped columns.
    let mut columns: Vec<Vec<Slot>> = vec![Vec::new(); last + 1];
    for v in 0..n {
        columns[layer[v]].push(Slot::Node(v));
    }
    let mut dummies: Vec<Dummy> = Vec::new();
    let mut dummy_at: HashMap<(Source, usize), usize> = HashMap::new();
    let mut links = Vec::new();
    let mut feedback = Vec::new();
    for &(source, si) in pairs.iter() {
        let sink = &sinks[si];
        let (from_layer, to_layer) = (layer[source.node], layer[sink.node]);
        if to_layer <= from_layer {
            feedback.push((source, si));
            continue;
        }
        let mut prev = (Slot::Node(source.node), geoms[source.node].out_off[source.pin]);
        for l in from_layer + 1..to_layer {
            let d = *dummy_at.entry((source, l)).or_insert_with(|| {
                dummies.push(Dummy { source, layer: l });
                columns[l].push(Slot::Dummy(dummies.len() - 1));
                dummies.len() - 1
            });
            links.push(Link { from: prev.0, from_off: prev.1, to: Slot::Dummy(d), to_off: DUMMY_H / 2.0 });
            prev = (Slot::Dummy(d), DUMMY_H / 2.0);
        }
        links.push(Link {
            from: prev.0,
            from_off: prev.1,
            to: Slot::Node(sink.node),
            to_off: geoms[sink.node].in_off[sink.pin],
        });
    }

    order_columns(&mut columns, &links, last);

    // Vertical positions: pull each slot toward the pins it connects to.
    let height = |slot: Slot| match slot {
        Slot::Node(v) => geoms[v].h,
        Slot::Dummy(_) => DUMMY_H,
    };
    let mut top: HashMap<Slot, f32> = HashMap::new();
    for column in &columns {
        let mut y = 0.0;
        for &slot in column {
            top.insert(slot, y);
            y += height(slot) + NODE_GAP;
        }
    }
    let passes: Vec<(usize, bool)> = (1..=last)
        .map(|l| (l, true))
        .chain((0..last).rev().map(|l| (l, false)))
        .chain((1..=last).map(|l| (l, true)))
        .collect();
    for (l, forward) in passes {
        let desired: Vec<Option<f32>> = columns[l]
            .iter()
            .map(|&slot| {
                let wants: Vec<f32> = links
                    .iter()
                    .filter_map(|link| {
                        if forward && link.to == slot {
                            Some(top[&link.from] + link.from_off - link.to_off)
                        } else if !forward && link.from == slot {
                            Some(top[&link.to] + link.to_off - link.from_off)
                        } else {
                            None
                        }
                    })
                    .collect();
                // Line up exactly with the connection nearest the average, so
                // one wire runs straight instead of every wire bending a little.
                let mean = wants.iter().sum::<f32>() / wants.len().max(1) as f32;
                wants.iter().copied().min_by(|a, b| (a - mean).abs().total_cmp(&(b - mean).abs()))
            })
            .collect();
        let mut floor = f32::NEG_INFINITY;
        let mut prev: Option<Slot> = None;
        for (i, &slot) in columns[l].iter().enumerate() {
            if let Some(p) = prev {
                let gap = if matches!((p, slot), (Slot::Node(_), Slot::Node(_))) { NODE_GAP } else { DUMMY_GAP };
                floor = top[&p] + height(p) + gap;
            }
            let want = desired[i].unwrap_or(top[&slot]);
            top.insert(slot, want.max(floor));
            prev = Some(slot);
        }
    }
    let min_top = top.values().copied().fold(f32::INFINITY, f32::min);
    for y in top.values_mut() {
        *y += TITLE_H - min_top;
    }
    let bottom = top.iter().map(|(&s, &y)| y + height(s)).fold(TITLE_H, f32::max);

    // Wire trunks between each pair of columns.
    let end_y = |end: End| match end {
        End::Out(s) => top[&Slot::Node(s.node)] + geoms[s.node].out_off[s.pin],
        End::In { node, pin } => top[&Slot::Node(node)] + geoms[node].in_off[pin],
        End::DummyLeft(d) | End::DummyRight(d) => top[&Slot::Dummy(d)] + DUMMY_H / 2.0,
    };
    let mut trunks: Vec<Trunk> = Vec::new();
    let mut trunk_index: HashMap<(usize, Source), usize> = HashMap::new();
    for &(source, si) in pairs.iter() {
        let sink = &sinks[si];
        let (from_layer, to_layer) = (layer[source.node], layer[sink.node]);
        if to_layer <= from_layer {
            continue;
        }
        for gap in from_layer..to_layer {
            let index = *trunk_index.entry((gap, source)).or_insert_with(|| {
                let from = if gap == from_layer { End::Out(source) } else { End::DummyRight(dummy_at[&(source, gap)]) };
                trunks.push(Trunk { source, gap, from, to: Vec::new() });
                trunks.len() - 1
            });
            let to = if gap + 1 == to_layer {
                End::In { node: sink.node, pin: sink.pin }
            } else {
                End::DummyLeft(dummy_at[&(source, gap + 1)])
            };
            if !trunks[index].to.contains(&to) {
                trunks[index].to.push(to);
            }
        }
    }

    // Channels: vertical runs in the same gap that overlap need different x.
    enum Run {
        Trunk(usize),
        FeedbackOut(usize),
        FeedbackIn(usize),
    }
    let lane_y = |i: usize| bottom + 30.0 + i as f32 * LANE_STEP;
    let mut runs: Vec<Vec<(Run, f32, f32)>> = (0..last).map(|_| Vec::new()).collect();
    for (t, trunk) in trunks.iter().enumerate() {
        let from_y = end_y(trunk.from);
        let ys: Vec<f32> = trunk.to.iter().map(|&e| end_y(e)).chain([from_y]).collect();
        let straight = trunk.to.len() == 1 && (end_y(trunk.to[0]) - from_y).abs() < 0.5;
        if !straight {
            let lo = ys.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            runs[trunk.gap].push((Run::Trunk(t), lo, hi));
        }
    }
    for (i, &(source, si)) in feedback.iter().enumerate() {
        let sink = &sinks[si];
        let sy = end_y(End::Out(source));
        let ty = end_y(End::In { node: sink.node, pin: sink.pin });
        runs[layer[source.node]].push((Run::FeedbackOut(i), sy, lane_y(i)));
        runs[layer[sink.node] - 1].push((Run::FeedbackIn(i), ty, lane_y(i)));
    }
    let mut trunk_channel: HashMap<usize, usize> = HashMap::new();
    let mut feedback_channels: HashMap<(usize, bool), (usize, usize)> = HashMap::new();
    let mut channel_count = vec![0usize; last];
    for (gap, gap_runs) in runs.iter_mut().enumerate() {
        gap_runs.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut ends: Vec<f32> = Vec::new();
        for (run, lo, hi) in gap_runs.iter() {
            let channel = match ends.iter().position(|&end| end + 8.0 < *lo) {
                Some(c) => c,
                None => {
                    ends.push(f32::NEG_INFINITY);
                    ends.len() - 1
                }
            };
            ends[channel] = *hi;
            match run {
                Run::Trunk(t) => {
                    trunk_channel.insert(*t, channel);
                }
                Run::FeedbackOut(i) => {
                    feedback_channels.insert((*i, true), (gap, channel));
                }
                Run::FeedbackIn(i) => {
                    feedback_channels.insert((*i, false), (gap, channel));
                }
            }
        }
        channel_count[gap] = ends.len();
    }

    // Column x positions, leaving room for channels and pin labels.
    let mut col_w = vec![0.0f32; last + 1];
    for v in 0..n {
        col_w[layer[v]] = col_w[layer[v]].max(geoms[v].w);
    }
    let mut label_room = vec![0.0f32; last + 1];
    for sink in sinks.iter() {
        for part in &sink.parts {
            let text = part.constant.as_ref().or(part.label.as_ref());
            if let Some(text) = text {
                let room = text.chars().count() as f32 * SMALL_CHAR_W + 16.0;
                let l = layer[sink.node];
                label_room[l] = label_room[l].max(room);
            }
        }
    }
    let input_label_w =
        info.inputs.iter().map(|p| port_label(&p.name, p.width).chars().count() as f32 * CHAR_W).fold(0.0, f32::max);
    let output_label_w =
        info.outputs.iter().map(|p| port_label(&p.name, p.width).chars().count() as f32 * CHAR_W).fold(0.0, f32::max);
    let mut col_x = vec![MARGIN + input_label_w + 12.0; last + 1];
    for l in 0..last {
        let gap = ((channel_count[l] + 1) as f32 * CHANNEL_STEP + label_room[l + 1]).max(MIN_GAP);
        col_x[l + 1] = col_x[l] + col_w[l] + gap;
    }
    let channel_x = |gap: usize, channel: usize| col_x[gap] + col_w[gap] + CHANNEL_STEP * (channel + 1) as f32;
    let node_x = |v: usize| {
        let l = layer[v];
        match kinds[v] {
            NodeKind::Input { .. } => col_x[l] + col_w[l] - geoms[v].w,
            NodeKind::Output { .. } => col_x[l],
            _ => col_x[l] + (col_w[l] - geoms[v].w) / 2.0,
        }
    };
    let end_xy = |end: End| {
        let x = match end {
            End::Out(s) => node_x(s.node) + geoms[s.node].w,
            End::In { node, .. } => node_x(node),
            End::DummyLeft(d) => col_x[dummies[d].layer],
            End::DummyRight(d) => col_x[dummies[d].layer] + col_w[dummies[d].layer],
        };
        (x, end_y(end))
    };

    // Segments, grouped by source.
    let mut segments: BTreeMap<Source, Vec<[f32; 4]>> = BTreeMap::new();
    for (t, trunk) in trunks.iter().enumerate() {
        let segs = segments.entry(trunk.source).or_default();
        let (sx, sy) = end_xy(trunk.from);
        match trunk_channel.get(&t) {
            Some(&channel) => {
                let cx = channel_x(trunk.gap, channel);
                segs.push([sx, sy, cx, sy]);
                for &end in &trunk.to {
                    let (tx, ty) = end_xy(end);
                    segs.push([cx, sy, cx, ty]);
                    segs.push([cx, ty, tx, ty]);
                }
            }
            None => {
                let (tx, _) = end_xy(trunk.to[0]);
                segs.push([sx, sy, tx, sy]);
            }
        }
    }
    for (d, dummy) in dummies.iter().enumerate() {
        let y = top[&Slot::Dummy(d)] + DUMMY_H / 2.0;
        let x = col_x[dummy.layer];
        segments.entry(dummy.source).or_default().push([x, y, x + col_w[dummy.layer], y]);
    }
    for (i, &(source, si)) in feedback.iter().enumerate() {
        let sink = &sinks[si];
        let (sx, sy) = end_xy(End::Out(source));
        let (tx, ty) = end_xy(End::In { node: sink.node, pin: sink.pin });
        let (out_gap, out_channel) = feedback_channels[&(i, true)];
        let (in_gap, in_channel) = feedback_channels[&(i, false)];
        let (c1, c2, lane) = (channel_x(out_gap, out_channel), channel_x(in_gap, in_channel), lane_y(i));
        segments.entry(source).or_default().extend([
            [sx, sy, c1, sy],
            [c1, sy, c1, lane],
            [c1, lane, c2, lane],
            [c2, lane, c2, ty],
            [c2, ty, tx, ty],
        ]);
    }

    let positions: Vec<(f32, f32)> = (0..n).map(|v| (node_x(v), top[&Slot::Node(v)])).collect();
    let lanes_bottom = if feedback.is_empty() { bottom } else { lane_y(feedback.len() - 1) };
    let width = col_x[last] + col_w[last] + output_label_w + MARGIN + 12.0;
    Some(graph.assemble(chip, &positions, segments, width, lanes_bottom + MARGIN + 8.0))
}

/// Lays a chip out from saved positions (top-left corners keyed by
/// [`Node::key`]). Boxes without a saved position go next to whatever feeds
/// them. Coordinates are kept as saved; call [`Schematic::fit`] before
/// drawing it as an image.
pub fn arrange(
    design: &Design,
    sources: &SourceMap,
    chip: &str,
    saved: &HashMap<String, (f32, f32)>,
) -> Option<Schematic> {
    let graph = Graph::build(design, sources, chip)?;
    let n = graph.kinds.len();
    let mut placed: Vec<Option<(f32, f32)>> = graph.keys.iter().map(|k| saved.get(k).copied()).collect();
    for v in 0..n {
        if placed[v].is_none() {
            placed[v] = Some(place_new(&graph, &placed, v));
        }
    }
    let positions: Vec<(f32, f32)> = placed.into_iter().map(|p| p.unwrap_or_default()).collect();
    let segments = route_free(&graph, &positions);

    let mut right = 0.0f32;
    let mut bottom = 0.0f32;
    for (v, &(x, y)) in positions.iter().enumerate() {
        right = right.max(x + graph.geoms[v].w + 80.0);
        bottom = bottom.max(y + graph.geoms[v].h);
    }
    for seg in segments.values().flatten() {
        bottom = bottom.max(seg[1]).max(seg[3]);
    }
    Some(graph.assemble(chip, &positions, segments, right + MARGIN, bottom + MARGIN))
}

/// Picks a spot for a box with no saved position: right of what feeds it,
/// or below everything else, nudged down until it doesn't overlap.
fn place_new(graph: &Graph, placed: &[Option<(f32, f32)>], v: usize) -> (f32, f32) {
    let g = &graph.geoms[v];
    let others: Vec<(usize, (f32, f32))> = placed.iter().enumerate().filter_map(|(i, p)| p.map(|p| (i, p))).collect();
    let feeders: Vec<(f32, f32)> = graph
        .pairs
        .iter()
        .filter(|(_, si)| graph.sinks[*si].node == v)
        .filter_map(|(s, _)| {
            placed[s.node].map(|(x, y)| (x + graph.geoms[s.node].w, y + graph.geoms[s.node].out_off[s.pin]))
        })
        .collect();
    let left = others.iter().map(|(_, p)| p.0).fold(f32::INFINITY, f32::min);
    let left = if left.is_finite() { left } else { 120.0 };
    let right = others.iter().map(|&(i, p)| p.0 + graph.geoms[i].w).fold(left, f32::max);
    let lowest = |filter: &dyn Fn(usize) -> bool| {
        others
            .iter()
            .filter(|(i, _)| filter(*i))
            .map(|&(i, p)| p.1 + graph.geoms[i].h)
            .fold(f32::NEG_INFINITY, f32::max)
    };
    let below = |y: f32| if y.is_finite() { y + 30.0 } else { TITLE_H };
    let mid = g.in_off.first().copied().unwrap_or(g.h / 2.0);
    let (mut x, mut y) = match &graph.kinds[v] {
        _ if !feeders.is_empty() => {
            let x = feeders.iter().map(|f| f.0).fold(f32::NEG_INFINITY, f32::max) + 70.0;
            let y = feeders.iter().map(|f| f.1).sum::<f32>() / feeders.len() as f32 - mid;
            (x, y)
        }
        NodeKind::Input { .. } => {
            let inputs_x = others.iter().find(|(i, _)| *i < graph.nin).map(|(_, p)| p.0);
            (inputs_x.unwrap_or(left - g.w - 120.0), below(lowest(&|i| i < graph.nin)))
        }
        NodeKind::Output { .. } => {
            let outputs_x = others.iter().find(|(i, _)| *i >= graph.out_base).map(|(_, p)| p.0);
            (outputs_x.unwrap_or(right + 80.0), below(lowest(&|i| i >= graph.out_base)))
        }
        _ => (left + 160.0, below(lowest(&|_| true))),
    };
    x = (x / 10.0).round() * 10.0;
    y = (y / 10.0).round() * 10.0;
    let overlaps = |x: f32, y: f32| {
        others.iter().any(|&(i, (ox, oy))| {
            let o = &graph.geoms[i];
            x < ox + o.w + 10.0 && ox < x + g.w + 10.0 && y < oy + o.h + 10.0 && oy < y + g.h + 10.0
        })
    };
    while overlaps(x, y) {
        y += 20.0;
    }
    (x, y)
}

/// Routes wires between freely placed boxes: a vertical trunk between the
/// source and its nearest target, with wires that run backwards detouring
/// underneath both boxes.
fn route_free(graph: &Graph, positions: &[(f32, f32)]) -> BTreeMap<Source, Vec<[f32; 4]>> {
    let mut by_source: BTreeMap<Source, Vec<usize>> = BTreeMap::new();
    for &(source, si) in &graph.pairs {
        by_source.entry(source).or_default().push(si);
    }
    let mut trunks: Vec<(f32, f32, f32)> = Vec::new();
    let mut segments: BTreeMap<Source, Vec<[f32; 4]>> = BTreeMap::new();
    let mut detours = 0;
    for (source, sink_indexes) in by_source {
        let (sx, sy) = graph.out_pin(positions, source);
        let mut forward = Vec::new();
        let mut backward = Vec::new();
        for si in sink_indexes {
            let sink = &graph.sinks[si];
            let (tx, ty) = graph.in_pin(positions, sink.node, sink.pin);
            if tx >= sx + 24.0 { forward.push((tx, ty)) } else { backward.push((sink.node, tx, ty)) }
        }
        let segs = segments.entry(source).or_default();
        if let [(tx, ty)] = forward.as_slice()
            && (ty - sy).abs() < 0.5
        {
            segs.push([sx, sy, *tx, sy]);
        } else if !forward.is_empty() {
            let nearest = forward.iter().map(|f| f.0).fold(f32::INFINITY, f32::min);
            let lo = forward.iter().map(|f| f.1).fold(sy, f32::min);
            let hi = forward.iter().map(|f| f.1).fold(sy, f32::max);
            let desired = sx + ((nearest - sx) / 2.0).max(14.0);
            let x = pick_channel(&mut trunks, desired, lo, hi, sx + 8.0, nearest - 8.0);
            segs.push([sx, sy, x, sy]);
            for &(tx, ty) in &forward {
                segs.push([x, sy, x, ty]);
                segs.push([x, ty, tx, ty]);
            }
        }
        for (node, tx, ty) in backward {
            let bottom = |n: usize| positions[n].1 + graph.geoms[n].h;
            let detour_y = bottom(source.node).max(bottom(node)) + 20.0 + detours as f32 * 10.0;
            detours += 1;
            let x1 = pick_channel(&mut trunks, sx + 14.0, sy, detour_y, sx + 6.0, sx + 60.0);
            let x2 = pick_channel(&mut trunks, tx - 14.0, ty, detour_y, tx - 60.0, tx - 6.0);
            segs.extend([
                [sx, sy, x1, sy],
                [x1, sy, x1, detour_y],
                [x1, detour_y, x2, detour_y],
                [x2, detour_y, x2, ty],
                [x2, ty, tx, ty],
            ]);
        }
    }
    segments
}

/// Finds an x for a vertical run near `desired` that doesn't sit on top of
/// another wire's vertical run, and reserves it.
fn pick_channel(trunks: &mut Vec<(f32, f32, f32)>, desired: f32, y1: f32, y2: f32, min_x: f32, max_x: f32) -> f32 {
    let (lo, hi) = (y1.min(y2), y1.max(y2));
    let desired = (desired / 2.0).round() * 2.0;
    let clash =
        |x: f32| trunks.iter().any(|&(tx, tlo, thi)| (tx - x).abs() < 8.0 && tlo <= hi + 6.0 && lo <= thi + 6.0);
    let mut chosen = desired;
    for k in 0..40 {
        let step = (k + 1) / 2;
        let x = desired + step as f32 * if k % 2 == 1 { 10.0 } else { -10.0 };
        if (x < min_x || x > max_x) && k > 0 {
            continue;
        }
        if !clash(x) {
            chosen = x;
            break;
        }
    }
    trunks.push((chosen, lo, hi));
    chosen
}

impl Schematic {
    /// Moves everything by (dx, dy).
    pub fn translate(&mut self, dx: f32, dy: f32) {
        for node in &mut self.nodes {
            node.x += dx;
            node.y += dy;
            for pin in node.inputs.iter_mut().chain(&mut node.outputs) {
                pin.x += dx;
                pin.y += dy;
            }
        }
        for wire in &mut self.wires {
            for seg in &mut wire.segments {
                *seg = [seg[0] + dx, seg[1] + dy, seg[2] + dx, seg[3] + dy];
            }
            for dot in &mut wire.dots {
                *dot = (dot.0 + dx, dot.1 + dy);
            }
            wire.label_at = wire.label_at.map(|(x, y)| (x + dx, y + dy));
        }
        for tag in &mut self.tags {
            tag.x += dx;
            tag.y += dy;
        }
        self.width += dx;
        self.height += dy;
    }

    /// Shifts a freely arranged schematic so it starts just below the title
    /// with room for labels, and sizes the image to fit.
    pub fn fit(&mut self) {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for node in &self.nodes {
            let (left, right) = match &node.kind {
                NodeKind::Input { name, width } => (port_label(name, *width).len() as f32 * CHAR_W + 12.0, 0.0),
                NodeKind::Output { name, width } => (0.0, port_label(name, *width).len() as f32 * CHAR_W + 12.0),
                _ => (0.0, 0.0),
            };
            min_x = min_x.min(node.x - left);
            max_x = max_x.max(node.x + node.w + right);
            min_y = min_y.min(node.y - 18.0);
            max_y = max_y.max(node.y + node.h);
        }
        for seg in self.wires.iter().flat_map(|w| &w.segments) {
            min_x = min_x.min(seg[0].min(seg[2]));
            max_x = max_x.max(seg[0].max(seg[2]));
            min_y = min_y.min(seg[1].min(seg[3]));
            max_y = max_y.max(seg[1].max(seg[3]));
        }
        if !min_x.is_finite() {
            return;
        }
        let (dx, dy) = (MARGIN - min_x, TITLE_H - 16.0 - min_y);
        self.translate(dx, dy);
        self.width = max_x + dx + MARGIN;
        self.height = max_y + dy + MARGIN;
    }
}

/// Depth-first search from the inputs. Returns nodes in topological order
/// (ignoring feedback) and the edges that close a loop.
fn depth_first(adj: &[Vec<usize>], nin: usize, n: usize) -> (Vec<usize>, HashSet<(usize, usize)>) {
    let mut state = vec![0u8; n];
    let mut finished = Vec::with_capacity(n);
    let mut back = HashSet::new();
    let roots = (0..nin).chain(nin..n);
    for root in roots {
        if state[root] != 0 {
            continue;
        }
        state[root] = 1;
        let mut stack = vec![(root, 0usize)];
        while let Some(&mut (u, ref mut next)) = stack.last_mut() {
            if let Some(&v) = adj[u].get(*next) {
                *next += 1;
                match state[v] {
                    0 => {
                        state[v] = 1;
                        stack.push((v, 0));
                    }
                    1 => {
                        back.insert((u, v));
                    }
                    _ => {}
                }
            } else {
                state[u] = 2;
                finished.push(u);
                stack.pop();
            }
        }
    }
    finished.reverse();
    (finished, back)
}

/// Reorders the middle columns to reduce crossings, by sorting each slot to
/// the average position of its neighbors. Inputs and outputs keep the order
/// they're declared in.
fn order_columns(columns: &mut [Vec<Slot>], links: &[Link], last: usize) {
    let mut pos: HashMap<Slot, f32> = HashMap::new();
    for column in columns.iter() {
        for (i, &slot) in column.iter().enumerate() {
            pos.insert(slot, i as f32);
        }
    }
    for _ in 0..6 {
        let sweeps = (1..last).map(|l| (l, true)).chain((1..last).rev().map(|l| (l, false)));
        for (l, forward) in sweeps {
            let mut keyed: Vec<(f32, Slot)> = columns[l]
                .iter()
                .map(|&slot| {
                    let neighbors: Vec<f32> = links
                        .iter()
                        .filter_map(|link| {
                            if forward && link.to == slot {
                                Some(pos[&link.from])
                            } else if !forward && link.from == slot {
                                Some(pos[&link.to])
                            } else {
                                None
                            }
                        })
                        .collect();
                    let key = if neighbors.is_empty() {
                        pos[&slot]
                    } else {
                        neighbors.iter().sum::<f32>() / neighbors.len() as f32
                    };
                    (key, slot)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
            columns[l] = keyed.into_iter().map(|(_, slot)| slot).collect();
            for (i, &slot) in columns[l].iter().enumerate() {
                pos.insert(slot, i as f32);
            }
        }
    }
}

/// Splits segments wherever another segment of the same wire ends on them,
/// removes duplicates, and puts a dot wherever three or more pieces meet.
fn tidy(segs: Vec<[f32; 4]>) -> (Vec<[f32; 4]>, Vec<(f32, f32)>) {
    let key = |x: f32, y: f32| ((x * 2.0).round() as i64, (y * 2.0).round() as i64);
    let points: Vec<(f32, f32)> = segs.iter().flat_map(|s| [(s[0], s[1]), (s[2], s[3])]).collect();
    let mut pieces: Vec<[f32; 4]> = Vec::new();
    for s in segs {
        let [x1, y1, x2, y2] = s;
        if key(x1, y1) == key(x2, y2) {
            continue;
        }
        let horizontal = (y1 - y2).abs() < 0.5;
        let (lo, hi) = if horizontal { (x1.min(x2), x1.max(x2)) } else { (y1.min(y2), y1.max(y2)) };
        let mut cuts: Vec<f32> = points
            .iter()
            .filter(|&&(px, py)| {
                if horizontal {
                    (py - y1).abs() < 0.5 && px > lo + 0.5 && px < hi - 0.5
                } else {
                    (px - x1).abs() < 0.5 && py > lo + 0.5 && py < hi - 0.5
                }
            })
            .map(|&(px, py)| if horizontal { px } else { py })
            .collect();
        cuts.push(lo);
        cuts.push(hi);
        cuts.sort_by(f32::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        for w in cuts.windows(2) {
            let piece = if horizontal { [w[0], y1, w[1], y1] } else { [x1, w[0], x1, w[1]] };
            let k = (key(piece[0], piece[1]), key(piece[2], piece[3]));
            if !pieces.iter().any(|p| (key(p[0], p[1]), key(p[2], p[3])) == k) {
                pieces.push(piece);
            }
        }
    }
    let mut degree: HashMap<(i64, i64), (u32, (f32, f32))> = HashMap::new();
    for p in &pieces {
        for (x, y) in [(p[0], p[1]), (p[2], p[3])] {
            degree.entry(key(x, y)).or_insert((0, (x, y))).0 += 1;
        }
    }
    let mut dots: Vec<(f32, f32)> = degree.into_values().filter(|(d, _)| *d >= 3).map(|(_, at)| at).collect();
    dots.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    (pieces, dots)
}

struct Resolver<'a> {
    design: &'a Design,
    info: &'a ChipInfo,
    sources: &'a SourceMap,
    nin: usize,
}

impl Resolver<'_> {
    /// What an expression is wired to.
    fn resolve(&self, expr: &Expr, depth: usize) -> Vec<Part> {
        match &expr.kind {
            ExprKind::Literal(_) => {
                vec![Part { source: None, constant: Some(self.sources.slice(expr.span).to_string()), label: None }]
            }
            ExprKind::Str(_) => Vec::new(),
            ExprKind::Concat(parts) => parts.iter().flat_map(|p| self.resolve(p, depth)).collect(),
            ExprKind::Ref { base, port, select } => {
                let mut parts = match self.info.names.get(&base.name) {
                    Some(Name::Input(i)) => vec![self.part(Source { node: *i, pin: 0 })],
                    Some(Name::Instance(k)) => {
                        let pin = match port {
                            Some(port) => self
                                .design
                                .signature(&self.info.instances[*k].callee)
                                .outputs
                                .iter()
                                .position(|(n, _)| *n == port.name)
                                .unwrap_or(0),
                            None => 0,
                        };
                        vec![self.part(Source { node: self.nin + k, pin })]
                    }
                    Some(&name) => self.drivers(name, depth + 1),
                    None => Vec::new(),
                };
                if select.is_some() {
                    let text = self.sources.slice(expr.span).to_string();
                    for part in &mut parts {
                        part.label = Some(text.clone());
                    }
                }
                parts
            }
        }
    }

    /// What drives an output or wire.
    fn drivers(&self, name: Name, depth: usize) -> Vec<Part> {
        if depth > 8 {
            return Vec::new();
        }
        let mut parts = Vec::new();
        for (k, inst) in self.info.instances.iter().enumerate() {
            if inst.drives == Some(name) {
                parts.push(self.part(Source { node: self.nin + k, pin: 0 }));
            }
        }
        for assign in &self.info.assigns {
            if assign.target != name {
                continue;
            }
            let mut sub = self.resolve(&assign.value, depth);
            if let Some(select) = assign.select {
                let target = format!("{}{}", self.name_text(name), bit_text(&select));
                for part in &mut sub {
                    part.label = Some(format!("{}→{target}", part.label.as_deref().unwrap_or("")));
                }
            }
            parts.extend(sub);
        }
        parts
    }

    fn part(&self, source: Source) -> Part {
        Part { source: Some(source), constant: None, label: None }
    }

    fn name_text(&self, name: Name) -> &str {
        match name {
            Name::Input(i) => &self.info.inputs[i].name,
            Name::Output(i) => &self.info.outputs[i].name,
            Name::Wire(i) => &self.info.wires[i].name,
            Name::Instance(i) => &self.info.instances[i].name,
        }
    }

    /// The signal path (as the netlist names it) and width a source carries.
    fn source_signal(&self, source: Source) -> (String, usize) {
        if source.node < self.nin {
            let port = &self.info.inputs[source.node];
            return (port.name.clone(), port.width);
        }
        let inst = &self.info.instances[source.node - self.nin];
        let sig = self.design.signature(&inst.callee);
        let width = sig.outputs.get(source.pin).map_or(1, |o| o.1);
        let path = match inst.drives {
            Some(Name::Output(i)) if !inst.shared => self.info.outputs[i].name.clone(),
            Some(Name::Wire(i)) if !inst.shared => self.info.wires[i].name.clone(),
            _ if sig.outputs.len() == 1 => inst.name.clone(),
            _ => format!("{}.{}", inst.name, sig.outputs.get(source.pin).map_or("out", |o| o.0.as_str())),
        };
        (path, width)
    }
}

fn bit_text(select: &Select) -> String {
    if select.hi == select.lo { format!("[{}]", select.hi) } else { format!("[{}:{}]", select.hi, select.lo) }
}

/// `a` for one bit, `a[8]` for a bus.
pub fn port_label(name: &str, width: usize) -> String {
    if width == 1 { name.to_string() } else { format!("{name}[{width}]") }
}

/// Characters needed to show a value: `1`, or `0x2A` for a bus.
pub fn value_chars(width: usize) -> usize {
    if width == 1 { 1 } else { 2 + width.div_ceil(4) }
}

fn port_geom(width: usize, is_input: bool) -> Geom {
    let w = if width == 1 && !is_input { PORT_H } else { (value_chars(width) as f32 * CHAR_W + 16.0).max(28.0) };
    let mid = PORT_H / 2.0;
    if is_input {
        Geom { w, h: PORT_H, in_names: Vec::new(), in_off: Vec::new(), out_names: vec!["".into()], out_off: vec![mid] }
    } else {
        Geom { w, h: PORT_H, in_names: vec!["".into()], in_off: vec![mid], out_names: Vec::new(), out_off: Vec::new() }
    }
}

fn gate_geom(kind: GateKind) -> Geom {
    let in_off = match kind {
        GateKind::Mux => vec![12.0, 25.0, 38.0],
        GateKind::TriBuf => vec![25.0, 45.0],
        _ if kind.inputs().len() == 1 => vec![GATE_H / 2.0],
        _ => vec![15.0, 35.0],
    };
    Geom {
        w: GATE_W,
        h: GATE_H,
        in_names: kind.inputs().iter().map(|n| n.to_string()).collect(),
        in_off,
        out_names: vec!["out".into()],
        out_off: vec![GATE_H / 2.0],
    }
}

fn chip_geom(chip: &str, sig: &Signature) -> Geom {
    let rows = sig.inputs.len().max(sig.outputs.len()).max(1);
    let offsets = |count: usize| -> Vec<f32> {
        let shift = (rows - count) as f32 / 2.0;
        (0..count).map(|i| CHIP_HEADER + 10.0 + (i as f32 + shift) * PIN_STEP).collect()
    };
    let longest = |ports: &[(String, usize)]| ports.iter().map(|(n, _)| n.chars().count()).max().unwrap_or(0);
    let w = (chip.chars().count() as f32 * CHAR_W + 32.0)
        .max((longest(&sig.inputs) + longest(&sig.outputs)) as f32 * SMALL_CHAR_W + 40.0)
        .max(96.0);
    Geom {
        w,
        h: CHIP_HEADER + rows as f32 * PIN_STEP + 6.0,
        in_names: sig.inputs.iter().map(|(n, _)| n.clone()).collect(),
        in_off: offsets(sig.inputs.len()),
        out_names: sig.outputs.iter().map(|(n, _)| n.clone()).collect(),
        out_off: offsets(sig.outputs.len()),
    }
}
