// 2-bit by 2-bit unsigned multiplier.
module top (
    input  logic [1:0] a,
    input  logic [1:0] b,
    output logic [3:0] p
);
    assign p = a * b;
endmodule
