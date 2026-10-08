// Parameterized adder, WIDTH defaults to 3.
module top #(parameter int WIDTH = 3) (
    input  logic [WIDTH-1:0] a,
    input  logic [WIDTH-1:0] b,
    output logic [WIDTH:0]   s
);
    assign s = a + b;
endmodule
