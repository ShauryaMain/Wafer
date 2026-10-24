# Wafer

A "Blender for computer chips": design digital chips from gates, simulate
them one gate delay at a time, and test them. See [PLANNING.md](PLANNING.md)
for the vision, the decisions so far, and the roadmap.

Phases 1 and 2 are done: the engine, `wafer` command and desktop app with
its canvas, timeline, code panel and chips inside chips; then clocks,
flip-flops, memory and shared buses, up to an 8-bit counter.

Phase 3 is building **Crumb**, our 16-bit game console CPU. Its spec is
[docs/crumb.md](docs/crumb.md). So far (milestone 3a): the instruction set,
a Forth-style assembler, a fast reference Crumb that runs programs, and
`Rom`s that load assembled programs.

## The app

You need Rust (`rustup`). From this folder:

```bash
cargo app
```

It opens `chips/` (pass another folder to open that instead). The canvas
edits the `.chip` files directly, and it reloads within half a second when
anything else changes them, so you can edit text and boxes side by side.

| Key | Does |
|---|---|
| Shift+A | Add a gate, chip, input or output (type to search) |
| Drag from a pin | Draw a wire to another pin. Between pins of different widths it asks which bits, like `a[7:0]` |
| Click an input | Flip it between 0 and 1 |
| Drag / G | Move the selection (G: it follows the pointer; click to drop) |
| Shift+D | Duplicate |
| Cmd+G | Make a new chip from the selected boxes |
| Tab / double-click | Look inside the selected chip box |
| Shift+Tab | Go back out a level (or use the breadcrumbs) |
| X or Delete | Delete the selected boxes or wire |
| F | Frame everything |
| Space / → | Play-pause / step one gate delay |
| T | One clock tick (or turn on Auto in the top bar) |
| Cmd+Z / Cmd+Shift+Z | Undo / redo |
| Two-finger drag / pinch | Pan / zoom |

Rename things in the Inspector. Positions are saved in `Name.layout.json`
next to each `.chip` file.

The **Timeline** tab shows every signal over time, one gate delay per
step. Click or drag on it to scrub back in time (the canvas follows), and
double-click to return to live. The **Code** tab (next to the Inspector)
is the chip's file: edit it there and the canvas updates; select a box and
its line lights up. It suggests names as you type (Tab accepts, Ctrl+Space
asks), shows a call's inputs while you fill it in, and **Format**
(Shift+Alt+F) tidies the whole file.

## The `wafer` command

From this folder:

```bash
cargo wafer -p chips test
```

```bash
cargo wafer -p chips sim FullAdder --set a=1,b=1,cin=1
```

```bash
cargo wafer -p chips render Add4 --set a=0x9,b=0x9,cin=1
```

`render` saves a PNG to `renders/` (pass `-o name.svg` for an SVG), and
`fmt` formats every `.chip` file (`--check` only reports).
`cargo wafer --help` lists every command.

Crumb programs run on the reference CPU straight from their `.asm` file:

```bash
cargo wafer run chips/hello.asm
```

```bash
cargo wafer run chips/bounce.asm --frames 60 --screen --png renders/bounce.png
```

`run` stops when the program halts (or use `--steps` / `--frames`), then
shows the console, number display, LEDs and stack. `--pad up,a` holds
buttons down. `cargo wafer asm chips/bounce.asm --list` prints every word
with its address, instruction and source, and `-o bounce.hex` saves the
machine code.

## Writing chips

Each `.chip` file holds chips and their tests. One line is one box on the
canvas:

```
chip HalfAdder {
  in  a, b
  out sum, carry

  sum   = Xor(a, b)
  carry = And(a, b)
}

test HalfAdder {
  // a b | sum carry
     0 0 | 0   0
     1 1 | 0   1
}
```

The built-in parts are the gates Nand, And, Or, Not, Xor, Nor and Xnor,
plus `Mux(a, b, sel)`, `TriBuf(a, en)`, `Dff(d)` (flip-flops, which change
only on a clock tick), `Ram(addr, data, write)` and
`Rom(addr, file: "pong.asm")`. A Rom holds 16-bit words from a program next
to the chip: a Crumb `.asm` file (assembled when it loads, and again when it
changes) or a `.hex` file of words.

If you write a chip with the same name as a built-in (like `chips/Xor.chip`),
yours replaces it across the project, and `wafer test` checks that the two
agree.

A chip whose inputs and outputs are all 1 bit works on buses too, like a
built-in gate: with 16-bit `a` and `b`, `Xor(a, b)` is 16 of your Xors side
by side (a box labelled `Xor ×16`), and a 1-bit input feeds every copy.
Look inside it and every wire is 16 bits wide.

Several drivers can share a bus with `bus <- TriBuf(x, en)` lines. Tests can
be truth tables or steps:

```
test Counter8 {
  set reset = 1
  tick
  expect count = 0
  set reset = 0, en = 1
  tick 15
  expect count = 15
}
```

Buses look like `in a[8]`, with `a[0]` for the lowest bit, `a[7:4]` for a
slice, and `{hi, lo}` to join buses. PLANNING.md has the full format.

## Writing Crumb programs

Crumb assembly is Forth-style: words run left to right on a stack.
[docs/crumb.md](docs/crumb.md) has the whole language.

```
var paddle_y

: move_paddle  ( -- )
  PAD @ BTN_UP and if
    paddle_y @ 1 - paddle_y !
  then ;

: main
  begin move_paddle again ;
```

## Layout

| Path | What's there |
|---|---|
| `crates/wafer-core` | The engine: parser, checker, netlist, simulator, test runner, schematic layout and SVG drawing. No UI code. |
| `crates/crumb` | Crumb: its instruction encoding, the assembler, and the reference CPU |
| `crates/wafer-cli` | The `wafer` command |
| `crates/wafer-app` | The desktop app: canvas, panels, live simulation. UI tests in `tests/ui.rs` drive it with simulated input. |
| `chips/` | Our chips: gates built from Nand, adders, a latch, a register, an 8-bit counter, a shared bus, some memory and a Rom lookup table. Also Crumb programs: `hello.asm`, `count.asm`, `bounce.asm` |
| `docs/crumb.md` | The Crumb spec: memory map, instructions, timing, the assembler language |
