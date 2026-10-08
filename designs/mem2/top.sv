// A 2-entry memory of 2 bits with a synchronous write and a combinational read:
// the memory pass turns it into flip-flops and muxes.
module top (
    input  logic       clk,
    input  logic       we,
    input  logic       waddr,
    input  logic [1:0] wdata,
    input  logic       raddr,
    output logic [1:0] rdata
);
    logic [1:0] mem [0:1];
    always_ff @(posedge clk) if (we) mem[waddr] <= wdata;
    assign rdata = mem[raddr];
endmodule
