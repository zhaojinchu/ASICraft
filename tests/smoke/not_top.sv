// Refused: the module is not named top and no --top is given.
// error: synthesis failed: no module named `top` in the source: name the module top or pass --top NAME
module inv (input logic a, output logic y);
    assign y = ~a;
endmodule
