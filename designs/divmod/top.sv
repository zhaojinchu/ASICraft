// Division and modulo of a 3-bit value by a 2-bit value; dividing by zero is defined
// here (quotient all ones, remainder the dividend) since the standard leaves it x
// and Verilator and Yosys would otherwise disagree.
module top (
    input  logic [2:0] a,
    input  logic [1:0] b,
    output logic [2:0] q,
    output logic [1:0] r
);
    assign q = (b == 2'd0) ? 3'b111 : a / b;
    assign r = (b == 2'd0) ? a[1:0] : 2'(a % b);
endmodule
