// Refused: an inout port driven with 'z.
// error: port `bus` is inout: inout and tristate ports are not supported; use separate input and output ports
// error: `y` is driven with 'z, a tristate (at tristate.sv:5): tristate drivers are not supported; drive every signal with 0 or 1
module top (input logic oe, input logic d, inout wire bus, output logic y);
    assign bus = oe ? d : 1'bz;
    assign y = bus;
endmodule
