// A 2-bit counter clocked on the falling edge, with an enable, and a flag that
// toggles on every falling edge whatever the enable says.
module top (
    input  logic       clk,
    input  logic       en,
    output logic [1:0] q,
    output logic       phase
);
    always_ff @(negedge clk) begin
        if (en) q <= q + 2'd1;
        phase <= ~phase;
    end
endmodule
