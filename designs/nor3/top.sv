// A three-input NOR, which uses all three input sides of the gate tile.
module top (
    input  logic a,
    input  logic b,
    input  logic c,
    output logic y
);
    always_comb y = ~(a | b | c);
endmodule
