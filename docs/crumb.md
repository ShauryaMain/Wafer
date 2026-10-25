# Crumb

Crumb is the CPU of our tiny game console: a 16-bit stack machine in the style of the [J1 Forth CPU](https://excamera.com/sphinx/fpga-j1.html). This page is the contract between its three implementations:

- the **reference Crumb** in Rust (`crates/crumb`), which runs programs at full speed,
- the **gate-level Crumb** built in Wafer (milestone 3b), which is checked against the reference instruction by instruction,
- the **assembler** (`wafer asm`), which turns Forth-style source into Crumb machine code.

If they disagree, this page decides who's right (or we fix this page).

## The machine

| Part | Size | What it is |
|---|---|---|
| `PC` | 13 bits | Address of the next instruction |
| `T` | 16 bits | Top of the data stack |
| `N` | 16 bits | Second on the data stack: `dstack[dsp]` |
| data stack | 32 words + `dsp` (5 bits) | Everything under `T` |
| `R` | 16 bits | Top of the return stack: `rstack[rsp]` |
| return stack | 32 words + `rsp` (5 bits) | Return addresses, and values parked with `>r` |

- **Words, not bytes.** Every address holds one 16-bit word.
- **Stacks wrap around.** Pushing a 33rd value overwrites the oldest. Nothing checks for overflow.
- **Reset:** `PC`, `T`, `dsp` and `rsp` are 0, and both stacks are full of zeros. Execution starts at address 0.

## Memory map

The top three address bits pick the device.

| Addresses | Device | Reads | Writes |
|---|---|---|---|
| `0x0000–0x1FFF` | Program ROM, 8K words | The program | Ignored |
| `0x2000–0x3FFF` | RAM, 8K words | The word | Stored |
| `0x4000–0x41FF` | Screen, 512 words | The word | Stored |
| `0x6000` | `PAD` gamepad | Buttons held (below) | Ignored |
| `0x6001` | `FRAME` | Frames since reset | Ignored |
| `0x6002` | `RANDOM` | A pseudo-random number | Ignored |
| `0x6003` | `LEDS` | The last value written | Shown on 16 LEDs |
| `0x6004` | `SWITCHES` | 16 switches | Ignored |
| `0x6005` | `NUMBER` | The last value written | Shown as 4 hex digits |
| `0x6006` | `CONSOLE` | 0 | The low 8 bits are printed as a character (10 is a new line) |
| anything else | nothing | 0 | Ignored |

RAM and the screen start cleared to 0.

### Screen

128×64 pixels, 1 bit each (1 = lit). Each row is 8 words, so row `y` starts at `0x4000 + 8*y`. Word `w` of a row covers pixels `x = 16*w` to `16*w + 15`, and **bit 15 is the leftmost pixel**, so a word written in binary looks like the pixels it draws.

### Gamepad

| Bit | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
|---|---|---|---|---|---|---|---|---|
| Button | Up | Down | Left | Right | A | B | Start | Select |
| Key | ↑ | ↓ | ← | → | Z | X | Enter | Shift |

A bit is 1 while its button is held. Bits 8–15 read 0.

### Frames and randomness

- **`FRAME`** reads `ticks / TICKS_PER_FRAME` (wrapping at 16 bits), where `ticks` counts clock ticks since reset. `TICKS_PER_FRAME` is **4096**, so every frame gets 2048 instructions. (We picked it after measuring the gate-level Crumb at about 100,000 ticks a second, so games run at a watchable speed even in gates.) A game waits for the next frame by reading `FRAME` until it changes.
- **`RANDOM`** reads a 16-bit Galois LFSR (taps `0xB400`) that starts at `0xACE1` and steps once **every clock tick**, whether or not anyone reads it. Since players press buttons at unpredictable ticks, games still feel random, but a recorded run replays exactly.

## Timing

**Every instruction takes exactly 2 clock ticks:** a fetch tick that reads the instruction at `PC`, then an execute tick. Instruction `k` (counting from 0) runs in ticks `2k` and `2k+1`. Anything it reads from `FRAME` or `RANDOM` is the value during its execute tick, after `2k+1` ticks have completed.

This fixed count is what keeps the reference and the gate-level CPU in step. If 3b needs more ticks per instruction, this number changes in one place in each.

## Instructions

Every instruction is one 16-bit word. The top bits pick the kind:

| Bits | Kind | What it does |
|---|---|---|
| `1vvv vvvv vvvv vvvv` | literal | Push `v` (0 to 0x7FFF) |
| `000a aaaa aaaa aaaa` | jump | `PC = a` |
| `001a aaaa aaaa aaaa` | jump if zero | Pop `T`; if it was 0, `PC = a` |
| `010a aaaa aaaa aaaa` | call | Push `PC + 1` on the return stack; `PC = a` |
| `011` + 13 bits | ALU | Everything else (below) |

Numbers from 0x8000 up don't fit in a literal, so the assembler pushes the inverted value and then flips it with `invert`.

### ALU instructions

```
 15 14 13 | 12   | 11 10 9 8 | 7    | 6    | 5     | 4 | 3 2 | 1 0
  0  1  1 | R→PC | op        | T→N  | T→R  | N→[T] | 0 | rd  | dd
```

All of these happen at once, using the values from before the instruction:

1. `T` becomes the result of `op` (table below).
2. `dsp` moves by `dd` and `rsp` moves by `rd`: `00` is 0, `01` is +1, `11` is −1 and `10` is −2. Bit 4 is unused and ignored.
3. **`T→N`**: the old `T` is written into the new `N` slot, `dstack[dsp + dd]`.
4. **`T→R`**: the old `T` is written into the new `R` slot, `rstack[rsp + rd]`.
5. **`N→[T]`**: `N` is stored at memory address `T`.
6. **`R→PC`**: `PC` becomes the low 13 bits of `R` (a return). Otherwise `PC` moves on by one.

| `op` | Result | `op` | Result |
|---|---|---|---|
| 0 | `T` | 8 | `N = T`: 0xFFFF if equal, else 0 |
| 1 | `N` | 9 | `N < T`, signed: 0xFFFF or 0 |
| 2 | `T + N` | 10 | `N < T`, unsigned: 0xFFFF or 0 |
| 3 | `N − T` | 11 | `N << (T & 15)` |
| 4 | `T and N` | 12 | `N >> (T & 15)`, filling with zeros |
| 5 | `T or N` | 13 | Memory at address `T` |
| 6 | `T xor N` | 14 | `R` |
| 7 | `not T` | 15 | `T − 1` |

True is 0xFFFF (all ones) and false is 0, as in Forth, so `and`, `or` and `invert` work as logic on flags too.

## The assembler

Crumb assembly is Forth-style: words separated by spaces, running left to right on the stack. A `.asm` file assembles into a ROM image. A `Rom` in a chip loads one with `rom = Rom(addr, file: "pong.asm")`, and `wafer asm` / `wafer run` assemble and run one from the terminal.

```
const PAD = 0x6000
const UP  = 0b0001
var paddle_y

: move_paddle  ( -- )
  PAD @ UP and if
    paddle_y @ 1 - paddle_y !
  then ;

: main
  begin move_paddle again ;
```

### Words that run on the stack

The comments show the stack before and after `--`, top on the right.

| Word | Stack | Instructions |
|---|---|---|
| `dup` | `a -- a a` | 1 |
| `drop` | `a --` | 1 |
| `swap` | `a b -- b a` | 1 |
| `over` | `a b -- a b a` | 1 |
| `nip` | `a b -- b` | 1 |
| `+` `-` `and` `or` `xor` | `a b -- a+b` (and so on) | 1 |
| `invert` | `a -- not a` | 1 |
| `=` `<` `u<` | `a b -- flag` | 1 |
| `lshift` `rshift` | `a n -- a<<n` / `a>>n` | 1 |
| `1-` | `a -- a-1` | 1 |
| `@` | `addr -- value` | 1 |
| `!` | `value addr --` | 2 |
| `>r` `r>` `r@` `rdrop` | move values to and from the return stack | 1 |
| `dup>r` | copies `T` onto the return stack | 1 |
| `exit` | return early from a word | 1 |
| `noop` | does nothing | 1 |
| `1+` | `a -- a+1` | 2 |
| `0=` | `a -- flag` | 2 |
| `>` `u>` | `a b -- flag` | 2 |
| `<>` | `a b -- flag` (not equal) | 2 |
| `negate` | `a -- -a` | 3 |
| `2dup` `2drop` `tuck` | as in Forth | 2 |
| `rot` | `a b c -- b c a` | 4 |
| `+!` | `n addr --` adds `n` to the word at `addr` | 6 |
| `halt` | stops: jumps to itself forever | 1 |

### Numbers

`42`, `-1`, `0x2A`, `0b101010`, and characters like `'A'` or `'\n'`. Underscores are allowed between digits (`0b1111_0000`). Values from −32768 to 65535 fit; negative numbers are stored as 16-bit two's complement.

### Definitions

| Syntax | Meaning |
|---|---|
| `: name ... ;` | Defines a word. Using `name` calls it. Words can call words defined later, and themselves |
| `const NAME = value` | A named number. The value can use `+`, `-` and `*` with numbers and earlier constants, like `const ROW = 8 * 2` |
| `var name` / `var name[10]` | Reserves 1 (or 10) words of RAM, starting at `0x2000`. Using `name` pushes its address |
| `table name ... end` | Puts numbers and `"strings"` (one character per word) into ROM. Using `name` pushes its address |
| `// ...` and `( ... )` | Comments. `( a b -- c )` is the usual place for a stack comment |

Every program needs a `main` word: address 0 holds a jump to it. Code has to be inside a definition.

### Control flow

| Syntax | Meaning |
|---|---|
| `flag if ... then` | Runs the middle when `flag` isn't 0 (it's popped either way) |
| `flag if ... else ... then` | One or the other |
| `begin ... flag until` | Repeats until `flag` isn't 0 |
| `begin ... again` | Repeats forever |
| `begin ... flag while ... repeat` | Repeats while `flag` isn't 0 |
| `n for ... next` | Runs the middle `n` times (`n` must be at least 1). Inside, `i` is how many runs are left, counting this one: `n`, then `n − 1`, ..., 1. Loops can nest; `i` is always the innermost one |

### Built-in constants

`SCREEN`, `PAD`, `FRAME`, `RANDOM`, `LEDS`, `SWITCHES`, `NUMBER`, `CONSOLE` (the addresses above), `BTN_UP`, `BTN_DOWN`, `BTN_LEFT`, `BTN_RIGHT`, `BTN_A`, `BTN_B`, `BTN_START`, `BTN_SELECT` (the gamepad bits), `true` (0xFFFF) and `false` (0). A `const` with the same name replaces a built-in one.

### What the assembler does for you

- **Returns merge into the instruction before them.** `+ ;` is one instruction: the add and the return share it.
- **Tail calls become jumps.** When a word ends by calling another, it jumps there instead, which saves a return and a return-stack slot.
- **Returns nothing can reach are left out**, like the `;` after `halt` or `again`.
- **Big numbers** (0x8000 and up, or negative) become a literal plus `invert`.
