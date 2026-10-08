// Builds with --top: the module is named inv and the build is told so.
// top: inv
// expect: ok
module inv (input logic a, output logic y);
    assign y = ~a;
endmodule
