// Builds: an initial value is kept and the proof covers it.
// expect: ok
// report: flip-flops: 1, clock arriving
// report: mapped netlist proven equal to the source
module top (input logic clk, output logic q);
    initial q = 1'b1;
    always_ff @(posedge clk) q <= ~q;
endmodule
