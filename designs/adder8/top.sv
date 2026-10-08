// An 8-bit ripple-carry adder with carry in and carry out: 17 input bits, 9 output
// bits, 102 cells. The capacity target: it needs more routing columns in its input
// gap than the router once allowed, and it is built by the gate so that the router
// cannot shrink back to designs of adder4's size without a check failing.
module top (
    input  logic [7:0] a,
    input  logic [7:0] b,
    input  logic       cin,
    output logic [7:0] sum,
    output logic       cout
);
    assign {cout, sum} = a + b + cin;
endmodule
