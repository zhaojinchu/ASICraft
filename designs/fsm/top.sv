// A small state machine: `hit` is high while the last three values of `x`
// seen at clock edges, followed by the current one, read 1, 0, 1.
module top (
    input  logic clk,
    input  logic rst,
    input  logic x,
    output logic hit
);
    typedef enum logic [1:0] {IDLE, SEEN1, SEEN10} state_t;
    state_t state;

    always_ff @(posedge clk)
        if (rst) state <= IDLE;
        else case (state)
            IDLE:    state <= x ? SEEN1 : IDLE;
            SEEN1:   state <= x ? SEEN1 : SEEN10;
            SEEN10:  state <= x ? SEEN1 : IDLE;
            default: state <= IDLE;
        endcase

    assign hit = (state == SEEN10) && x;
endmodule
