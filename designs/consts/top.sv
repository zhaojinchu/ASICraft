// Constant Outputs
module top (
    input  logic a,
    output logic one,
    output logic zero,
    output logic not_a
);
    assign one   = 1'b1;
    assign zero  = 1'b0;
    assign not_a = ~a;
endmodule
