// 4-to-1 mux with a 2-bit select.
module top (
    input  logic [3:0] d,
    input  logic [1:0] sel,
    output logic       y
);
    assign y = d[sel];
endmodule
