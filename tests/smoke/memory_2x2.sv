// Builds: a 2-entry memory of 2 bits becomes flip-flops and muxes.
// expect: ok
// report: cells: 31
// report: mapped netlist proven equal to the source
module top (input logic clk, input logic we, input logic waddr, input logic [1:0] wdata, input logic raddr, output logic [1:0] rdata);
    logic [1:0] mem [0:1];
    always_ff @(posedge clk) if (we) mem[waddr] <= wdata;
    assign rdata = mem[raddr];
endmodule
