// Refused: a register clocked by another register's output (a clock divider).
// error: register `q` is clocked by `half`, which is not an input (at derived_clock.sv:6): derived and gated clocks are not supported; clock every register straight from the clock input
module top (input logic clk, output logic q);
    logic half;
    always_ff @(posedge clk) half <= ~half;
    always_ff @(posedge half) q <= ~q;
endmodule
