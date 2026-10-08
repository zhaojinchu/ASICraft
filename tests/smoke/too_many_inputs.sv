// Refused: 65 input bits, more than the timing walks can pack into one word.
// error: the design has 65 input bits; at most 64 are supported (a 32-bit adder with carry in has 65): narrow the inputs or split the design
module top (input logic [64:0] a, output logic y);
    assign y = ^a;
endmodule
