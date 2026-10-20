// An 8-bit ALU slice: add, subtract, and, or by a 2-bit op.
module top (
    input  logic [7:0] a,
    input  logic [7:0] b,
    input  logic [1:0] op,
    output logic [7:0] y
);
    always_comb case (op)
        2'd0:    y = a + b;
        2'd1:    y = a - b;
        2'd2:    y = a & b;
        default: y = a | b;
    endcase
endmodule
