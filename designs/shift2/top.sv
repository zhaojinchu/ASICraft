// A two-stage shift register: the shortest possible path between two
// flip-flops, which is where a hold-time violation shows first.
module top (
    input  logic clk,
    input  logic d,
    output logic q1,
    output logic q2
);
    always_ff @(posedge clk) begin
        q1 <= d;
        q2 <= q1;
    end
endmodule
