// Refused: an always_comb that leaves q unassigned when en is 0, which makes a latch.
// error: `q` depends on itself through logic with no register in the loop (at latch_comb.sv:4): an always_comb that leaves `q` unassigned on some path makes a latch, which is not supported; assign it on every path (an else branch or a default), or make it a register
module top (input logic en, input logic d, output logic q);
    always_comb if (en) q = d;
endmodule
