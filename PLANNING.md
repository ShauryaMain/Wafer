# Wafer: Planning

**Wafer** is our chip design software. The first chip we're building with it is **Crumb**, a 16-bit CPU for a tiny game console.

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
| Target chip | Undecided. Keep the tool general; decide before Phase 3 (now decided: **Crumb**, see Phase 3 rounds) |
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
| Names | Software: **Wafer**. Chip: **Crumb** (decided in Phase 3) |
| Shared buses | `wire data[8]` declares a shared wire, and `data <- TriBuf(x, en)` adds a driver. Two drivers on at once = X. Needed by Phase 2 |

### Phase 3, round 1 (2026-10-24)

| Topic | Decision |
|---|---|
| Goal | A simple CPU that runs games like Pong, Tetris and Space Invaders |
| CPU | Our own **16-bit** design. (RISC-V RV32I was picked first, then swapped for 16 bits: enough for these games, and half the wiring) |
| Style | Stack machine: instructions take their inputs from a stack and push results back |
| Timing | Several clock ticks per instruction (multi-cycle), so each step is easy to watch |
| Outputs | All four: number display, LEDs and switches, a tiny screen, a text console |

### Phase 3, round 2 (2026-10-24)

| Topic | Decision |
|---|---|
| Name | **Crumb** |
| Screen | 128×64, 1 bit per pixel (black and white). Memory-mapped: 512 words, 8 words per row |
| Controls | 8-button gamepad read as one word: up, down, left, right, A (Z key), B (X key), Start (Enter), Select (Shift) |
| Instruction format | Packed, one 16-bit word per instruction, in the style of the J1 Forth CPU (draft below) |
| Memory | Word-addressed: every address holds one 16-bit word, no bytes |
| Game helpers | A frame counter and a random-number port |
| First games | Pong first, then Tetris, then Space Invaders |

### Phase 3, round 3 (2026-10-24)

| Topic | Decision |
|---|---|
| Assembly | Forth-style words (example below). Words run left to right on the stack, and most words are one instruction. `if`/`else`/`then` and `begin`/`until` turn into jumps, `: name ... ;` defines a function, and `const`/`var` name numbers and RAM words |
| Instruction set | Lives in Rust code (assembler and reference CPU), with a written spec in `docs/crumb.md`. The ALU instructions are bit fields, not a plain opcode list, so a table file would cost more than it saves |
| Rom program | The chip names a default program: `rom = Rom(pc, file: "pong.asm")`. Wafer assembles it on load, and again whenever the `.asm` changes. The app also has a cartridge menu to swap games without editing the chip |
| Frames | The frame counter goes up every N clock ticks. Every frame gets the same instruction budget, games behave the same at any speed, and tests can replay a game exactly. Turbo only changes how fast frames arrive in real time. N is picked after measuring Crumb in gates |
| Milestones | 3a–3e as drafted below |

```
const PAD = 0x6000
const UP  = 0b0001
var paddle_y

: move_paddle
  PAD @ UP and if
    paddle_y @ 1 - paddle_y !
  then ;
```

### Phase 3, round 4: milestone 3b (2026-10-24)

| Topic | Decision |
|---|---|
| Gates | Our own, all the way down: our Nand-built And, Or, Xor and Not replace the built-ins everywhere, alongside the built-in Mux, Dff, Ram and Rom. About 3× more gates to simulate; turbo mode (3d) may swap in built-ins wherever it has proven ours identical |
| Chips | `Crumb` is just the CPU, with a memory bus (address, data in, data out, write) and a `reset` input. `Console` is the board: it wires Crumb to the Rom, RAM, screen and ports |
| Order | Bottom-up: each part is built and tested on its own, then Crumb, then Console |
| Who builds | **The user builds the parts on the canvas.** Claude writes each part's tests first (a `.chip` file with the ports and a `test` block, body empty), reviews the result, and helps when a test fails |
| Timing | 2 ticks per instruction, as in the spec. A stack write lands during the next instruction's fetch tick, when that stack's Ram isn't busy, so each stack is one plain `Ram` |
| Reset | Crumb has a `reset` input that zeroes `PC`, `T` and both stack pointers, since flip-flops start unknown (X) |
| Checking | `wafer run prog.asm --gates` runs the program on the gate-level Console and the reference Crumb side by side, and stops at the first instruction where they differ |
| Chips on buses | (Decided after Add16.) **Any chip whose inputs and outputs are all 1 bit works on buses bit by bit**, the way built-in gates do: `Xor(a, b)` on 16-bit buses is 16 of our Xors side by side. 1-bit inputs feed every copy, and the bus inputs must all be the same width. On the canvas it's one box labelled `Xor ×16`, and inside it's the same circuit with 16-bit wires. HalfAdder and FullAdder work this way too: on buses they're 16 separate adders with no carry between them |

### Phase 3, round 5: the Console (2026-10-25)

| Topic | Decision |
|---|---|
| Frame length | **4096 ticks**: 2048 instructions per frame. Chosen after measuring the gate-level Crumb at about 100,000 ticks a second, so games run at roughly 20 frames a second even in gates, before turbo mode. (Was 16384, provisional) |
| Unused addresses | Full decoding, as the spec says: only the exact addresses in the memory map answer; everything else reads 0 and ignores writes |
| Program | The Console's Rom holds `count.asm` by default (milestone 3b's finish line); the cartridge menu (3c) swaps it |
| Structure | Four sub-chips first, each with its own tests: `AddressDecoder` (which device an address means), `Ports` (the LEDs and number registers, console characters, and the port Crumb reads), `FrameCounter` (counts ticks; its top bits are `FRAME`) and `Random` (the LFSR). Then `Console` wires them to Crumb, the Rom, the RAM and the screen |

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
- **Text in quotes** is only for settings that aren't wires: a Rom's `file: "pong.asm"`.
- **No loops yet.** One line, one box. Revisit when writing out repeated lines gets painful.

## Simulation model

- **Values:** 0, 1, X, Z.
  - X spreads through logic, for example `And(X, 1) = X` but `And(X, 0) = 0`.
  - Z is what a disconnected or switched-off driver outputs.
- **Timing:** event-driven, one time step per gate. A clock tick runs until every signal settles, or is reported as oscillating.
- **Built-in parts:** Nand, And, Or, Not, Xor, Nor, Xnor, the constants 0 and 1 (Phase 1); Dff (flip-flop), Ram, TriBuf and Mux (Phase 2); Rom (Phase 3, milestone 3a).

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
- **1c, Text & time. ✅ Done 2026-10-24.**
  - Editable code panel synced with the canvas.
  - Waveform timeline showing every signal over time steps, with scrubbing.
  - *Done when:* you watch a carry ripple through a 4-bit adder in the timeline.
  - Built:
    - Timeline: one lane per signal at the chip's own level, buses drawn as hex segments, X shown red, markers where inputs changed. Click or drag to scrub (the canvas and Inspector show that step), double-click for live, pinch to zoom, Fit, and a Reset that puts every gate back to X.
    - Code panel: syntax colors, problems underlined, saves 0.35 s after you stop typing (or Cmd+S). Selecting a box highlights its line; putting the cursor on a line selects its box. If the file changes outside the app while you're typing, it asks which version to keep.
  - Not yet: picking which signals the timeline shows (it shows the chip's own level; inner signals come with 1d's dive-in).
- **1d, Hierarchy. ✅ Done 2026-10-24.**
  - Make Chip from a selection (Cmd+G).
  - Dive in and out with live values inside, and a working hierarchy tree.
  - *Done when:* you select a few gates, turn them into a chip, reuse it, and dive back inside it.
  - Built:
    - Make Chip writes the new chip to its own file. Wires crossing the selection become its inputs and outputs, and the parent's behavior is unchanged (checked by re-running its tests). One undo takes it all back.
    - Tab or double-click a chip box to look inside; Shift+Tab, double-clicking empty space, or the breadcrumbs go back out. Inside, values (and the timeline) come from that particular box in the outer chip's simulation, and edits change the chip's definition everywhere.
    - The Chips panel shows the open chip's insides as a tree.
  - Also added to the code editor (requested): Format (button or Shift+Alt+F, plus `wafer fmt`) and autocompletion (gates and chips after `=`, outputs after `name.`, signals and named inputs inside calls, with the call's signature as a hint).

### Phase 2: Time & Memory ✅ Done 2026-10-24

Clock, flip-flops, registers, Ram/Rom, shared buses with tri-state drivers, `tick` tests. *Milestone: an 8-bit counter.*

- Built:
  - New built-ins: `Dff(d)` (a flip-flop, or a whole register on a bus), `Ram(addr, data, write)` (reads any time, writes on a tick), `TriBuf(a, en)`, and `Mux(a, b, sel)`.
  - One global clock: Tick (T key or button), or Auto at a chosen rate. A tick settles the logic, samples every flip-flop at once, stores RAM writes, then the logic settles again.
  - Shared buses: `data <- TriBuf(x, en)` lines, with drivers named `data_d1`, `data_d2`, ... Drivers that let go are Z; two at once is X.
  - Tests can use `set`, `tick` and `expect` steps (mixed with truth-table rows); `expect` checks outputs, wires or boxes.
  - The timeline has a clk lane, and the Inspector shows a Ram's contents live.
  - Examples: Add8, Register8, Counter8 (the milestone, tested through 255 ticks and the wrap), BusDemo, Scratchpad.
- Changes from the plan:
  - `Mux` is built in too: registers and counters need one constantly. You can still build your own from gates.
  - Flip-flops start unknown (X), like real ones, so sequential chips need a reset. RAM starts cleared to 0.
  - ROM moved to Phase 3: it needs a program to fill it, which comes with the assembler.

### Phase 3: Computer

Build **Crumb**, a 16-bit stack CPU, plus the console around it: instruction set, assembler, ROM loaded from assembled programs, screen and gamepad, CPU debugger (registers, program counter, stacks, memory view). *Milestone: Crumb plays Pong.*

#### Crumb

The spec is [docs/crumb.md](docs/crumb.md): memory map, instruction encoding, the 16 ALU operations, timing, I/O ports and the assembler language. In short:

- **Memory map**, by word address. The top 3 address bits pick the device, and every device sits below 0x8000, so a single 15-bit push reaches any address. ROM `0x0000–0x1FFF` (8K words), RAM `0x2000–0x3FFF` (8K), screen `0x4000–0x41FF` (512), I/O ports from `0x6000`.
- **Instructions**, J1 style, one word each: push a 15-bit number, jump, jump if zero, call, or an ALU instruction whose fields pick the operation, the stack moves, a return, and a memory write. Jump targets are 13 bits, which is why the ROM is 8K words.
- **Stacks:** a data stack and a return stack, 32 deep each, with the top of the data stack (`T`) in a register.

#### Milestones (Decided)

- **3a, ISA and tools. ✅ Done 2026-10-24.** Write the Crumb instruction spec (`docs/crumb.md`), the Forth-style assembler (`wafer asm`), a `Rom` built-in that loads assembled programs (this needs string arguments like `file: "pong.asm"` in the `.chip` language), and a fast reference Crumb written in Rust. The reference CPU lets us write and test games at full speed while the gates are being built, and later checks the gate-level CPU instruction by instruction.
  - Built:
    - A new `crates/crumb`: the instruction encoding and disassembler, the assembler, and the reference Crumb (the whole console: CPU, memory, screen and ports, tick for tick).
    - `wafer asm file.asm` (`--list` for a listing, `-o` for a `.hex` file) and `wafer run file.asm` (`--steps`, `--frames`, `--pad up,a`, `--screen` to print the screen, `--png` to save it).
    - `Rom(addr, file: "pong.asm")` in chips. It loads `.asm` (assembled on the spot) or `.hex` files from next to the chip, reloads when they change, and assembler mistakes show up in Problems pointing into the `.asm` file. Rewiring a Rom on the canvas keeps its `file:`.
    - The Inspector lists a selected Rom's program, disassembled, with the word it's reading highlighted.
    - Examples: `hello.asm` (console), `count.asm` (number display and LEDs, once per frame), `bounce.asm` (a ball bouncing inside a border: the first step towards Pong), and `Squares.chip`, a Rom used as a lookup table.
  - Decided along the way:
    - **Every instruction takes exactly 2 ticks** (fetch, then execute), so the reference and the gate-level CPU stay in step. If 3b needs more, it changes in one place in each.
    - **Unused addresses read 0** and ignore writes: full address decoding, no mirrored copies.
    - **`RANDOM` steps every tick**, whether or not it's read, so button timing makes games feel random but a recorded run replays exactly.
    - **Programs live next to the chips** in `chips/`, so a Rom can say `file: "pong.asm"`.
    - **A Rom's words are always 16 bits**, Crumb's word size. `.hex` files work too, for machine code that didn't come from our assembler.
    - **The assembler optimizes a little:** a return merges into the instruction before it, a call just before a return becomes a jump (a tail call), and a return nothing can reach is left out.
  - Known limits: the app can show a Rom's program, but it can't run a Crumb program yet. The screen, gamepad and cartridge menu come with the console in 3c.
- **3b, the CPU in gates.** ALU16, stacks, program counter and control as `.chip` files, each with tests, all built from our own parts. *Done when:* `count.asm` runs on the gate-level Console, showing on the number display, in step with the reference. Then measure its speed.
  - **First (Claude): the canvas has to do everything the parts need.** Right now picking bits out of a bus (`a[7:0]`) or joining buses (`{hi, lo}`) only works in the Code tab, and Add16 needs both. Fix gaps like this before the user starts, and fix new ones as they turn up.
    - ✅ Done 2026-10-24: dropping a wire between pins of different widths asks which bits. A wide source gives a slice (`lo = Add8(a[7:0])`); a narrow source fills part of the pin (`sum[7:0] = lo.sum`, and once every bit is filled, `sum = {hi.sum, lo.sum}`). Deleting one wire of a joined bus leaves the others, and each piece of a join is labeled with the bits it fills (`→[15:8]`).
    - ✅ Done 2026-10-24: chips whose ports are all 1 bit work on buses (see "Chips on buses" in round 4). The copies come from the first bus wired in, a 1-bit input feeds every copy, the box reads `Xor ×16`, and inside it every signal (and every wire on the canvas) is 16 bits wide. Wiring a bus into a fresh one makes it that wide without asking which bit.
    - ✅ Done 2026-10-24, for Shift16: hold Option (Alt) while dropping a wire to pick any bits of the source and where they land in the pin, with `<< 1/2/4/8` and `>> 1/2/4/8` shortcuts. Bits you don't wire read 0, which is how the canvas makes constants for now.
    - ✅ Done 2026-10-24, for Compare16: right-click an input pin to set it to 0, 1, all 1s or any number. Option-dropping onto a pin that takes its width from what's wired (a fresh gate or 1-bit chip) offers the top bit, the top and bottom halves, and the bottom bit; onto a narrower pin, the pin-sized pieces.
    - ✅ Done 2026-10-24, for ALU16: a wire from a bus into a pin of a spread chip (`Mux4 ×16`) asks which one bit feeds every copy, and a fresh gate's pins take typed numbers like `0xFFFF` (a number doesn't fix a gate's width).
    - ✅ Done 2026-10-25, for Decode: Option-dropping onto a spread chip's pin offers "one bit for every copy" (`insn[13]`), since a same-width wire there means a different bit per copy. Also fixed in the checker: a spread chip given numbers and signals now takes its copies from the signals, so gates fed by it can work out their width.
  - **The parts, in build order.** For each, Claude writes the tests and the user builds it:
    1. `Add16`: two `Add8`s. ✅ Built by the user on 2026-10-24; all 8 tests pass.
    2. `Register16`: holds 16 bits, loads on a tick when `load` is 1, and clears on a tick when `reset` is 1. ✅ Built by the user on 2026-10-24 from two `Register8`s, loading zeros on reset through an `And ×16`; all 7 tests pass.
    3. `Shift16`: shifts by 0 to 15 places, with `left` and `right` outputs (ALU16 uses one for `lshift`, the other for `rshift`). ✅ Built by the user on 2026-10-24: two barrel-shifter chains of four Muxes; all 12 tests pass.
    4. `Compare16`: `eq`, `lt` (signed) and `ult` (unsigned) as 1-bit flags, which ALU16 widens into Forth flags. Built by subtracting with Add16. ✅ Built by the user on 2026-10-24: Add16 does `a + ~b + 1`, a tree of Ors on the halves checks for zero, and a Mux picks the signed answer; all 14 tests pass.
    5. `ALU16`: all 16 operations, with `op` picking the result. Built from units side by side (three Add16s, And/Or/Xor/Not ×16, Compare16 with its flags widened by Muxes, Shift16), then a helper `Mux4` (three Muxes, 1-bit pins, so it spreads) picks within each group of four and then the group. ✅ Built by the user on 2026-10-24, Mux4 and ALU16 both: all 32 tests pass, and `alu16_vs_reference` shows ALU16 matching the reference Crumb's ALU on 400 random inputs. (Building it pushed the Chips panel's New chip field out of view, so it now sits above the scrolling lists.)
    6. `Inc13`: the program counter's +1, wrapping from 0x1FFF to 0. One Add16 with the 13-bit number in its low bits. ✅ Built by the user on 2026-10-24, on their own from the guide's outline; all 7 tests pass.
    7. `StackPointer`: a 5-bit pointer that moves by 0, +1, −1 or −2 on a tick when `move` is 1, and clears on `reset`. Works in 8 bits (a Mux4 picks 0x00, 0x01, 0xFE or 0xFF; an Add8 adds it) and outputs the low 5, which wrap exactly like a 5-bit pointer. ✅ Built by the user on 2026-10-25; all 14 tests pass.
    8. `Decode`: splits an instruction into its fields (kind, op, flags, stack moves, address). Every kind acts like an ALU instruction, whose low 13 bits are already Crumb's control signals: a Mux4 picks the control word (jump `0x0000`, jz `0x0103`, call `0x0004`, ALU the instruction itself) and a Mux swaps in a literal's (`0x0081`); a second Mux4 and Mux give the jump, jz and call flags. ✅ Built by the user on 2026-10-25: all 29 tests pass, and `decode_vs_reference` agrees with the reference Crumb's decoder on all 65,536 instructions.
    9. `Stack` (decided 2026-10-25): one stack, used twice. A StackPointer, a 32-word `Ram`, and two flip-flops that catch what to push and whether at the end of the execute tick, so the Ram writes it during the next fetch tick. In: `data`, `write`, `delta`, `execute`, `reset`; out: `top`. Six boxes: StackPointer, a Dff for the word, a Mux (reset clears `write`) and a Dff for whether, a Mux that blocks the write during execute, and the Ram. ✅ Built by the user on 2026-10-25; all 9 tests pass.
    10. `Sequencer` (decided 2026-10-25): the phase flip-flop (fetch and execute take turns), the PC in a Register16, PC+1 from Inc13, and the next PC: a return goes to R, a jump or call to `addr`, `jz` to `addr` if T is 0 (checked with `Compare16(t, 0)`), and anything else to PC+1. Ten boxes. ✅ Built by the user on 2026-10-25; all 13 tests pass. (Its phase loop, `Not` → `Mux` → `Dff` → `Not`, has no width of its own, so the checker now settles a spread chip nothing decides at 1 bit and works the rest out from there.)
    11. `Crumb`: wires the instruction register (a Register16 loading in fetch), Decode, ALU16, the T register (a Register16 loading in execute, from the ALU or a literal's value), the Sequencer and two Stacks. The data stack pushes T when `t_to_n`; the return stack pushes T when `t_to_r`, or PC+1 on a call. The bus puts out the PC in fetch and T in execute, with N as the data to write and `store` as the write in execute. Ports: `reset` and `mem_in` in; `addr`, `mem_out`, `write`, and (to watch from outside) `execute`, `pc`, `t`, `r` out. ✅ Built by the user on 2026-10-25. Its first version passed its own tests, but the lockstep harness caught two wiring slips (the bus showed T while fetching, and ALU16 read R from the N stack); the tests now check both (21 checks). It runs hello.asm, count.asm and bounce.asm exactly like the reference: `wafer run prog.asm --gates -p chips` shows what the program did on the gates.
    12. `AddressDecoder`, `Ports`, `FrameCounter` and `Random`: the Console's sub-chips (round 5).
        - `AddressDecoder` (planned 2026-10-25): four `Match16`s, one per device. `Match16(a, mask, value)` is 1 when the bits of `a` that `mask` picks equal `value`: an And, an Xor, Compare16's tree of Ors, then a Not. The masks and values: ROM `0xE000`/`0`, RAM `0xE000`/`0x2000`, screen `0xFE00`/`0x4000`, I/O `0xFFF8`/`0x6000` (Ports reads the eighth port, 0x6007, as 0). Tests: Match16 10, AddressDecoder 15, and `address_decoder_vs_spec` checks all 65,536 addresses against the memory map. ✅ Built by the user on 2026-10-25, Match16 and AddressDecoder both: all 25 tests pass, and the decoder agrees with the memory map on every address.
        - `Ports` (planned 2026-10-25): in `addr[16]`, `io` (from AddressDecoder), `write`, `data[16]`, `reset`, `pad[8]`, `frame[16]`, `random[16]`, `switches[16]`; out `out[16]`, `leds[16]`, `number[16]`, `char[8]`, `print`. Writes: `And(io, write)`, and a lookup like Decode's (two Mux4s of constants and a Mux on `addr[2:0]`) says which register a store is for: bit 0 the LEDs (0x6003), bit 1 the number (0x6005), bit 2 the console (0x6006). Two Register16s hold the LEDs and number; `print` is the console bit, live during the writing tick, and `char` is `data[7:0]`. Reads: two Mux4s and a Mux pick the port by `addr[2:0]`, and a last Mux gives 0 when `io` is 0. Eleven boxes. The read-side Mux4s are 16 copies wide, the same as `addr`, so their selects are Option-dragged ("one bit for every copy"); a plain drag would wire `addr` bit by bit. Tests: 20 checks, and `ports_vs_reference` runs 3,000 random reads and writes against the reference Crumb's ports. ✅ Built by the user on 2026-10-25: all 20 checks pass, and it matches the reference on all 3,000 steps.
        - `FrameCounter` (planned 2026-10-25): in `reset`, out `frame[16]`. Two counters, each an Add16 (+1) feeding a Register16. The first counts the ticks of this frame; `Match16(ticks, 0xFFFF, 0x0FFF)` spots tick 4095, and an Or of that with `reset` clears it on the next tick. The second counts frames and loads only when the match is 1. Six boxes, all plain drags and pin constants. Tests: 9 checks over five frames and a mid-frame reset, and `frame_counter_vs_reference` compares it with the reference Crumb after every tick for six frames, which also ties it to `TICKS_PER_FRAME`. ✅ Built by the user on 2026-10-25: all 9 checks pass, and it matches the reference on every tick.
    13. `Console`: Crumb, the program Rom (`count.asm`), 8K of RAM, the 512-word screen, and the four sub-chips. Its outputs show the LEDs, the number display and console characters, plus Crumb's `pc`, `t` and `execute` for checking it against the reference.
  - **Also Claude:** a lockstep harness that runs the gate-level Crumb next to the reference Crumb with memory simulated in Rust, comparing after every instruction (so Crumb is checked before the Console exists); then the `--gates` checker on the Console, and the speed measurement at the end.
    - ✅ Done 2026-10-25: `wafer_core::lockstep` and `wafer run prog.asm --gates -p chips`. It compares PC and T after each instruction, N and R once the next fetch has landed the push, the number display, LEDs and console, and at the end all of RAM and the screen. A throwaway Crumb built to the plan matched the reference on hello.asm, count.asm, bounce.asm and nine instruction workouts.
    - **Speed, first measurement (2026-10-25):** the gate-level Crumb (our Nand-built gates throughout) runs about 5,000 instructions/s in a debug build and 40,000–60,000 instructions/s (80,000–120,000 ticks/s) in a release build (`cargo wafer` now builds in release). At 16,384 ticks per frame that's about 5 frames a second, so real-time games need turbo mode (3d) or fewer ticks per frame.
- **3c, the console.** Screen panel, gamepad, LEDs and switches, number display, text console, frame counter and random port in the app, plus the cartridge menu.
- **3d, debugger and turbo.** Step one instruction, breakpoints, a register, stack and memory view, and a turbo mode fast enough for games.
- **3e, games.** Pong, then Tetris, then Space Invaders.

#### Still open

- Speed: measured (about 100,000 ticks a second for Crumb in gates, release build) and the frame length set from it (round 5). How turbo works is still open, for 3d.

### Phase 4: Silicon

Verilog export, Yosys synthesis, real gate counts, 3D layer viewer, FPGA, maybe Tiny Tapeout.

## Open questions (later)

- Loop syntax for repeated parts
- Realistic per-gate delays (turbo mode is now part of Phase 3)
- Light theme toggle

## Setup status

- Mac: Apple Silicon, macOS 26.6.2. git installed.
- Rust: installed (cargo 1.99.1 in `~/.cargo/bin`).
- Git repo initialized on `main`. `cargo wafer ...` runs the CLI and `cargo app` opens the desktop app, both from the repo root.
- App stack pinned to egui/eframe 0.36 because egui_dock 0.21 hasn't caught up with egui 0.37 yet.
