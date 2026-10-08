// Refused: an asynchronous reset. The flip-flop tile has no asynchronous clear.
// error: register `q` has an asynchronous reset (at async_reset.sv:4): asynchronous resets are not supported; write a synchronous one: always_ff @(posedge clk) if (rst) q <= 0; else ...
module top (input logic clk, input logic rst, input logic d, output logic q);
    always_ff @(posedge clk or posedge rst)
        if (rst) q <= 1'b0; else q <= d;
endmodule
