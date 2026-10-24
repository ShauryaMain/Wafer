// The squares of 0 to 15, for Squares.chip to look up. The table starts at
// address 1: address 0 always holds the jump to `main`.

table squares
  0 1 4 9 16 25 36 49 64 81 100 121 144 169 196 225
end

: main halt ;
