// Refused: a missing semicolon; the frontend's first error, without colour codes.
// error: synthesis failed: syntax_error.sv:4:18: expected ';'
module top (input logic a, output logic y);
    assign y = ~a
endmodule
