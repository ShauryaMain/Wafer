// Counts up on the number display, one step per frame, and shows the
// count on the LEDs too.
//   cargo wafer run chips/count.asm --frames 10

var count
var last-frame

: wait-frame ( -- )         // returns once a new frame starts
  begin FRAME @ last-frame @ <> until
  FRAME @ last-frame ! ;

: main
  begin
    wait-frame
    count @ 1+ dup count !
    dup NUMBER ! LEDS !
  again ;
