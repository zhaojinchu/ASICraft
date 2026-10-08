// Refused: an immediate assertion.
// error: assertions are not supported (at assert_immediate.sv:6): remove the assert
module top (input logic clk, input logic d, output logic q);
    always_ff @(posedge clk) begin
        q <= d;
        assert (d == q);
    end
endmodule
