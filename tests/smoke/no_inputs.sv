// Builds: a design with no inputs has lamps and constants only, and the report says so.
// expect: ok
// report: cells: 0
// report: 1 routing column)
// report: no inputs: the outputs never change
// report: chunks when placed at the origin (/forceload add 0 0
module top (output logic y, output logic z);
    assign y = 1'b1;
    assign z = 1'b0;
endmodule
