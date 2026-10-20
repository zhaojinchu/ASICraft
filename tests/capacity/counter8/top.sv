// An 8-bit counter with a synchronous reset and an enable.
module top (
    input  logic       clk,
    input  logic       rst,
    input  logic       en,
    output logic [7:0] count
);
    always_ff @(posedge clk)
        if (rst)     count <= 8'd0;
        else if (en) count <= count + 8'd1;
endmodule
