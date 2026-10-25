// Shows the gamepad working: the LEDs light up for the buttons you hold,
// the arrows move a dot around the screen (off one edge and back on the
// other), and the number display shows where it is, 0xXXYY.
//   In the app: open the Console, pick buttons.asm as its cartridge,
//   click the screen, and press the arrows, Z, X, Enter and Shift.

const UP    = 1
const DOWN  = 2
const LEFT  = 4
const RIGHT = 8

var x
var y
var last-frame

: pixel-addr ( x y -- addr )    // the screen word holding pixel (x, y)
  3 lshift swap 4 rshift + SCREEN + ;

: pixel-mask ( x -- mask )      // pixel x's bit in that word (bit 15 is leftmost)
  15 and 0x8000 swap rshift ;

: flip ( x y -- )               // lights a dark pixel, or darkens a lit one
  over pixel-mask >r
  pixel-addr dup @ r> xor swap ! ;

: dot ( -- )                    // draws the dot, or erases it
  x @ y @ flip ;

: held ( button -- flag )       // is that button down?
  PAD @ and 0 <> ;

: move ( -- )
  UP held    if  y @ 1- 63 and y !  then
  DOWN held  if  y @ 1+ 63 and y !  then
  LEFT held  if  x @ 1- 127 and x !  then
  RIGHT held if  x @ 1+ 127 and x !  then ;

: wait-frame ( -- )             // returns once a new frame starts
  begin FRAME @ last-frame @ <> until
  FRAME @ last-frame ! ;

: main
  64 x !  32 y !  dot
  begin
    wait-frame
    PAD @ LEDS !
    dot move dot                // erase, move, draw
    x @ 8 lshift y @ or NUMBER !
  again ;
