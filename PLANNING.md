# Wafer: Planning

**Wafer** is our chip design software. The chip we'll build with it is unnamed until Phase 3.

This is our shared planning doc. Either of us can edit it. A decision moves from **Proposed** to **Decided** once we both agree.

## Vision

A "Blender for computer chips": a native desktop app for designing, simulating and testing digital chips visually. It's built so a human (dragging things on a canvas) and Claude (editing text files) can work on the same design at the same time.

The end goal is to design our own chip with Wafer, and possibly run it on real hardware (an FPGA board, or a fabricated chip through a program like Tiny Tapeout).

## Decided

### Round 1 (2026-10-23)

| Topic | Decision |
|---|---|
| Platform | Native desktop app |
| Design style | Visual gates and wires on a canvas, with a readable text file underneath as the source of truth |
| Target chip | Undecided. Keep the tool general; decide before Phase 3 |
| Audience | Assumes the user knows gates and binary. The tool explains deeper hardware concepts (timing, clocks, synthesis) |

### Round 2 (2026-10-23)

| Topic | Decision |
|---|---|
| Language | Rust |
| UI | egui + egui_dock for dockable, Blender-style panels |
| Rendering | wgpu (GPU) for the schematic canvas and, later, the 3D silicon view |
| Project layout | Cargo workspace: `crates/wafer-core` (parser, netlist, simulator, test runner; no UI code), `crates/wafer-cli` (the `wafer` command: `wafer check`, `wafer test`, `wafer render` to PNG so Claude can see designs), `crates/wafer-app` (the desktop app) |
| Built-in parts | Gates and memory built in and fast. You can still build your own from NAND, and tests check they match the built-ins |
| Timing | Gate delays: every gate takes one time step, so ripples and glitches are visible and latches built from raw gates work. Realistic per-gate delays and a fast "turbo" mode can come later |
| Signal values | 0, 1, X (unknown), Z (disconnected). Matches Verilog |
| Clock | One global clock. Registers and flip-flops tick on it without a wire |
| Default layout | Blender classic: hierarchy on the left, canvas in the middle, inspector on the right, waveform timeline along the bottom. Every panel is dockable |
| File format | The v0 format below |

### Round 3 (2026-10-23)

| Topic | Decision |
|---|---|
| Phase 1 scope | Core sandbox **plus** waveform timeline, code panel, Make Chip + dive inside, and buses. Split into milestones 1a–1d below |
| Wires | Right-angle. The app auto-routes, you can drag segments, and dots mark junctions |
| Controls | Blender keys + mouse menus (keymap below). Trackpad-friendly navigation |
| Look | Terminal green (palette below). A light theme toggle can come later |
| Names | Software: **Wafer**. Chip: decide in Phase 3 |
| Shared buses | `wire data[8]` declares a shared wire, and `data <- TriBuf(x, en)` adds a driver. Two drivers on at once = X. Needed by Phase 2 |

## Core principles

1. **Instant feedback.** Everything is live: flip an input and watch the signal ripple through.
2. **Build up from nothing.** Start with NAND and build everything else from your own parts.
3. **Everything is text underneath.** The visual view and the text file are two views of the same design. You edit visually, Claude edits text, and the app reloads live.
4. **Tests are built in.** Every chip can have a test file, runnable with one click or one command.

## UI

| Blender | Wafer |
|---|---|
| Outliner | Hierarchy: chips inside chips (CPU → ALU → Adder → gates) |
| Node editor | Schematic canvas where wires light up when they carry a 1 |
| Tab into edit mode / node groups | Tab or double-click to dive into a chip; select gates and hit Cmd+G for "Make Chip" |
| Timeline | Waveform timeline for scrubbing through time steps and clock cycles |
| Properties panel | Inspector |
| Text editor | Code panel with the live `.chip` text |
| Workspaces | Design / Simulate / Program / Silicon (3D layer view) |

Default layout (the code panel docks as a tab beside the inspector):

```
┌─────────┬──────────────────┬──────────┐
│Hierarchy│                  │Inspector │
│ ▾ CPU   │                  │ Xor "sum"│
│  ▾ ALU  │ schematic canvas │ in:  a, b│
│   Adder │                  │ out: sum │
├─────────┴──────────────────┴──────────┤
│ clk ▁▔▁▔▁▔▁▔  a ▁▁▔▔▔▁  sum ▁▔▔▁▁▔    │
└───────────────────────────────────────┘
```

### Keymap

| Key | Action |
|---|---|
| Shift+A | Add part (search menu) |
| G | Grab / move selection |
| X or Delete | Delete |
| Shift+D | Duplicate |
| Cmd+G | Make Chip from selection |
| Tab / Shift+Tab | Dive into the selected chip / back out |
| F | Frame everything |
| Cmd+K | Command search |
| Cmd+Z / Cmd+Shift+Z | Undo / redo |
| Space | Play / pause simulation |
| → | Step one time step |
| Two-finger drag / pinch | Pan / zoom |
| Right-click | Context menu |

Everything on the keymap is also reachable from menus.

### Terminal green palette

| Element | Color |
|---|---|
| Canvas background | `#0f1411` |
| Grid dots | `#1e2a22` |
| Gate fill / outline | `#16201a` / `#5f8f6c` |
| Text | `#c8e6cf` (muted `#6f8f78`) |
| Signal 1 | `#4dff88`, thicker line |
| Signal 0 | `#25523a` |
| Signal X | `#ff5f56` |
| Signal Z | `#5d6b62`, dashed |
| Bus | `#e6d36a`, thick, with a hex value label |

Built-in gates use classic logic symbols. Custom chips are labeled boxes.

## File format v0

```
// FullAdder.chip
chip FullAdder {
  in  a, b, cin
  out sum, cout

  ab   = Xor(a, b)          // one line = one box on the canvas
  sum  = Xor(ab, cin)
  c1   = And(a, b)
  c2   = And(ab, cin)
  cout = Or(c1, c2)
}

test FullAdder {
  //  a b cin | sum cout
      0 0 0   | 0   0
      0 1 1   | 0   1
      1 1 1   | 1   1
}
```

```
// Counter4.chip (Phase 2: needs the clock and registers)
chip Counter4 {
  in  reset
  out count[4]

  next  = Add4(a: count, b: 1)
  d     = Mux4(a: next, b: 0, sel: reset)
  count = Register4(d)        // ticks on the global clock
}

test Counter4 {
  set reset = 1; tick; expect count = 0
  set reset = 0
  tick;    expect count = 1
  tick 14; expect count = 15
  tick;    expect count = 0   // wraps around
}
```

Rules:

- **One line is one box on the canvas.** `x = Gate(...)` creates a box named `x`, and its output wire is also called `x`. Chips with several outputs use dotted names like `fa0.sum` and `fa0.cout`.
- **Arguments** go by position (`Xor(a, b)`) or by name (`Mux(a: x, b: y, sel: s)`).
- **Buses:** `in a[8]` declares an 8-bit bus, `a[0]` is the lowest bit, `a[7:4]` is a slice, and `{hi, lo}` joins buses.
- **Shared wires:** `wire data[8]` plus one `data <- TriBuf(x, en)` line per driver.
- **Numbers:** `0`, `1`, `0xFF`, `0b1010`.
- **Comments:** `//`.
- **Tests live in the same file as the chip.** Use truth tables for plain logic, and `set` / `tick` / `expect` steps for anything with a clock.
- **Canvas positions** go in `Name.layout.json` next to the chip, written by the app. A chip with no saved positions gets auto-laid-out.
- **Canvas edits rewrite only the lines they change**, so comments and line order in a `.chip` file survive.
- **No loops yet.** One line, one box. Revisit when writing out repeated lines gets painful.

## Simulation model

- **Values:** 0, 1, X, Z.
  - X spreads through logic, for example `And(X, 1) = X` but `And(X, 0) = 0`.
  - Z is what a disconnected or switched-off driver outputs.
- **Timing:** event-driven, one time step per gate. A clock tick runs until every signal settles, or is reported as oscillating.
- **Built-in parts:** Nand, And, Or, Not, Xor, Nor, Xnor, the constants 0 and 1 (Phase 1); Dff (flip-flop), Ram, Rom and TriBuf (Phase 2).

## Roadmap

### Phase 1: Logic Sandbox

- **1a, Engine. ✅ Done 2026-10-23.**
  - Parser for the v0 format (including buses), chips using other chips, the 0/1/X/Z gate-delay simulator, truth-table tests.
  - `wafer check`, `wafer test`, `wafer sim`, `wafer render`. Clear errors with line numbers.
  - *Done when:* Xor built from Nand, and a HalfAdder built from that, pass `wafer test` and render to PNG.
  - Also built: chips named after a built-in gate replace it and are checked against it automatically. Example chips in `chips/` go from Not up to Add4 and SRLatch.
  - Known limits:
    - Auto-layout gets busy on bus-heavy chips like Add4, and two different wires can briefly run along the same line.
    - Tests are truth tables only; `set` / `tick` / `expect` steps come with the clock in Phase 2.
    - Shared wires (`<-`) and loops aren't parsed yet.
- **1b, Canvas. ✅ Done 2026-10-24.**
  - App shell in the Blender classic layout.
  - Canvas: pan, zoom, add/move/delete/duplicate, right-angle wires, box select, buses drawn thick with value labels.
  - Switches and lamps, live wire colors, play/pause/step and a speed slider.
  - Undo/redo. Saving `.chip` and `.layout.json`, live reload with last-good fallback, auto-layout, a Run tests button.
  - *Done when:* you build the half adder on the canvas and its tests pass.
  - Verified by a UI test that builds a half adder from an empty chip with simulated mouse and keyboard input, and checks the file text and test results.
  - Changes from the plan:
    - Unconnected inputs and undriven outputs are now warnings (they read as Z), so half-built chips still simulate.
    - The canvas draws through egui's painter, which runs on wgpu. A custom wgpu pipeline waits until big chips need it.
    - Tests re-run automatically whenever the chip changes; the button is still there.
  - Not yet:
    - Dragging individual wire segments.
    - Cmd+K command search.
    - A router that steers wires around boxes (wires can cross boxes after you move things; move the box to tidy up).
- **1c, Text & time.**
  - Editable code panel synced with the canvas.
  - Waveform timeline showing every signal over time steps, with scrubbing.
  - *Done when:* you watch a carry ripple through a 4-bit adder in the timeline.
- **1d, Hierarchy.**
  - Make Chip from a selection (Cmd+G).
  - Dive in and out with live values inside, and a working hierarchy tree.
  - *Done when:* you select a few gates, turn them into a chip, reuse it, and dive back inside it.

### Phase 2: Time & Memory

Clock, flip-flops, registers, Ram/Rom, shared buses with tri-state drivers, `tick` tests. *Milestone: an 8-bit counter.*

### Phase 3: Computer

Decide the target chip and its name. Instruction set, assembler, CPU debugger (registers, program counter, memory view). *Milestone: our CPU runs a real program.*

### Phase 4: Silicon

Verilog export, Yosys synthesis, real gate counts, 3D layer viewer, FPGA, maybe Tiny Tapeout.

## Open questions (later)

- Target chip and its name (before Phase 3)
- Loop syntax for repeated parts
- Realistic per-gate delays and a turbo mode
- Light theme toggle

## Setup status

- Mac: Apple Silicon, macOS 26.6.2. git installed.
- Rust: installed (cargo 1.99.1 in `~/.cargo/bin`).
- Git repo initialized on `main`. `cargo wafer ...` runs the CLI and `cargo app` opens the desktop app, both from the repo root.
- App stack pinned to egui/eframe 0.36 because egui_dock 0.21 hasn't caught up with egui 0.37 yet.
