// Pong: you against the computer. First to 9 wins.
//   In the app: open the Console, pick pong.asm as its cartridge, press
//   Play, click the screen, and use the up and down arrows. When a game is
//   over, Enter (Start) begins another.
//
// Everything that moves is drawn with Xor, so drawing it a second time
// erases it and leaves whatever was underneath. Each frame erases what
// moves, moves it, and draws it again.

const PADDLE   = 12            // a paddle is 12 pixels tall
const START    = 26            // where the paddles' top rows start: the middle
const TOP      = 1             // the highest a paddle's top row goes (row 0 is the wall)
const BOTTOM   = 63 - PADDLE   // the lowest (row 63 is the other wall)
const SPEED    = 2             // pixels your paddle moves each frame
const PLAYER   = 0x3000        // your paddle's two pixels, x = 2 and 3, in a row's first word
const COMPUTER = 0x000C        // the computer's, x = 124 and 125, in a row's last word
const NET      = 0x8000        // the net's pixel, x = 64, in a row's fifth word

const BALL-TOP    = 1          // the ball is 2×2: its top row stays between the walls
const BALL-BOTTOM = 61
const BALL-LEFT   = 4          // where it meets your paddle
const BALL-RIGHT  = 122        // and where it meets the computer's
const BALL-OUT    = 126        // past the computer's paddle: your point
const SERVE-X     = 63         // the ball is served from the middle...
const SERVE-Y     = 31
const SERVE-WAIT  = 40         // ...this many frames after a point
const WIN         = 9          // first to this many points wins
const DIGIT-Y     = 3          // the scores' top row

var player-y                   // your paddle's top row
var computer-y                 // the computer's
var ball-x                     // the ball's top-left pixel
var ball-y
var ball-dx                    // 1 or -1: a pixel across each frame
var ball-dy                    // -2 to 2: rows down (or up) each frame
var serve                      // frames until the next serve; 0 while the ball's in play
var player-score
var computer-score
var game-over                  // true once someone has won
var last-frame                 // the frame wait-frame last saw

// The court -----------------------------------------------------------------

: row ( y -- addr )            // the first screen word of row y
  3 lshift SCREEN + ;

: wall ( y -- )                // a line all the way across row y
  row 8 for dup -1 swap ! 1+ next drop ;

: net ( -- )                   // a dashed line down the middle
  60 for
    i 1+                       // rows 61, 60, ... 2
    dup 2 and if
      row 4 +  dup @ NET or  swap !
    else
      drop
    then
  next ;

: court ( -- )
  0 wall  63 wall  net ;

// The paddles ---------------------------------------------------------------
//
// A paddle that moves only changes at its ends: going down 2 rows, the two
// rows above it go dark and the two below it light up. So a move flips just
// those rows, and a paddle standing still costs nothing.

: player-rows ( from n -- )    // flips n rows of your paddle's pixels, from row `from` down
  for  dup row  dup @ PLAYER xor swap !  1+  next  drop ;

: computer-rows ( from n -- )  // the same for the computer's, in each row's last word
  for  dup row 7 +  dup @ COMPUTER xor swap !  1+  next  drop ;

: held ( button -- flag )      // is that button down?
  PAD @ and 0 <> ;

: on-court ( top -- top )      // keeps a paddle between the walls
  dup TOP < if drop TOP then
  dup BOTTOM > if drop BOTTOM then ;

: changes ( old new -- low n ) // a paddle moving from top row `old` to `new`: the first
  2dup > if swap then          // row that changes at each end, and how many change
  over - ;

: move-player ( -- )
  player-y @  dup
  BTN_UP held if SPEED - then
  BTN_DOWN held if SPEED + then
  on-court  dup player-y !     ( old new )
  changes dup if
    2dup player-rows  swap PADDLE + swap player-rows
  else
    2drop
  then ;

: playing ( -- flag )          // is the ball in play?
  serve @ 0=  game-over @ 0=  and ;

: move-computer ( -- )         // a pixel a frame: after the ball when it's coming,
  ball-dx @ 0 >  playing and   // back to the middle when it isn't
  if  ball-y @ 5 -  else  START  then
  computer-y @  tuck           ( old target old )
  2dup < if  1-  else  2dup > if 1+ then  then
  nip on-court  dup computer-y !
  changes dup if
    2dup computer-rows  swap PADDLE + swap computer-rows
  else
    2drop
  then ;

// The ball ------------------------------------------------------------------

: pixel-addr ( x y -- addr )   // the screen word holding pixel (x, y)
  row swap 4 rshift + ;

: pixel-mask ( x -- mask )     // pixel x's bit in that word (bit 15 is leftmost)
  15 and 0x8000 swap rshift ;

: flip ( x y -- )              // lights a dark pixel, or darkens a lit one
  over pixel-mask >r
  pixel-addr dup @ r> xor swap ! ;

: flip-ball ( -- )             // draws the 2×2 ball, or erases it
  ball-x @ ball-y @ flip
  ball-x @ 1+ ball-y @ flip
  ball-x @ ball-y @ 1+ flip
  ball-x @ 1+ ball-y @ 1+ flip ;

: level ( top -- offset )      // the ball's lower row, counted from a paddle's top row
  ball-y @ 1+ swap - ;

: hits ( top -- flag )         // is the ball level with a paddle there?
  level  dup 0 < invert  swap 13 <  and ;

: aim ( top -- dy )            // where it meets the paddle sets its slope:
  level                        // steep off the ends, straight off the middle
  dup 3 < if drop -2 exit then
  dup 5 < if drop -1 exit then
  dup 8 < if drop 0 exit then
  10 < if 1 exit then
  2 ;

: bounce ( top dx -- )         // sends the ball back off a paddle at that top row
  ball-dx !  aim ball-dy ! ;

: move-ball ( -- )
  // Across, meeting a paddle on the way.
  ball-x @ ball-dx @ + ball-x !
  ball-x @ BALL-LEFT =  ball-dx @ 0 <  and if
    player-y @ hits if  player-y @ 1 bounce  then
  then
  ball-x @ BALL-RIGHT =  ball-dx @ 0 >  and if
    computer-y @ hits if  computer-y @ -1 bounce  then
  then
  // Down (or up), turning at the walls.
  ball-y @ ball-dy @ +
  dup BALL-TOP 1+ < if  drop BALL-TOP  ball-dy @ negate ball-dy !  then
  dup BALL-BOTTOM 1- > if  drop BALL-BOTTOM  ball-dy @ negate ball-dy !  then
  ball-y ! ;

: serve-ball ( -- )            // into play from the middle, at a slant either way
  SERVE-X ball-x !  SERVE-Y ball-y !
  RANDOM @ 2 and 1- ball-dy !
  flip-ball ;

// The scores ----------------------------------------------------------------

table digits                   // 3×5 digits, a row a word, the leftmost pixel in bit 2
  0b111 0b101 0b101 0b101 0b111    // 0
  0b010 0b110 0b010 0b010 0b111    // 1
  0b111 0b001 0b111 0b100 0b111    // 2
  0b111 0b001 0b011 0b001 0b111    // 3
  0b101 0b101 0b111 0b001 0b001    // 4
  0b111 0b100 0b111 0b001 0b111    // 5
  0b111 0b100 0b111 0b101 0b111    // 6
  0b111 0b001 0b010 0b010 0b010    // 7
  0b111 0b101 0b111 0b101 0b111    // 8
  0b111 0b101 0b111 0b001 0b111    // 9
end

: digit-row ( n r -- pattern ) // row r of digit n
  swap dup 2 lshift + + digits + @ ;

: flip-player-score ( -- )     // your score at x = 56 to 58: bits 7 to 5 of word 3
  5 for
    player-score @ i 1- digit-row 5 lshift
    DIGIT-Y i 1- + row 3 +
    dup @ rot xor swap !
  next ;

: flip-computer-score ( -- )   // the computer's at x = 70 to 72: bits 9 to 7 of word 4
  5 for
    computer-score @ i 1- digit-row 7 lshift
    DIGIT-Y i 1- + row 4 +
    dup @ rot xor swap !
  next ;

: show-scores ( -- )           // on the number display too: yours, then the computer's
  player-score @ 8 lshift  computer-score @ or  NUMBER ! ;

: point ( -- )                 // after a point: wait to serve, unless someone's won
  SERVE-WAIT serve !
  show-scores
  player-score @ WIN =  computer-score @ WIN =  or if
    true game-over !
    player-score @ WIN = LEDS !     // every LED on if you won
  then ;

: your-point ( -- )
  flip-player-score  1 player-score +!  flip-player-score
  1 ball-dx !  point ;         // the next serve goes to the computer, who lost it

: computers-point ( -- )
  flip-computer-score  1 computer-score +!  flip-computer-score
  -1 ball-dx !  point ;        // and this one to you

: new-game ( -- )
  flip-player-score  flip-computer-score
  0 player-score !  0 computer-score !
  flip-player-score  flip-computer-score
  false game-over !  0 LEDS !  show-scores
  1 ball-dx !  SERVE-WAIT serve ! ;

// The game ------------------------------------------------------------------

: wait-frame ( -- )            // returns once a new frame starts
  begin FRAME @ last-frame @ <> until
  FRAME @ last-frame ! ;

: play-ball ( -- )             // the ball's part of a frame
  game-over @ if
    BTN_START held if new-game then
    exit
  then
  serve @ if
    -1 serve +!
    serve @ 0= if serve-ball then
    exit
  then
  flip-ball move-ball
  ball-x @ 0 = if computers-point exit then
  ball-x @ BALL-OUT = if your-point exit then
  flip-ball ;

: main
  START player-y !  START computer-y !
  1 ball-dx !  SERVE-WAIT serve !
  court  START PADDLE player-rows  START PADDLE computer-rows
  flip-player-score  flip-computer-score  show-scores
  begin
    wait-frame
    move-player  move-computer  play-ball
  again ;
