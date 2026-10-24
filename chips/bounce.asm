// A ball bouncing around inside a border: the first step towards Pong.
//   cargo wafer run chips/bounce.asm --frames 60 --png renders/bounce.png

const BALL   = 4            // the ball is 4×4 pixels
const LEFT   = 1            // the ball's top-left corner stays inside the border
const TOP    = 1
const RIGHT  = 127 - BALL
const BOTTOM = 63 - BALL

var x
var y
var dx                      // speed: 1 or -1 pixels per frame
var dy
var last-frame

// Drawing ---------------------------------------------------------------

: pixel-addr ( x y -- addr )    // the screen word holding pixel (x, y)
  3 lshift swap 4 rshift + SCREEN + ;

: pixel-mask ( x -- mask )      // pixel x's bit in that word (bit 15 is leftmost)
  15 and 0x8000 swap rshift ;

: flip ( x y -- )               // lights a dark pixel, or darkens a lit one
  over pixel-mask >r
  pixel-addr dup @ r> xor swap ! ;

: flip-row ( y -- )             // flips one row of the ball
  BALL for
    x @ i 1- +  over  flip
  next drop ;

: flip-ball ( -- )              // draws the ball, or erases it if it's drawn
  BALL for
    y @ i 1- + flip-row
  next ;

: hline ( row -- )              // a line across the whole screen
  3 lshift SCREEN +
  8 for  dup -1 swap !  1+  next
  drop ;

: border ( -- )
  64 for  0 i 1- flip  127 i 1- flip  next
  0 hline  63 hline ;

// Moving ----------------------------------------------------------------

: step-x ( -- )                 // moves, and turns around at the walls
  x @ dx @ + dup x !
  dup LEFT = swap RIGHT = or if  dx @ negate dx !  then ;

: step-y ( -- )
  y @ dy @ + dup y !
  dup TOP = swap BOTTOM = or if  dy @ negate dy !  then ;

: wait-frame ( -- )             // returns once a new frame starts
  begin FRAME @ last-frame @ <> until
  FRAME @ last-frame ! ;

: main
  10 x !  20 y !  1 dx !  1 dy !
  border
  flip-ball
  begin
    wait-frame
    flip-ball                   // erase it
    step-x step-y
    flip-ball                   // and draw it in its new spot
  again ;
