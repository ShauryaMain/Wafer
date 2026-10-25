//! The schematic canvas. It draws the current chip from the same
//! [`Schematic`] the `wafer render` command uses, and turns mouse and
//! keyboard input into edits on the `.chip` file.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;

use egui::{
    Align2, Color32, FontId, Key, Modifiers, PointerButton, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2,
    vec2,
};
use wafer_core::GateKind;
use wafer_core::edit::{Edit, PortDir, Sink, fresh_name};
use wafer_core::gate::PortWidth;
use wafer_core::logic::{Logic, format_bits};
use wafer_core::schematic::{self, GATE_W, NodeKind, PORT_H, Schematic, TagKind, port_label};

use crate::shapes::{GateShape, gate_shape, triangulate};
use crate::simrun::SimRunner;
use crate::theme;
use crate::workspace::{Workspace, port_key};

/// What's under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Node(usize),
    /// (node, input pin)
    InPin(usize, usize),
    /// (node, output pin)
    OutPin(usize, usize),
    Wire(usize),
    Empty,
}

enum Drag {
    /// Moving the selection, by mouse drag or `G`.
    Move {
        origin: Pos2,
        base: Vec<(String, (f32, f32))>,
        grab: bool,
    },
    Select {
        from: Pos2,
    },
    Wire {
        from: Hit,
    },
}

struct AddMenu {
    world: Pos2,
    screen: Pos2,
    query: String,
    focus: bool,
}

/// A wire, identified in a way that survives reloads: its source node's
/// key and output pin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireId {
    pub node: String,
    pub pin: usize,
}

pub struct Canvas {
    /// The chip on screen, and the one edits apply to.
    pub chip: String,
    /// The chip being simulated: the one opened from the Chips panel.
    pub root: String,
    /// Boxes dived into from the root, as (box name, its chip), outermost first.
    pub path: Vec<(String, String)>,
    /// Signal-name prefix for the chip on screen, like `fa0.h1.`.
    pub prefix: String,
    /// The Make Chip dialog's name field, while it's open.
    make_chip: Option<(String, bool)>,
    /// Asking which bits a wire carries, after one was dropped between
    /// pins of different widths.
    bit_picker: Option<BitPicker>,
    /// The number typed into a pin's right-click menu.
    constant_text: String,
    offset: Vec2,
    zoom: f32,
    pub selection: BTreeSet<String>,
    pub selected_wire: Option<WireId>,
    drag: Option<Drag>,
    /// Positions of boxes being moved, not saved yet.
    moving: HashMap<String, (f32, f32)>,
    add_menu: Option<AddMenu>,
    context: Hit,
    /// The chip that was last framed, so a newly opened chip fits the view.
    framed: Option<String>,
    cache: Option<(String, u64, u64, Schematic)>,
    rect: Rect,
    /// Where the pointer last was over the canvas, for when a drag ends
    /// with the pointer already gone.
    last_pointer: Option<Pos2>,
    /// The canvas widget's id, so app-wide shortcuts still work while it
    /// has keyboard focus.
    pub focus_id: Option<egui::Id>,
}

impl Canvas {
    pub fn new(chip: String) -> Canvas {
        Canvas {
            root: chip.clone(),
            path: Vec::new(),
            prefix: String::new(),
            make_chip: None,
            bit_picker: None,
            constant_text: String::new(),
            chip,
            offset: Vec2::ZERO,
            zoom: 1.0,
            selection: BTreeSet::new(),
            selected_wire: None,
            drag: None,
            moving: HashMap::new(),
            add_menu: None,
            context: Hit::Empty,
            framed: None,
            cache: None,
            rect: Rect::NOTHING,
            last_pointer: None,
            focus_id: None,
        }
    }

    /// Opens a chip at the top level, to simulate on its own.
    pub fn open(&mut self, chip: &str) {
        if self.root != chip || !self.path.is_empty() {
            self.root = chip.to_string();
            self.set_path(Vec::new());
        }
    }

    /// Looks inside a chip box on screen.
    pub fn dive(&mut self, instance: &str, chip: &str) {
        let mut path = self.path.clone();
        path.push((instance.to_string(), chip.to_string()));
        self.set_path(path);
    }

    /// Goes back out one level.
    pub fn up(&mut self) {
        if let Some((instance, _)) = self.path.last().cloned() {
            let mut path = self.path.clone();
            path.pop();
            self.set_path(path);
            self.selection = [instance].into();
        }
    }

    pub fn set_path(&mut self, path: Vec<(String, String)>) {
        self.path = path;
        let chip = self.path.last().map_or(self.root.clone(), |(_, c)| c.clone());
        self.prefix = self.path.iter().map(|(i, _)| format!("{i}.")).collect();
        if chip != self.chip {
            self.framed = None;
        }
        self.chip = chip;
        self.selection.clear();
        self.selected_wire = None;
        self.drag = None;
        self.moving.clear();
        self.add_menu = None;
        self.make_chip = None;
        self.bit_picker = None;
    }

    /// Drops levels of the path that no longer exist after an edit.
    pub fn validate(&mut self, ws: &Workspace) {
        let mut chip = self.root.clone();
        let mut keep = 0;
        for (instance, child) in &self.path {
            let still_there = ws.design.chips.get(&chip).is_some_and(|info| {
                info.instances.iter().any(|i| {
                    &i.name == instance
                        && matches!(&i.callee, wafer_core::check::Callee::Chip { name, .. } if name == child)
                })
            });
            if !still_there {
                break;
            }
            chip = child.clone();
            keep += 1;
        }
        if keep < self.path.len() {
            let path = self.path[..keep].to_vec();
            self.set_path(path);
        }
    }

    /// A signal's value for the chip on screen (inside a dive, that's the
    /// value inside that particular box).
    /// How many copies of the chip on screen run side by side: more than 1
    /// inside a 1-bit chip used on a bus (`Xor ×16`), multiplied down the
    /// path when those are nested.
    pub fn copies(&self, ws: &Workspace) -> usize {
        let mut parent = self.root.clone();
        let mut copies = 1;
        for (instance, child) in &self.path {
            let inst = ws.design.chips.get(&parent).and_then(|c| c.instances.iter().find(|i| &i.name == instance));
            if let Some(wafer_core::check::Callee::Chip { copies: n, .. }) = inst.map(|i| &i.callee) {
                copies *= (*n).max(1);
            }
            parent = child.clone();
        }
        copies
    }

    fn val(&self, sim: &SimRunner, name: &str) -> Option<Vec<Logic>> {
        sim.value(&format!("{}{name}", self.prefix))
    }

    /// The laid-out chip, rebuilt only when something changed.
    pub fn schematic(&mut self, ws: &Workspace) -> Option<&Schematic> {
        // The same chip looks different inside `Xor ×16`, so the key says how many copies.
        let copies = self.copies(ws);
        let key = format!("{}×{copies}", self.chip);
        let fresh = matches!(&self.cache, Some((k, g, lg, _)) if *k == key && *g == ws.generation && *lg == ws.layout_generation);
        if !fresh || !self.moving.is_empty() {
            let built = build_schematic(ws, &self.chip, &self.moving, copies)?;
            self.cache = Some((key, ws.generation, ws.layout_generation, built));
        }
        self.cache.as_ref().map(|(_, _, _, s)| s)
    }

    /// The canvas's area on screen, as of the last frame.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    fn to_screen(&self, p: Pos2) -> Pos2 {
        self.rect.min + self.offset + p.to_vec2() * self.zoom
    }

    fn to_world(&self, p: Pos2) -> Pos2 {
        ((p - self.rect.min - self.offset) / self.zoom).to_pos2()
    }

    /// Screen position of a world point, for tests and tools.
    pub fn screen_pos(&self, world: (f32, f32)) -> Pos2 {
        self.to_screen(pos2(world.0, world.1))
    }

    pub fn frame_all(&mut self, schematic: &Schematic) {
        let mut bounds = Rect::NOTHING;
        for node in &schematic.nodes {
            let label = match &node.kind {
                NodeKind::Input { name, width } | NodeKind::Output { name, width } => {
                    port_label(name, *width).len() as f32 * 7.2 + 12.0
                }
                _ => 0.0,
            };
            bounds = bounds.union(Rect::from_min_size(
                pos2(node.x - label, node.y - 20.0),
                vec2(node.w + 2.0 * label, node.h + 30.0),
            ));
        }
        if !bounds.is_positive() || !self.rect.is_positive() {
            self.zoom = 1.0;
            self.offset = vec2(40.0, 40.0);
            return;
        }
        let fit = (self.rect.width() / bounds.width()).min(self.rect.height() / bounds.height()) * 0.9;
        self.zoom = fit.clamp(0.3, 1.5);
        let center = bounds.center().to_vec2() * self.zoom;
        self.offset = self.rect.size() / 2.0 - center;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ws: &mut Workspace, sim: &mut SimRunner) {
        self.breadcrumbs(ui);
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.rect = rect;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::BACKGROUND);

        let Some(schematic) = self.schematic(ws).cloned() else {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "No chip open. Make one in the Chips panel.",
                FontId::proportional(15.0),
                theme::MUTED,
            );
            return;
        };
        if self.framed.as_deref() != Some(self.chip.as_str()) {
            self.frame_all(&schematic);
            self.framed = Some(self.chip.clone());
        }

        self.pan_and_zoom(ui, &response);
        let pointer = response.hover_pos();
        if pointer.is_some() {
            self.last_pointer = pointer;
        }
        let hit = pointer.map_or(Hit::Empty, |p| self.hit_test(&schematic, p));
        self.handle_pointer(ui, &response, &schematic, ws, sim);
        // The canvas takes keyboard focus (so Tab dives instead of moving
        // focus around the window) unless you're typing somewhere else.
        let id = response.id;
        self.focus_id = Some(id);
        let focused = ui.memory(|m| m.focused());
        if response.clicked_by(PointerButton::Primary)
            || response.drag_started()
            || response.secondary_clicked()
            || (response.hovered() && focused.is_none())
        {
            response.request_focus();
        }
        if response.has_focus() {
            let filter = egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true };
            ui.memory_mut(|m| m.set_focus_lock_filter(id, filter));
        }
        if response.hovered() && ui.memory(|m| m.focused().is_none_or(|f| f == id)) {
            self.handle_keys(ui, &schematic, ws, pointer);
        }
        if response.secondary_clicked() {
            self.context = hit;
        }
        response.context_menu(|ui| self.context_menu(ui, &schematic, ws, sim));
        if response.clicked() || response.drag_started() {
            self.bit_picker = None;
        }

        self.draw(&painter, &schematic, sim, hit);
        self.add_menu_ui(ui, ws);

        if self.drag.is_none()
            && let Hit::Wire(w) = hit
            && let Some(wire) = schematic.wires.get(w)
        {
            let value = self.val(sim, &wire.signal).map(|v| format_bits(&v)).unwrap_or_else(|| "?".into());
            response.on_hover_text_at_pointer(format!("{} = {value}", wire.signal));
        }
        self.make_chip_dialog(ui, ws);
        self.bit_picker_ui(ui, ws);
    }

    /// `Add4 › fa0 · FullAdder › h1 · HalfAdder`: click a level to go back to it.
    fn breadcrumbs(&mut self, ui: &mut egui::Ui) {
        let mut jump: Option<usize> = None;
        ui.horizontal(|ui| {
            let top = egui::RichText::new(&self.root).monospace();
            if ui.selectable_label(self.path.is_empty(), top).clicked() {
                jump = Some(0);
            }
            for (depth, (instance, chip)) in self.path.iter().enumerate() {
                ui.label(egui::RichText::new("›").color(theme::MUTED));
                let here = depth + 1 == self.path.len();
                let text = egui::RichText::new(format!("{instance} · {chip}")).monospace();
                if ui.selectable_label(here, text).clicked() {
                    jump = Some(depth + 1);
                }
            }
            let hint = if self.path.is_empty() {
                "double-click a chip box (or Tab) to look inside".to_string()
            } else {
                format!("inside one {}: edits here change every {} · Shift+Tab to go back out", self.chip, self.chip)
            };
            ui.label(egui::RichText::new(hint).color(theme::MUTED).small());
        });
        if let Some(depth) = jump
            && depth < self.path.len()
        {
            let path = self.path[..depth].to_vec();
            self.set_path(path);
        }
    }

    /// Cmd+G: names the new chip, then moves the selected boxes into it.
    fn make_chip_dialog(&mut self, ui: &mut egui::Ui, ws: &mut Workspace) {
        let Some((name, focus)) = &mut self.make_chip else { return };
        let count = self.selection.iter().filter(|k| !k.contains(':')).count();
        let (mut make, mut cancel) = (false, false);
        egui::Window::new("Make a chip")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.label(format!(
                    "Turn the {count} selected box{} into a new chip.",
                    if count == 1 { "" } else { "es" }
                ));
                ui.label(
                    egui::RichText::new("Wires in and out become its inputs and outputs.").color(theme::MUTED).small(),
                );
                let field =
                    ui.add(egui::TextEdit::singleline(name).hint_text("Chip name").font(egui::TextStyle::Monospace));
                if *focus {
                    field.request_focus();
                    *focus = false;
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    make = true;
                }
                ui.horizontal(|ui| {
                    make |= ui.button("Make it").clicked();
                    cancel |= ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
                });
            });
        if cancel {
            self.make_chip = None;
            return;
        }
        if !make {
            return;
        }
        let name = name.trim().to_string();
        let Some(s) = self.cache.as_ref().map(|(_, _, _, s)| s.clone()) else { return };
        let picked: Vec<&wafer_core::schematic::Node> =
            s.nodes.iter().filter(|n| self.selection.contains(&n.key) && !n.key.contains(':')).collect();
        if picked.is_empty() {
            self.make_chip = None;
            return;
        }
        let (sum_x, sum_y) = picked.iter().fold((0.0, 0.0), |(x, y), n| (x + n.x, y + n.y));
        let place = (sum_x / picked.len() as f32, sum_y / picked.len() as f32);
        let inner: Vec<(String, (f32, f32))> = picked.iter().map(|n| (n.key.clone(), (n.x, n.y))).collect();
        let boxes: Vec<String> = picked.iter().map(|n| n.key.clone()).collect();
        if let Some(instance) = ws.extract(&self.chip, &boxes, &name, place, inner) {
            self.make_chip = None;
            self.selection = [instance].into();
            ws.status = Some((format!("Made {name}. Double-click it (or press Tab) to look inside."), false));
        }
    }

    /// Opens the Make Chip dialog for the selected boxes.
    pub fn start_make_chip(&mut self, ws: &Workspace) {
        if self.selection.iter().any(|k| !k.contains(':')) {
            let taken = |n: &str| ws.design.chips.contains_key(n);
            let name = std::iter::once("Part".to_string())
                .chain((2..).map(|n| format!("Part{n}")))
                .find(|n| !taken(n))
                .unwrap_or_default();
            self.make_chip = Some((name, true));
        }
    }

    pub fn make_chip_open(&self) -> bool {
        self.make_chip.is_some()
    }

    fn pan_and_zoom(&mut self, ui: &egui::Ui, response: &egui::Response) {
        if response.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            self.offset += scroll;
            if zoom != 1.0
                && let Some(p) = response.hover_pos()
            {
                let world = self.to_world(p);
                self.zoom = (self.zoom * zoom).clamp(0.2, 4.0);
                self.offset = p - self.rect.min - world.to_vec2() * self.zoom;
            }
        }
        if response.dragged_by(PointerButton::Middle) {
            self.offset += response.drag_delta();
        }
    }

    pub fn hit_test(&self, s: &Schematic, screen: Pos2) -> Hit {
        let world = self.to_world(screen);
        let pin_radius = (9.0 / self.zoom).max(5.0);
        for (i, node) in s.nodes.iter().enumerate() {
            for (p, pin) in node.outputs.iter().enumerate() {
                if world.distance(pos2(pin.x, pin.y)) < pin_radius {
                    return Hit::OutPin(i, p);
                }
            }
            for (p, pin) in node.inputs.iter().enumerate() {
                if world.distance(pos2(pin.x, pin.y)) < pin_radius {
                    return Hit::InPin(i, p);
                }
            }
        }
        for (i, node) in s.nodes.iter().enumerate().rev() {
            if Rect::from_min_size(pos2(node.x, node.y), vec2(node.w, node.h)).expand(2.0).contains(world) {
                return Hit::Node(i);
            }
        }
        let reach = (6.0 / self.zoom).max(3.0);
        for (i, wire) in s.wires.iter().enumerate() {
            if wire.segments.iter().any(|seg| distance_to_segment(world, *seg) < reach) {
                return Hit::Wire(i);
            }
        }
        Hit::Empty
    }

    fn handle_pointer(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        s: &Schematic,
        ws: &mut Workspace,
        sim: &mut SimRunner,
    ) {
        let pointer = ui.input(|i| i.pointer.hover_pos()).or(self.last_pointer);
        let shift = ui.input(|i| i.modifiers.shift);

        // `G` grab follows the pointer without a button; click to drop it.
        if let Some(Drag::Move { grab: true, .. }) = &self.drag {
            if let Some(p) = pointer {
                self.update_move(p);
            }
            if response.clicked_by(PointerButton::Primary)
                || ui.input(|i| i.pointer.primary_pressed() && response.hovered())
            {
                self.finish_move(ws);
            } else if response.secondary_clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                self.drag = None;
                self.moving.clear();
            }
            return;
        }

        if response.drag_started_by(PointerButton::Primary) {
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
            match self.hit_test(s, origin) {
                hit @ (Hit::InPin(..) | Hit::OutPin(..)) => self.drag = Some(Drag::Wire { from: hit }),
                Hit::Node(i) => {
                    let key = &s.nodes[i].key;
                    if !self.selection.contains(key) {
                        if !shift {
                            self.selection.clear();
                        }
                        self.selection.insert(key.clone());
                    }
                    self.start_move(s, origin, false);
                }
                Hit::Wire(_) | Hit::Empty => self.drag = Some(Drag::Select { from: origin }),
            }
        }

        if response.dragged_by(PointerButton::Primary)
            && let (Some(Drag::Move { .. }), Some(p)) = (&self.drag, pointer)
        {
            self.update_move(p);
        }

        if response.drag_stopped() {
            match self.drag.take() {
                Some(Drag::Move { base, origin, grab }) => {
                    self.drag = Some(Drag::Move { base, origin, grab });
                    self.finish_move(ws);
                }
                Some(Drag::Select { from }) => {
                    if let Some(to) = pointer.or(response.interact_pointer_pos()) {
                        let area = Rect::from_two_pos(self.to_world(from), self.to_world(to));
                        if !shift {
                            self.selection.clear();
                            self.selected_wire = None;
                        }
                        for node in &s.nodes {
                            if area.intersects(Rect::from_min_size(pos2(node.x, node.y), vec2(node.w, node.h))) {
                                self.selection.insert(node.key.clone());
                            }
                        }
                    }
                }
                Some(Drag::Wire { from }) => {
                    let target =
                        pointer.or(response.interact_pointer_pos()).map_or(Hit::Empty, |p| self.hit_test(s, p));
                    let choose = ui.input(|i| i.modifiers.alt);
                    self.finish_wire(s, ws, from, target, choose);
                }
                None => {}
            }
        }

        // Double-click a chip box to look inside it, or empty space to go back out.
        if response.double_clicked() {
            match response.interact_pointer_pos().map_or(Hit::Empty, |p| self.hit_test(s, p)) {
                Hit::Node(i) => {
                    if let NodeKind::Chip { name, chip, .. } = &s.nodes[i].kind
                        && ws.design.chips.contains_key(chip)
                    {
                        let (name, chip) = (name.clone(), chip.clone());
                        self.dive(&name, &chip);
                        return;
                    }
                }
                Hit::Empty => {
                    self.up();
                    return;
                }
                _ => {}
            }
        }

        if response.clicked_by(PointerButton::Primary) {
            let hit = response.interact_pointer_pos().map_or(Hit::Empty, |p| self.hit_test(s, p));
            match hit {
                Hit::Node(i) | Hit::OutPin(i, _) | Hit::InPin(i, _) => {
                    let node = &s.nodes[i];
                    if let (NodeKind::Input { name, width: 1 }, Hit::Node(_)) = (&node.kind, hit) {
                        if self.prefix.is_empty() {
                            sim.toggle(name);
                        } else {
                            let outer =
                                self.path.len().checked_sub(2).map_or(self.root.clone(), |i| self.path[i].1.clone());
                            ws.status = Some((
                                format!("Inside a chip, `{name}` comes from {outer}. Flip inputs on {}.", self.root),
                                false,
                            ));
                        }
                    }
                    if shift {
                        if !self.selection.remove(&node.key) {
                            self.selection.insert(node.key.clone());
                        }
                    } else {
                        self.selection = BTreeSet::from([node.key.clone()]);
                        self.selected_wire = None;
                    }
                }
                Hit::Wire(w) => {
                    let wire = &s.wires[w];
                    self.selected_wire = Some(WireId { node: s.nodes[wire.source.0].key.clone(), pin: wire.source.1 });
                    if !shift {
                        self.selection.clear();
                    }
                }
                Hit::Empty => {
                    self.selection.clear();
                    self.selected_wire = None;
                }
            }
        }
    }

    fn start_move(&mut self, s: &Schematic, origin: Pos2, grab: bool) {
        let base =
            s.nodes.iter().filter(|n| self.selection.contains(&n.key)).map(|n| (n.key.clone(), (n.x, n.y))).collect();
        self.drag = Some(Drag::Move { origin: self.to_world(origin), base, grab });
    }

    fn update_move(&mut self, pointer: Pos2) {
        let world = self.to_world(pointer);
        let Some(Drag::Move { origin, base, .. }) = &self.drag else { return };
        let delta = world - *origin;
        let snap = |v: f32| (v / 10.0).round() * 10.0;
        let (dx, dy) = (snap(delta.x), snap(delta.y));
        self.moving = base.iter().map(|(k, (x, y))| (k.clone(), (x + dx, y + dy))).collect();
    }

    fn finish_move(&mut self, ws: &mut Workspace) {
        self.drag = None;
        if self.moving.is_empty() {
            return;
        }
        // The first time a chip is arranged by hand, save every box's spot so
        // nothing else jumps around.
        let mut positions: Vec<(String, (f32, f32))> = Vec::new();
        if ws.positions(&self.chip).is_none()
            && let Some((_, _, _, s)) = &self.cache
        {
            positions.extend(s.nodes.iter().map(|n| (n.key.clone(), (n.x, n.y))));
        }
        positions.extend(self.moving.drain());
        ws.move_nodes(&self.chip, positions);
    }

    /// Connects a dropped wire, or asks which bits it carries. `choose`
    /// (Option/Alt held) always asks, with every bit free to pick.
    fn finish_wire(&mut self, s: &Schematic, ws: &mut Workspace, from: Hit, to: Hit, choose: bool) {
        let (source, sink) = match (from, to) {
            (Hit::OutPin(n, p), Hit::InPin(m, q)) | (Hit::InPin(m, q), Hit::OutPin(n, p)) => ((n, p), (m, q)),
            (_, Hit::Empty) | (_, Hit::Wire(_)) | (_, Hit::Node(_)) => return,
            _ => {
                ws.status = Some(("Wires go from an output pin to an input pin".into(), true));
                return;
            }
        };
        let (Some(sink_pin), Some(source_pin)) =
            (s.nodes[sink.0].inputs.get(sink.1), s.nodes[source.0].outputs.get(source.1))
        else {
            return;
        };
        // Inside `Xor ×16` everything is drawn 16 times wider than the text it edits.
        let shown = self.copies(ws).max(1);
        let (sink_width, source_width) = (sink_pin.width / shown, source_pin.width / shown);
        let at = self.screen_pos((sink_pin.x, sink_pin.y)) + vec2(14.0, -12.0);
        let sink_adapts = adapts_to_wire(ws, &self.chip, &s.nodes[sink.0], Some(sink.1));
        let adapts = sink_adapts || adapts_to_wire(ws, &self.chip, &s.nodes[source.0], None);
        // A 1-bit signal feeds every copy of a chip spread across a bus, so
        // from anything else of the wrong width, the question is which bit.
        let spread = copies_of(ws, &self.chip, &s.nodes[sink.0]) > 1;
        let feeds_every_copy = source_width == 1 && spread;
        let sink_width = if spread && source_width != sink_width && !choose { 1 } else { sink_width };
        let (Some(sink), Some(source)) = (sink_of(s, sink.0, sink.1), source_text(s, source.0, source.1)) else {
            return;
        };
        if !choose && (sink_width == source_width || adapts || feeds_every_copy) {
            ws.edit(&self.chip, &[Edit::Connect { sink, source }], &[]);
            return;
        }
        let sink_label = match &sink {
            Sink::Instance { name, port } => format!("{name}.{port}"),
            Sink::Output(name) => name.clone(),
        };
        // Free picking starts with as much of the source as fits, at bit 0.
        let take = if sink_adapts { source_width } else { source_width.min(sink_width).max(1) };
        let free = choose.then_some(FreePick { hi: take - 1, lo: 0, at: 0 });
        self.bit_picker = Some(BitPicker {
            sink,
            sink_label,
            sink_width,
            sink_adapts,
            sink_spread: spread,
            source,
            source_width,
            screen: at,
            custom: 0,
            free,
        });
    }

    /// Picks any bits of the source and where they land in the pin.
    pub fn choose_bits(&mut self, hi: usize, lo: usize, at: usize) {
        if let Some(picker) = &mut self.bit_picker {
            picker.free = Some(FreePick { hi, lo, at });
        }
    }

    /// The free side of the bit picker. Returns the edit to make, if any.
    fn free_pick_ui(ui: &mut egui::Ui, picker: &mut BitPicker) -> Option<Edit> {
        let (sw, kw) = (picker.source_width, picker.sink_width);
        let source = picker.source.clone();
        let slice = |hi: usize, lo: usize| match (hi, lo) {
            (hi, 0) if hi + 1 == sw => source.clone(),
            (hi, lo) if hi == lo => format!("{source}[{lo}]"),
            (hi, lo) => format!("{source}[{hi}:{lo}]"),
        };
        let button = |ui: &mut egui::Ui, text: &str| ui.button(egui::RichText::new(text).monospace()).clicked();
        // A whole-pin connection to `source[hi:lo]`, or bits placed in the pin.
        let mut whole: Option<(usize, usize)> = None;
        let mut placed: Option<FreePick> = None;
        let mut pick = picker.free.unwrap_or(FreePick { hi: 0, lo: 0, at: 0 });
        ui.label(format!("Wire bits of {} into {}", picker.source, picker.sink_label));
        if picker.sink_spread && sw > 1 && sw <= 32 {
            ui.label(egui::RichText::new("One bit for every copy:").color(theme::MUTED).small());
            ui.horizontal_wrapped(|ui| {
                for bit in (0..sw).rev() {
                    if button(ui, &slice(bit, bit)) {
                        whole = Some((bit, bit));
                    }
                }
            });
        }
        if picker.sink_adapts && sw > 1 {
            // The pin takes any width: the usual picks are a half or an end bit.
            let mut picks = vec![(sw - 1, sw - 1)];
            if sw.is_multiple_of(2) && sw > 2 {
                picks.extend([(sw - 1, sw / 2), (sw / 2 - 1, 0)]);
            }
            picks.push((0, 0));
            ui.horizontal_wrapped(|ui| {
                for (hi, lo) in picks {
                    if button(ui, &slice(hi, lo)) {
                        whole = Some((hi, lo));
                    }
                }
            });
        } else if sw == kw && sw > 1 {
            // Shifts, the usual reason to wire bits by hand.
            ui.horizontal_wrapped(|ui| {
                for k in [1, 2, 4, 8].into_iter().filter(|&k| k < sw) {
                    if button(ui, &format!("<< {k}")) {
                        placed = Some(FreePick { hi: sw - 1 - k, lo: 0, at: k });
                    }
                }
                for k in [1, 2, 4, 8].into_iter().filter(|&k| k < sw) {
                    if button(ui, &format!(">> {k}")) {
                        placed = Some(FreePick { hi: sw - 1, lo: k, at: 0 });
                    }
                }
            });
        } else if sw > kw && sw.is_multiple_of(kw) && sw / kw <= 16 {
            // Pieces the size of the pin, highest first.
            ui.horizontal_wrapped(|ui| {
                for lo in (0..sw / kw).rev().map(|k| k * kw) {
                    if button(ui, &slice(lo + kw - 1, lo)) {
                        whole = Some((lo + kw - 1, lo));
                    }
                }
            });
        }
        ui.horizontal(|ui| {
            ui.label("take");
            ui.label(egui::RichText::new(format!("{}[", picker.source)).monospace());
            ui.add(egui::DragValue::new(&mut pick.hi).range(0..=sw - 1));
            ui.label(egui::RichText::new(":").monospace());
            ui.add(egui::DragValue::new(&mut pick.lo).range(0..=sw - 1));
            ui.label(egui::RichText::new("]").monospace());
        });
        if !picker.sink_adapts {
            ui.horizontal(|ui| {
                ui.label("into");
                ui.label(egui::RichText::new(&picker.sink_label).monospace());
                ui.label("from bit");
                ui.add(egui::DragValue::new(&mut pick.at).range(0..=kw - 1));
            });
        }
        picker.free = Some(pick);
        let n = pick.hi.checked_sub(pick.lo).map(|d| d + 1);
        let bits = |hi: usize, lo: usize| if hi == lo { format!("[{lo}]") } else { format!("[{hi}:{lo}]") };
        match n {
            Some(n) if picker.sink_adapts => {
                ui.label(
                    egui::RichText::new(format!("{} ← {}", picker.sink_label, slice(pick.hi, pick.lo)))
                        .monospace()
                        .color(theme::BUS),
                );
                ui.label(
                    egui::RichText::new(format!("It becomes {n} bit{} wide.", if n == 1 { "" } else { "s" }))
                        .color(theme::MUTED)
                        .small(),
                );
                if ui.button("Connect").clicked() {
                    whole = Some((pick.hi, pick.lo));
                }
            }
            Some(n) if pick.at + n <= kw => {
                ui.label(
                    egui::RichText::new(format!(
                        "{}{} ← {}{}",
                        picker.sink_label,
                        bits(pick.at + n - 1, pick.at),
                        picker.source,
                        bits(pick.hi, pick.lo)
                    ))
                    .monospace()
                    .color(theme::BUS),
                );
                ui.label(egui::RichText::new("Bits you don't wire read 0.").color(theme::MUTED).small());
                if ui.button("Connect").clicked() {
                    placed = Some(pick);
                }
            }
            Some(n) => {
                ui.label(
                    egui::RichText::new(format!(
                        "That's {n} bits, but {} has only {} from bit {} up.",
                        picker.sink_label,
                        kw - pick.at,
                        pick.at
                    ))
                    .color(theme::UNKNOWN),
                );
            }
            None => {
                ui.label(egui::RichText::new("The first bit has to be the higher one.").color(theme::UNKNOWN));
            }
        }
        if let Some((hi, lo)) = whole {
            return Some(Edit::Connect { sink: picker.sink.clone(), source: slice(hi, lo) });
        }
        let pick = placed?;
        let n = pick.hi - pick.lo + 1;
        Some(Edit::ConnectBits {
            sink: picker.sink.clone(),
            hi: pick.at + n - 1,
            lo: pick.at,
            source: slice(pick.hi, pick.lo),
        })
    }

    fn bit_picker_ui(&mut self, ui: &mut egui::Ui, ws: &mut Workspace) {
        let Some(picker) = &mut self.bit_picker else { return };
        if picker.free.is_some() {
            let (mut edit, mut close) = (None, false);
            egui::Area::new(egui::Id::new("bit_picker")).fixed_pos(picker.screen).order(egui::Order::Foreground).show(
                ui.ctx(),
                |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(340.0);
                        edit = Self::free_pick_ui(ui, picker);
                        close |= ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
                    });
                },
            );
            if let Some(edit) = edit {
                ws.edit(&self.chip, &[edit], &[]);
                close = true;
            }
            if close {
                self.bit_picker = None;
            }
            return;
        }
        let wide_source = picker.source_width > picker.sink_width;
        // The narrow side fits somewhere in the wide side: these are the
        // places it lines up with, lowest first.
        let (n, total) = if wide_source {
            (picker.sink_width, picker.source_width)
        } else {
            (picker.source_width, picker.sink_width)
        };
        let mut starts: Vec<usize> = (0..).map(|k| k * n).take_while(|lo| lo + n <= total).collect();
        if total % n != 0 {
            starts.push(total - n);
        }
        let wide = if wide_source { picker.source.clone() } else { picker.sink_label.clone() };
        let bits = |lo: usize| if n == 1 { format!("{wide}[{lo}]") } else { format!("{wide}[{}:{lo}]", lo + n - 1) };
        let (mut chosen, mut close, mut free) = (None, false, false);
        egui::Area::new(egui::Id::new("bit_picker")).fixed_pos(picker.screen).order(egui::Order::Foreground).show(
            ui.ctx(),
            |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(320.0);
                    let question = if wide_source {
                        format!(
                            "{} takes {n} bit{} of {}'s {total}. Which?",
                            picker.sink_label,
                            if n == 1 { "" } else { "s" },
                            picker.source
                        )
                    } else {
                        format!(
                            "{} is {n} bit{}. Which bits of {} does it fill?",
                            picker.source,
                            if n == 1 { "" } else { "s" },
                            picker.sink_label
                        )
                    };
                    ui.label(question);
                    if starts.len() <= 32 {
                        ui.horizontal_wrapped(|ui| {
                            // Highest bits first, the way a bus reads.
                            for &lo in starts.iter().rev() {
                                if ui.button(egui::RichText::new(bits(lo)).monospace()).clicked() {
                                    chosen = Some(lo);
                                }
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Or starting at bit").color(theme::MUTED));
                        ui.add(egui::DragValue::new(&mut picker.custom).range(0..=total - n));
                        if ui.button("Connect").clicked() {
                            chosen = Some(picker.custom);
                        }
                    });
                    ui.horizontal(|ui| {
                        close |= ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
                        if ui
                            .small_button("Choose bits freely…")
                            .on_hover_text("Or hold Option (Alt) as you drop a wire")
                            .clicked()
                        {
                            free = true;
                        }
                    });
                });
            },
        );
        if free {
            let take = picker.source_width.min(picker.sink_width).max(1);
            picker.free = Some(FreePick { hi: take - 1, lo: 0, at: 0 });
            return;
        }
        if let Some(lo) = chosen {
            let hi = lo + n - 1;
            let edit = if wide_source {
                let source =
                    if n == 1 { format!("{}[{lo}]", picker.source) } else { format!("{}[{hi}:{lo}]", picker.source) };
                Edit::Connect { sink: picker.sink.clone(), source }
            } else {
                Edit::ConnectBits { sink: picker.sink.clone(), hi, lo, source: picker.source.clone() }
            };
            ws.edit(&self.chip, &[edit], &[]);
            close = true;
        }
        if close {
            self.bit_picker = None;
        }
    }

    pub fn bit_picker_open(&self) -> bool {
        self.bit_picker.is_some()
    }

    fn handle_keys(&mut self, ui: &egui::Ui, s: &Schematic, ws: &mut Workspace, pointer: Option<Pos2>) {
        let (add, delete, duplicate, grab, frame, escape) = ui.input(|i| {
            let plain = !i.modifiers.any();
            (
                i.modifiers.shift && i.key_pressed(Key::A),
                plain && (i.key_pressed(Key::X) || i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace)),
                i.modifiers.shift && i.key_pressed(Key::D),
                plain && i.key_pressed(Key::G),
                plain && i.key_pressed(Key::F),
                i.key_pressed(Key::Escape),
            )
        });
        let (up, dive, make) = ui.ctx().input_mut(|i| {
            let up = i.consume_key(Modifiers::SHIFT, Key::Tab);
            (up, !up && i.consume_key(Modifiers::NONE, Key::Tab), i.consume_key(Modifiers::COMMAND, Key::G))
        });
        if up {
            self.up();
            return;
        }
        if dive
            && let [key] = self.selection.iter().collect::<Vec<_>>().as_slice()
            && let Some(NodeKind::Chip { name, chip, .. }) = s.nodes.iter().find(|n| &n.key == *key).map(|n| &n.kind)
            && ws.design.chips.contains_key(chip)
        {
            let (name, chip) = (name.clone(), chip.clone());
            self.dive(&name, &chip);
            return;
        }
        if make {
            self.start_make_chip(ws);
        }
        let at = pointer.unwrap_or(self.rect.center());
        if add {
            self.open_add_menu(at);
        }
        if delete {
            self.delete_selection(s, ws);
        }
        if duplicate {
            self.duplicate_selection(s, ws, at);
        }
        if grab && !self.selection.is_empty() && self.drag.is_none() {
            self.start_move(s, at, true);
        }
        if frame {
            self.frame_all(s);
        }
        if escape && self.drag.is_none() {
            self.add_menu = None;
            self.selection.clear();
            self.selected_wire = None;
        }
    }

    pub fn open_add_menu(&mut self, screen: Pos2) {
        self.add_menu = Some(AddMenu { world: self.to_world(screen), screen, query: String::new(), focus: true });
    }

    pub fn delete_selection(&mut self, s: &Schematic, ws: &mut Workspace) {
        let mut edits = Vec::new();
        for key in &self.selection {
            edits.push(if let Some(name) = key.strip_prefix("in:") {
                Edit::RemovePort { dir: PortDir::In, name: name.into() }
            } else if let Some(name) = key.strip_prefix("out:") {
                Edit::RemovePort { dir: PortDir::Out, name: name.into() }
            } else {
                Edit::RemoveInstance { name: key.clone() }
            });
        }
        if let Some(id) = &self.selected_wire
            && let Some(wire) = s.wires.iter().find(|w| s.nodes[w.source.0].key == id.node && w.source.1 == id.pin)
        {
            // Only the bits this wire carries: the rest of a joined bus stays.
            let source = source_text(s, wire.source.0, wire.source.1).unwrap_or_default();
            edits.extend(
                wire.sinks
                    .iter()
                    .filter_map(|&(n, p)| sink_of(s, n, p))
                    .map(|sink| Edit::DisconnectSource { sink, source: source.clone() }),
            );
        }
        if !edits.is_empty() && ws.edit(&self.chip, &edits, &[]) {
            self.selection.clear();
            self.selected_wire = None;
        }
    }

    fn duplicate_selection(&mut self, s: &Schematic, ws: &mut Workspace, pointer: Pos2) {
        let Some(info) = ws.design.chips.get(&self.chip) else { return };
        let mut used: HashSet<String> = HashSet::new();
        let mut edits = Vec::new();
        let mut place = Vec::new();
        for node in s.nodes.iter().filter(|n| self.selection.contains(&n.key)) {
            let callee = match &node.kind {
                NodeKind::Gate { kind, .. } => kind.name().to_string(),
                NodeKind::Chip { chip, .. } => chip.clone(),
                _ => continue,
            };
            let base = callee.to_lowercase();
            let name = (1..)
                .map(|n| format!("{base}{n}"))
                .find(|n| !info.names.contains_key(n) && !used.contains(n))
                .expect("names run out eventually");
            used.insert(name.clone());
            edits.push(Edit::AddInstance { name: name.clone(), callee });
            place.push((name, (node.x + 30.0, node.y + 30.0)));
        }
        if edits.is_empty() {
            ws.status = Some(("Shift+D duplicates gates and chips (ports have to be unique)".into(), false));
            return;
        }
        if ws.edit(&self.chip, &edits, &place) {
            self.selection = place.iter().map(|(k, _)| k.clone()).collect();
            if let Some(s) = self.schematic(ws).cloned() {
                self.start_move(&s, pointer, true);
            }
        }
    }

    /// Adds a gate, chip or port where the pointer was.
    pub fn add(&mut self, ws: &mut Workspace, item: &AddItem, world: Pos2) {
        let Some(info) = ws.design.chips.get(&self.chip) else { return };
        let snap = |v: f32| (v / 10.0).round() * 10.0;
        let (edit, key) = match item {
            AddItem::Input => {
                let name = first_free(info, &["a", "b", "c", "d", "e", "f", "g", "h"], "in");
                (Edit::AddPort { dir: PortDir::In, name: name.clone(), width: 1 }, port_key(PortDir::In, &name))
            }
            AddItem::Output => {
                let name = first_free(info, &["out", "y", "z"], "out");
                (Edit::AddPort { dir: PortDir::Out, name: name.clone(), width: 1 }, port_key(PortDir::Out, &name))
            }
            AddItem::Callee(callee) => {
                let name = fresh_name(info, callee);
                (Edit::AddInstance { name: name.clone(), callee: callee.clone() }, name)
            }
        };
        if ws.edit(&self.chip, &[edit], &[(key.clone(), (snap(world.x), snap(world.y)))]) {
            self.selection = BTreeSet::from([key]);
            self.selected_wire = None;
        }
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, s: &Schematic, ws: &mut Workspace, sim: &mut SimRunner) {
        let at = ui.input(|i| i.pointer.interact_pos()).unwrap_or(self.rect.center());
        match self.context {
            Hit::InPin(n, p) => {
                let Some(sink) = sink_of(s, n, p) else { return };
                let width = s.nodes[n].inputs.get(p).map_or(1, |pin| pin.width) / self.copies(ws).max(1);
                let mut value: Option<String> = None;
                if ui.button("Set to 0").clicked() {
                    value = Some("0".into());
                }
                let one = if width > 1 { format!("Set to 1 ({})", constant_text(1, width)) } else { "Set to 1".into() };
                if ui.button(one).clicked() {
                    value = Some("1".into());
                }
                let adapts = adapts_to_wire(ws, &self.chip, &s.nodes[n], Some(p));
                if width > 1 {
                    let all = constant_text(u64::MAX >> (64 - width.min(64)), width);
                    if ui.button(format!("Set to all 1s ({all})")).clicked() {
                        value = Some(all);
                    }
                }
                if width > 1 || adapts {
                    ui.horizontal(|ui| {
                        let label = ui.label("Set to");
                        let field = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.constant_text)
                                    .hint_text("0x2A")
                                    .desired_width(70.0)
                                    .font(egui::TextStyle::Monospace),
                            )
                            .labelled_by(label.id);
                        if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            value = Some(self.constant_text.trim().to_string());
                        }
                    });
                }
                if let Some(value) = value {
                    match parse_constant(&value) {
                        // A pin that adapts takes the number's width, like `0xFFFF`.
                        Some(v) if adapts || width >= 64 || v >> width == 0 => {
                            ws.edit(&self.chip, &[Edit::Connect { sink: sink.clone(), source: value }], &[]);
                            self.constant_text.clear();
                        }
                        Some(_) => ws.status = Some((format!("{value} doesn't fit in {width} bits"), true)),
                        None => {
                            ws.status = Some((format!("`{value}` isn't a number (try 42, 0x2A or 0b101010)"), true))
                        }
                    }
                    ui.close();
                }
                ui.separator();
                if ui.button("Disconnect this pin").clicked() {
                    ws.edit(&self.chip, &[Edit::Disconnect { sink }], &[]);
                    ui.close();
                }
            }
            Hit::Node(n) | Hit::OutPin(n, _) => {
                let node = &s.nodes[n];
                if let NodeKind::Input { name, width: 1 } = &node.kind
                    && ui.button("Toggle").clicked()
                {
                    sim.toggle(name);
                    ui.close();
                }
                if ui.button("Delete        X").clicked() {
                    self.selection = BTreeSet::from([node.key.clone()]);
                    self.delete_selection(s, ws);
                    ui.close();
                }
            }
            Hit::Wire(w) => {
                if ui.button("Delete wire   X").clicked() {
                    let wire = &s.wires[w];
                    self.selected_wire = Some(WireId { node: s.nodes[wire.source.0].key.clone(), pin: wire.source.1 });
                    self.selection.clear();
                    self.delete_selection(s, ws);
                    ui.close();
                }
            }
            Hit::Empty => {
                if ui.button("Add…        Shift+A").clicked() {
                    self.open_add_menu(at);
                    ui.close();
                }
                if ui.button("Frame all         F").clicked() {
                    self.frame_all(s);
                    ui.close();
                }
            }
        }
    }

    fn add_menu_ui(&mut self, ui: &mut egui::Ui, ws: &mut Workspace) {
        let Some(menu) = &mut self.add_menu else { return };
        let mut items = vec![(AddItem::Input, "Input".to_string()), (AddItem::Output, "Output".to_string())];
        items.extend(GateKind::ALL.iter().map(|g| (AddItem::Callee(g.name().into()), g.name().to_string())));
        let shadowed: HashSet<&str> = GateKind::ALL.iter().map(|g| g.name()).collect();
        for chip in ws.chip_names() {
            if chip != self.chip && !shadowed.contains(chip.as_str()) {
                items.push((AddItem::Callee(chip.clone()), chip));
            }
        }
        let query = menu.query.to_lowercase();
        let mut matches: Vec<&(AddItem, String)> =
            items.iter().filter(|(_, label)| label.to_lowercase().contains(&query)).collect();
        // Exact matches first, then ones that start with what was typed, so
        // "And" picks And rather than Nand.
        matches.sort_by_key(|(_, label)| {
            let label = label.to_lowercase();
            (label != query, !label.starts_with(&query))
        });

        let mut chosen: Option<AddItem> = None;
        let mut close = false;
        let area = egui::Area::new(egui::Id::new("add_menu"))
            .fixed_pos(menu.screen)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(180.0);
                    ui.label(egui::RichText::new("Add").color(theme::MUTED));
                    let search = ui.add(egui::TextEdit::singleline(&mut menu.query).hint_text("Search…"));
                    if menu.focus {
                        search.request_focus();
                        menu.focus = false;
                    }
                    if ui.input(|i| i.key_pressed(Key::Enter))
                        && let Some((item, _)) = matches.first()
                    {
                        chosen = Some(item.clone());
                    }
                    if ui.input(|i| i.key_pressed(Key::Escape)) {
                        close = true;
                    }
                    ui.separator();
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for (item, label) in &matches {
                            if ui.selectable_label(false, label.as_str()).clicked() {
                                chosen = Some(item.clone());
                            }
                        }
                    });
                });
            });
        if ui.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer() && chosen.is_none() {
            close = true;
        }
        let world = menu.world;
        if let Some(item) = chosen {
            self.add_menu = None;
            self.add(ws, &item, world);
        } else if close {
            self.add_menu = None;
        }
    }

    // ---- drawing ----

    fn draw(&self, painter: &egui::Painter, s: &Schematic, sim: &SimRunner, hit: Hit) {
        let z = self.zoom;
        self.draw_grid(painter);

        let selected_wire = self
            .selected_wire
            .as_ref()
            .and_then(|id| s.wires.iter().position(|w| s.nodes[w.source.0].key == id.node && w.source.1 == id.pin));
        // Floating (Z) wires first, so a live value drawn over the same spot wins.
        let mut order: Vec<(usize, &wafer_core::schematic::Wire)> = s.wires.iter().enumerate().collect();
        order.sort_by_key(|(_, w)| !self.val(sim, &w.signal).is_some_and(|v| v.iter().all(|&b| b == Logic::Z)));
        for (i, wire) in order {
            let value = self.val(sim, &wire.signal);
            let style = theme::wire_style(wire.width, value.as_deref());
            let stroke = Stroke::new((style.width * z).max(1.0), style.color);
            if selected_wire == Some(i) || hit == Hit::Wire(i) {
                let glow = Stroke::new(
                    stroke.width + 6.0,
                    theme::SELECTED.gamma_multiply(if selected_wire == Some(i) { 0.45 } else { 0.2 }),
                );
                for seg in &wire.segments {
                    painter.line_segment(
                        [self.to_screen(pos2(seg[0], seg[1])), self.to_screen(pos2(seg[2], seg[3]))],
                        glow,
                    );
                }
            }
            for seg in &wire.segments {
                let (a, b) = (self.to_screen(pos2(seg[0], seg[1])), self.to_screen(pos2(seg[2], seg[3])));
                if style.dashed {
                    painter.extend(Shape::dashed_line(&[a, b], stroke, 6.0 * z, 5.0 * z));
                } else {
                    painter.line_segment([a, b], stroke);
                }
            }
            for &(x, y) in &wire.dots {
                painter.circle_filled(self.to_screen(pos2(x, y)), (style.width + 1.5) * z, style.color);
            }
        }

        for (i, node) in s.nodes.iter().enumerate() {
            let at = self.to_screen(pos2(node.x, node.y));
            let size = vec2(node.w, node.h) * z;
            match &node.kind {
                NodeKind::Input { name, width } => {
                    let value = self.val(sim, name);
                    self.port_box(painter, at, size, *width, value.as_deref());
                    self.text(
                        painter,
                        at + vec2(-8.0 * z, PORT_H / 2.0 * z),
                        Align2::RIGHT_CENTER,
                        &port_label(name, *width),
                        12.0,
                        theme::TEXT,
                    );
                }
                NodeKind::Output { name, width } => {
                    let value = self.val(sim, name);
                    if *width == 1 {
                        let (fill, _) = theme::port_colors(value.as_deref());
                        painter.circle(at + size / 2.0, 11.0 * z, fill, Stroke::new(1.5 * z, theme::GATE_STROKE));
                    } else {
                        self.port_box(painter, at, size, *width, value.as_deref());
                    }
                    self.text(
                        painter,
                        at + vec2(size.x + 8.0 * z, PORT_H / 2.0 * z),
                        Align2::LEFT_CENTER,
                        &port_label(name, *width),
                        12.0,
                        theme::TEXT,
                    );
                }
                NodeKind::Gate { name, kind, label } if kind.is_box() => {
                    self.block(painter, node, at, size, label, name, kind.is_sequential());
                }
                NodeKind::Gate { name, kind, .. } => {
                    self.gate(painter, *kind, at);
                    self.text(
                        painter,
                        at + vec2(GATE_W / 2.0 * z, -9.0 * z),
                        Align2::CENTER_CENTER,
                        name,
                        11.0,
                        theme::MUTED,
                    );
                }
                NodeKind::Chip { name, chip, copies } => {
                    self.block(painter, node, at, size, &schematic::chip_title(chip, *copies), name, false)
                }
            }
            if self.selection.contains(&node.key) {
                let r = Rect::from_min_size(at, size).expand(5.0 * z);
                painter.rect_stroke(r, 6.0 * z, Stroke::new(1.5, theme::SELECTED), StrokeKind::Middle);
            }

            // Pins show up when you might want to wire them.
            let wiring = matches!(self.drag, Some(Drag::Wire { .. }));
            let near = matches!(hit, Hit::Node(n) | Hit::InPin(n, _) | Hit::OutPin(n, _) if n == i);
            if wiring || near {
                for (p, pin) in node.inputs.iter().enumerate() {
                    let big = hit == Hit::InPin(i, p);
                    self.pin(painter, pos2(pin.x, pin.y), big);
                }
                for (p, pin) in node.outputs.iter().enumerate() {
                    let big = hit == Hit::OutPin(i, p);
                    self.pin(painter, pos2(pin.x, pin.y), big);
                }
            }
        }

        for tag in &s.tags {
            let at = self.to_screen(pos2(tag.x, tag.y));
            match tag.kind {
                TagKind::Constant => {
                    let w = (tag.text.len() as f32 * 6.6 + 10.0) * z;
                    let r = Rect::from_min_max(at - vec2(w, 8.0 * z), at + vec2(0.0, 8.0 * z));
                    painter.rect(
                        r,
                        3.0 * z,
                        theme::GATE_FILL,
                        Stroke::new(1.0, theme::GATE_STROKE),
                        StrokeKind::Middle,
                    );
                    self.text(painter, at - vec2(5.0 * z, 0.0), Align2::RIGHT_CENTER, &tag.text, 11.0, theme::TEXT);
                }
                TagKind::Bits => self.text(painter, at, Align2::RIGHT_CENTER, &tag.text, 10.0, theme::MUTED),
            }
        }

        for wire in &s.wires {
            if wire.width == 1 {
                continue;
            }
            if let (Some((x, y)), Some(value)) = (wire.label_at, self.val(sim, &wire.signal)) {
                let style = theme::wire_style(wire.width, Some(&value));
                let at = self.to_screen(pos2(x, y));
                let text = format_bits(&value);
                let r = Rect::from_min_size(
                    at - vec2(3.0 * z, 7.0 * z),
                    vec2((text.len() as f32 * 6.6 + 6.0) * z, 14.0 * z),
                );
                painter.rect_filled(r, 3.0 * z, theme::BACKGROUND);
                self.text(painter, at, Align2::LEFT_CENTER, &text, 11.0, style.color);
            }
        }

        match &self.drag {
            Some(Drag::Select { from }) => {
                if let Some(to) = painter.ctx().pointer_hover_pos() {
                    let r = Rect::from_two_pos(*from, to);
                    painter.rect(
                        r,
                        0.0,
                        theme::SELECTED.gamma_multiply(0.08),
                        Stroke::new(1.0, theme::SELECTED),
                        StrokeKind::Middle,
                    );
                }
            }
            Some(Drag::Wire { from }) => {
                let start = match *from {
                    Hit::OutPin(n, p) => Some(&s.nodes[n].outputs[p]),
                    Hit::InPin(n, p) => Some(&s.nodes[n].inputs[p]),
                    _ => None,
                };
                if let (Some(pin), Some(to)) = (start, painter.ctx().pointer_hover_pos()) {
                    let a = self.to_screen(pos2(pin.x, pin.y));
                    painter.extend(Shape::dashed_line(&[a, to], Stroke::new(2.0, theme::SELECTED), 6.0, 4.0));
                }
            }
            _ => {}
        }
    }

    fn draw_grid(&self, painter: &egui::Painter) {
        let step = 20.0 * self.zoom;
        if step < 8.0 {
            return;
        }
        let origin = self.rect.min + self.offset;
        let start_x = origin.x + ((self.rect.min.x - origin.x) / step).ceil() * step;
        let start_y = origin.y + ((self.rect.min.y - origin.y) / step).ceil() * step;
        let mut y = start_y;
        while y < self.rect.max.y {
            let mut x = start_x;
            while x < self.rect.max.x {
                painter.circle_filled(pos2(x, y), 1.0, theme::GRID);
                x += step;
            }
            y += step;
        }
    }

    fn text(&self, painter: &egui::Painter, at: Pos2, align: Align2, text: &str, size: f32, color: Color32) {
        if size * self.zoom >= 5.0 && !text.is_empty() {
            painter.text(at, align, text, FontId::monospace(size * self.zoom), color);
        }
    }

    fn pin(&self, painter: &egui::Painter, world: Pos2, big: bool) {
        let r = if big { 6.0 } else { 3.5 } * self.zoom.max(0.6);
        painter.circle(
            self.to_screen(world),
            r,
            theme::BACKGROUND,
            Stroke::new(1.5, if big { theme::SELECTED } else { theme::GATE_STROKE }),
        );
    }

    fn port_box(&self, painter: &egui::Painter, at: Pos2, size: Vec2, width: usize, value: Option<&[Logic]>) {
        let (fill, text) = theme::port_colors(value);
        let border = if width > 1 { theme::BUS } else { theme::GATE_STROKE };
        painter.rect(
            Rect::from_min_size(at, size),
            5.0 * self.zoom,
            fill,
            Stroke::new(1.5 * self.zoom, border),
            StrokeKind::Middle,
        );
        let shown = value.map(format_bits).unwrap_or_default();
        self.text(painter, at + size / 2.0, Align2::CENTER_CENTER, &shown, 12.0, text);
    }

    fn gate(&self, painter: &egui::Painter, kind: GateKind, at: Pos2) {
        let z = self.zoom;
        let (shape, triangles) = gate_mesh(kind);
        let map = |p: Pos2| at + p.to_vec2() * z;
        let stroke = Stroke::new((2.0 * z).max(1.0), theme::GATE_STROKE);
        for &(y, x) in &shape.stubs {
            if x > 0.0 {
                painter.line_segment([map(pos2(0.0, y)), map(pos2(x, y))], stroke);
            }
        }
        for line in &shape.extra {
            painter.add(Shape::line(line.iter().map(|&p| map(p)).collect(), stroke));
        }
        let points: Vec<Pos2> = shape.body.iter().map(|&p| map(p)).collect();
        let mut mesh = egui::Mesh::default();
        for &p in &points {
            mesh.colored_vertex(p, theme::GATE_FILL);
        }
        for t in triangles.chunks(3) {
            mesh.add_triangle(t[0], t[1], t[2]);
        }
        painter.add(Shape::mesh(mesh));
        painter.add(Shape::closed_line(points, stroke));
        let stub_start = if shape.bubble {
            painter.circle(map(pos2(shape.tip + 4.5, 25.0)), 4.5 * z, theme::GATE_FILL, stroke);
            shape.tip + 9.0
        } else {
            shape.tip
        };
        painter.line_segment([map(pos2(stub_start, 25.0)), map(pos2(GATE_W, 25.0))], stroke);
        for &(at, label) in &shape.labels {
            self.text(painter, map(at), Align2::LEFT_CENTER, label, 9.0, theme::MUTED);
        }
    }

    /// A chip, or a box-shaped built-in like Dff and Ram: a labeled box with
    /// its pin names inside, and a clock triangle if it ticks.
    #[allow(clippy::too_many_arguments)]
    fn block(
        &self,
        painter: &egui::Painter,
        node: &wafer_core::schematic::Node,
        at: Pos2,
        size: Vec2,
        title: &str,
        name: &str,
        clocked: bool,
    ) {
        let z = self.zoom;
        let r = Rect::from_min_size(at, size);
        painter.rect(r, 6.0 * z, theme::GATE_FILL, Stroke::new(1.5 * z, theme::GATE_STROKE), StrokeKind::Middle);
        self.text(painter, at + vec2(size.x / 2.0, 16.0 * z), Align2::CENTER_CENTER, title, 12.0, theme::TEXT);
        painter.line_segment([at + vec2(0.0, 28.0 * z), at + vec2(size.x, 28.0 * z)], Stroke::new(1.0, theme::GRID));
        for pin in &node.inputs {
            self.text(
                painter,
                self.to_screen(pos2(pin.x + 8.0, pin.y)),
                Align2::LEFT_CENTER,
                &pin.name,
                11.0,
                theme::MUTED,
            );
        }
        for pin in &node.outputs {
            self.text(
                painter,
                self.to_screen(pos2(pin.x - 8.0, pin.y)),
                Align2::RIGHT_CENTER,
                &pin.name,
                11.0,
                theme::MUTED,
            );
        }
        if clocked {
            let b = at.y + size.y;
            let points = vec![pos2(at.x, b - 16.0 * z), pos2(at.x + 9.0 * z, b - 10.0 * z), pos2(at.x, b - 4.0 * z)];
            painter.add(Shape::line(points, Stroke::new(1.5 * z, theme::GATE_STROKE)));
        }
        self.text(painter, at + vec2(size.x / 2.0, -9.0 * z), Align2::CENTER_CENTER, name, 11.0, theme::MUTED);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddItem {
    Input,
    Output,
    Callee(String),
}

/// Lays out a chip from its saved positions (plus any boxes mid-move), or
/// automatically if it has never been arranged by hand.
fn build_schematic(
    ws: &Workspace,
    chip: &str,
    moving: &HashMap<String, (f32, f32)>,
    copies: usize,
) -> Option<Schematic> {
    let sources = &ws.project.sources;
    match ws.positions(chip) {
        Some(mut saved) => {
            saved.extend(moving.iter().map(|(k, v)| (k.clone(), *v)));
            schematic::arrange_copies(&ws.design, sources, chip, &saved, copies)
        }
        None if moving.is_empty() => schematic::layout_copies(&ws.design, sources, chip, copies),
        None => {
            let auto = schematic::layout_copies(&ws.design, sources, chip, copies)?;
            let mut positions: HashMap<String, (f32, f32)> =
                auto.nodes.iter().map(|n| (n.key.clone(), (n.x, n.y))).collect();
            positions.extend(moving.iter().map(|(k, v)| (k.clone(), *v)));
            schematic::arrange_copies(&ws.design, sources, chip, &positions, copies)
        }
    }
}

/// Gate shapes and their triangles, computed once.
fn gate_mesh(kind: GateKind) -> &'static (GateShape, Vec<u32>) {
    static MESHES: OnceLock<Vec<(GateShape, Vec<u32>)>> = OnceLock::new();
    let meshes = MESHES.get_or_init(|| {
        GateKind::ALL
            .iter()
            .map(|&k| {
                let shape = gate_shape(k);
                let triangles = triangulate(&shape.body);
                (shape, triangles)
            })
            .collect()
    });
    &meshes[GateKind::ALL.iter().position(|&k| k == kind).unwrap_or(0)]
}

/// A wire dropped between pins of different widths.
struct BitPicker {
    sink: Sink,
    /// The pin as the question names it: `sum`, or `lo.a`.
    sink_label: String,
    sink_width: usize,
    /// The pin takes whatever width is wired to it (a fresh gate or 1-bit
    /// chip), so any bits can go in, starting at bit 0.
    sink_adapts: bool,
    /// The pin belongs to a 1-bit chip spread across a bus, so one bit of
    /// the source can feed every copy.
    sink_spread: bool,
    /// As written in the file: `a`, `lo.sum`.
    source: String,
    source_width: usize,
    screen: Pos2,
    /// The lowest bit, for a range that isn't one of the buttons.
    custom: usize,
    /// Picking any bits of the source and where they land (Option-drop).
    free: Option<FreePick>,
}

/// Bits `hi..lo` of the source, landing in the pin from bit `at` up.
#[derive(Clone, Copy)]
struct FreePick {
    hi: usize,
    lo: usize,
    at: usize,
}

/// True when a pin (an input, or the output for `None`) takes its width
/// from whatever gets wired to it, because nothing has fixed it yet: wiring
/// a 16-bit bus into a fresh `Nand()`, or into our own 1-bit `Xor`, makes
/// it 16 bits wide rather than asking which bit.
fn adapts_to_wire(ws: &Workspace, chip: &str, node: &schematic::Node, input: Option<usize>) -> bool {
    let find = |name: &str| ws.design.chips.get(chip).and_then(|c| c.instances.iter().find(|i| i.name == name));
    if let NodeKind::Chip { name, chip: callee, .. } = &node.kind {
        // A 1-bit chip spreads across any bus wired into it while it's
        // still 1 bit wide (other inputs on 1-bit signals just feed every
        // copy). Its output adapts only before anything is wired in.
        let Some(inst) = find(name) else { return false };
        let one = wafer_core::check::Callee::Chip { name: callee.clone(), copies: 1 };
        let bitwise = wafer_core::check::is_bitwise(&ws.design.signature(&one));
        return bitwise
            && match input {
                Some(_) => copies_of(ws, chip, node) <= 1,
                None => inst.inputs.iter().all(Option::is_none),
            };
    }
    let NodeKind::Gate { name, kind, .. } = &node.kind else { return false };
    let Some(inst) = find(name) else {
        return false;
    };
    let group = input.map_or(PortWidth::Bus, |p| kind.port_width(p));
    if group == PortWidth::Bit || (group == PortWidth::Bus && kind.fixed_width().is_some()) {
        return false;
    }
    // A plain number doesn't fix the width: `0` fits any bus.
    let fixes = |e: &Option<wafer_core::ast::Expr>| {
        e.as_ref().is_some_and(|e| !matches!(e.kind, wafer_core::ast::ExprKind::Literal(_)))
    };
    !inst.inputs.iter().enumerate().any(|(p, e)| Some(p) != input && fixes(e) && kind.port_width(p) == group)
}

/// `0x0001` for a bus, written with as many digits as it's wide.
fn constant_text(value: u64, width: usize) -> String {
    if width.is_multiple_of(4) { format!("0x{value:0w$X}", w = width / 4) } else { format!("0b{value:0width$b}") }
}

/// `42`, `0x2A` or `0b101010`.
fn parse_constant(text: &str) -> Option<u64> {
    let text = text.trim().replace('_', "");
    let lower = text.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()
    } else if let Some(bin) = lower.strip_prefix("0b") {
        u64::from_str_radix(bin, 2).ok()
    } else {
        lower.parse().ok()
    }
}

/// How many copies a chip box runs, as the file says (1 for anything else).
fn copies_of(ws: &Workspace, chip: &str, node: &schematic::Node) -> usize {
    let NodeKind::Chip { name, .. } = &node.kind else { return 1 };
    let inst = ws.design.chips.get(chip).and_then(|c| c.instances.iter().find(|i| &i.name == name));
    match inst.map(|i| &i.callee) {
        Some(wafer_core::check::Callee::Chip { copies, .. }) => (*copies).max(1),
        _ => 1,
    }
}

/// The edit-level name for an input pin.
pub fn sink_of(s: &Schematic, node: usize, pin: usize) -> Option<Sink> {
    let n = s.nodes.get(node)?;
    match &n.kind {
        NodeKind::Gate { name, .. } | NodeKind::Chip { name, .. } => {
            Some(Sink::Instance { name: name.clone(), port: n.inputs.get(pin)?.name.clone() })
        }
        NodeKind::Output { name, .. } => Some(Sink::Output(name.clone())),
        NodeKind::Input { .. } => None,
    }
}

/// How to refer to an output pin in a `.chip` file: `a`, `n1` or `h1.sum`.
pub fn source_text(s: &Schematic, node: usize, pin: usize) -> Option<String> {
    let n = s.nodes.get(node)?;
    match &n.kind {
        NodeKind::Input { name, .. } => Some(name.clone()),
        NodeKind::Gate { name, .. } => Some(name.clone()),
        NodeKind::Chip { name, .. } if n.outputs.len() == 1 => Some(name.clone()),
        NodeKind::Chip { name, .. } => Some(format!("{name}.{}", n.outputs.get(pin)?.name)),
        NodeKind::Output { .. } => None,
    }
}

fn first_free(info: &wafer_core::check::ChipInfo, names: &[&str], fallback: &str) -> String {
    names
        .iter()
        .find(|n| !info.names.contains_key(**n))
        .map(|n| n.to_string())
        .unwrap_or_else(|| fresh_name(info, fallback))
}

fn distance_to_segment(p: Pos2, seg: [f32; 4]) -> f32 {
    let (a, b) = (pos2(seg[0], seg[1]), pos2(seg[2], seg[3]));
    let ab = b - a;
    let t = if ab.length_sq() == 0.0 { 0.0 } else { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) };
    p.distance(a + ab * t)
}
