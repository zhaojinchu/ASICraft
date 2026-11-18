// 3-Input NOR Gate
module top (
    input  logic a,
    input  logic b,
    input  logic c,
    output logic y
);
    always_comb y = ~(a | b | c);
endmodule
