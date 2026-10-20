// A register file of 2 words of 4 bits: one synchronous write port, one
// combinational read port.
module top (
    input  logic       clk,
    input  logic       we,
    input  logic       waddr,
    input  logic [3:0] wdata,
    input  logic       raddr,
    output logic [3:0] rdata
);
    logic [3:0] regs [0:1];
    always_ff @(posedge clk) if (we) regs[waddr] <= wdata;
    assign rdata = regs[raddr];
endmodule
