// Refused: two clock inputs.
// error: registers are clocked by 2 different signals (clk1, clk2): one clock only; clock every always_ff block from the same input
module top (input logic clk1, input logic clk2, input logic d, output logic q1, output logic q2);
    always_ff @(posedge clk1) q1 <= d;
    always_ff @(posedge clk2) q2 <= d;
endmodule
