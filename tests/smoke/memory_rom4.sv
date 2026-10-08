// Builds: a 4-entry constant array (a lookup table) becomes logic.
// expect: ok
// report: mapped netlist proven equal to the source
module top (input logic [1:0] addr, output logic [3:0] data);
    logic [3:0] rom [0:3] = '{4'h3, 4'ha, 4'h5, 4'hc};
    assign data = rom[addr];
endmodule
