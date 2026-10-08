// A toggle flip-flop with a declared initial value of 1 and no reset.
module top (
    input  logic clk,
    output logic q
);
    initial q = 1'b1;
    always_ff @(posedge clk) q <= ~q;
endmodule
