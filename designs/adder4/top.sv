// A 4-bit ripple-carry adder with carry out: 9 input bits, 5 output bits.
module top (
    input  logic [3:0] a,
    input  logic [3:0] b,
    input  logic       cin,
    output logic [3:0] sum,
    output logic       cout
);
    assign {cout, sum} = a + b + cin;
endmodule
