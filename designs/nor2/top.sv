// Stage 3's end-to-end slice: one NOR gate.
module top (
    input  logic a,
    input  logic b,
    output logic y
);
    assign y = ~(a | b);
endmodule
