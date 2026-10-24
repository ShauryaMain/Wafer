// Prints a greeting on the text console, then stops.
//   cargo wafer run chips/hello.asm

table greeting "Hello, Crumb!\n" 0 end

: emit ( char -- )  CONSOLE ! ;

: print ( addr -- )         // prints characters until it reaches a 0
  begin dup @ dup while
    emit 1+
  repeat
  2drop ;

: main
  greeting print
  halt ;
