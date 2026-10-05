// The smallest design: one inverter.
module top (
    input  logic a,
    output logic y
);
    assign y = ~a;
endmodule
