// Signed arithmetic: a 3-bit signed subtract to a 4-bit result, and a signed compare.
module top (
    input  logic signed [2:0] a,
    input  logic signed [2:0] b,
    output logic signed [3:0] d,
    output logic              neg
);
    assign d   = a - b;
    assign neg = a < b;
endmodule
