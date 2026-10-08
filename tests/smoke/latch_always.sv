// Refused: an always_latch.
// error: `q` is a latch (at latch_always.sv:4): latches are not supported; assign `q` on every path of its always block, or make it a register with always_ff @(posedge clk)
module top (input logic en, input logic d, output logic q);
    always_latch if (en) q <= d;
endmodule
