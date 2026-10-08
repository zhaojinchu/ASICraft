// Hierarchy: a 2-bit adder built from two instantiated full adders.
module fa (
    input  logic a, b, cin,
    output logic s, cout
);
    assign s    = a ^ b ^ cin;
    assign cout = (a & b) | (cin & (a ^ b));
endmodule

module top (
    input  logic [1:0] a,
    input  logic [1:0] b,
    output logic [2:0] s
);
    logic c;
    fa u0 (.a(a[0]), .b(b[0]), .cin(1'b0), .s(s[0]), .cout(c));
    fa u1 (.a(a[1]), .b(b[1]), .cin(c),    .s(s[1]), .cout(s[2]));
endmodule
