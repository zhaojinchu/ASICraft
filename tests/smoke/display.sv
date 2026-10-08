// Refused: $display in a clocked block.
// error: $display and $write are not supported (at display.sv:6): remove them; the design becomes a circuit, which cannot print
module top (input logic clk, input logic d, output logic q);
    always_ff @(posedge clk) begin
        q <= d;
        $display("q=%b", q);
    end
endmodule
