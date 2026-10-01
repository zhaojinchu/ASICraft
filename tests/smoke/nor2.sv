// Stage 0 smoke design: uses SystemVerilog-only syntax (logic, always_comb)
// so it only parses if the slang frontend is present.
module nor2 (
    input  logic a,
    input  logic b,
    output logic y
);
    always_comb y = ~(a | b);
endmodule
