// Builds: port bits are named by the declared index, so a[4] feeds y[3] (both least significant).
// expect: ok
// report:     a[4]             lever at
// report:     a[7]             lever at
// report:     y[3]             lamp at
// report:     y[0]             lamp at
// report: chunks when placed at the origin (/forceload add 0 0
module top (input logic [7:4] a, output logic [0:3] y);
    assign y = a;
endmodule
