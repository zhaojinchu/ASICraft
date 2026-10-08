// Builds: a register that is never assigned starts at 0 (setundef gives every register an initial value).
// expect: ok
module top (input logic clk, output logic q);
    always_ff @(posedge clk) q <= q;
endmodule
