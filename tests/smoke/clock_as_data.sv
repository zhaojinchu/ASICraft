// Refused: the clock input read as data.
// error: the clock `clk` is also used as data by and logic (at clock_as_data.sv:5): the clock input may only clock registers; use another input for data
module top (input logic clk, input logic d, output logic q, output logic y);
    always_ff @(posedge clk) q <= d;
    assign y = clk & d;
endmodule
