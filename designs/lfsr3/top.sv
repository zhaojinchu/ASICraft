// A 3-bit Fibonacci LFSR (x^3 + x^2 + 1), seeded to 3'b001 by reset.
module top (
    input  logic       clk,
    input  logic       rst,
    output logic [2:0] q
);
    always_ff @(posedge clk)
        if (rst) q <= 3'b001;
        else     q <= {q[1:0], q[2] ^ q[1]};
endmodule
