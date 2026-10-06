// A 4-bit counter with a synchronous reset and an enable: the first clocked design.
module top (
    input  logic       clk,
    input  logic       rst,
    input  logic       en,
    output logic [3:0] count
);
    always_ff @(posedge clk)
        if (rst)     count <= 4'd0;
        else if (en) count <= count + 4'd1;
endmodule
