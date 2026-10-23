# Wafer

A "Blender for computer chips": design digital chips from gates, simulate
them one gate delay at a time, and test them. See [PLANNING.md](PLANNING.md)
for the vision, the decisions so far, and the roadmap.

Milestone 1a, the engine and the `wafer` command, is done. The desktop app
comes next.

## Try it

You need Rust (`rustup`). From this folder:

```bash
cargo wafer -p chips test
```

```bash
cargo wafer -p chips sim FullAdder --set a=1,b=1,cin=1
```

```bash
cargo wafer -p chips render Add4 --set a=0x9,b=0x9,cin=1
```

`render` saves a PNG to `renders/` (pass `-o name.svg` for an SVG).
`cargo wafer --help` lists every command.

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

The built-in gates are Nand, And, Or, Not, Xor, Nor and Xnor. If you
write a chip with the same name as a built-in (like `chips/Xor.chip`), yours
replaces it across the project, and `wafer test` checks that the two agree.
Buses look like `in a[8]`, with `a[0]` for the lowest bit, `a[7:4]` for a
slice, and `{hi, lo}` to join buses. PLANNING.md has the full format.

## Layout

| Path | What's there |
|---|---|
| `crates/wafer-core` | The engine: parser, checker, netlist, simulator, test runner, schematic layout and SVG drawing. No UI code. |
| `crates/wafer-cli` | The `wafer` command |
| `chips/` | Our chips: gates built from Nand up to a 4-bit adder and a latch |
