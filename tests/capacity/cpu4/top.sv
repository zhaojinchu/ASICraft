// The smallest plausible CPU: a 4-bit accumulator, a 4-bit program counter and
// a 16-word program ROM of 8-bit instructions, {op[1:0], arg[3:0], 2 unused}.
//   00 LDI  imm   acc <= imm
//   01 ADD  imm   acc <= acc + imm
//   10 OUT        out <= acc
//   11 JMP  addr  pc  <= addr
// One clock, a synchronous reset; the accumulator shows on lamps.
module top (
    input  logic       clk,
    input  logic       rst,
    output logic [3:0] acc_out,
    output logic [3:0] out
);
    logic [3:0] pc;
    logic [3:0] acc;
    logic [7:0] instr;

    // The program: count up by 3 from 1, showing each value, forever.
    always_comb case (pc)
        4'd0:    instr = 8'b00_0001_00; // LDI 1
        4'd1:    instr = 8'b10_0000_00; // OUT
        4'd2:    instr = 8'b01_0011_00; // ADD 3
        4'd3:    instr = 8'b10_0000_00; // OUT
        4'd4:    instr = 8'b01_0011_00; // ADD 3
        4'd5:    instr = 8'b10_0000_00; // OUT
        4'd6:    instr = 8'b01_0101_00; // ADD 5
        4'd7:    instr = 8'b11_0001_00; // JMP 1
        default: instr = 8'b11_0000_00; // JMP 0
    endcase

    logic [1:0] opcode;
    logic [3:0] arg;
    assign opcode = instr[7:6];
    assign arg    = instr[5:2];

    always_ff @(posedge clk)
        if (rst) begin
            pc  <= 4'd0;
            acc <= 4'd0;
            out <= 4'd0;
        end else begin
            pc <= (opcode == 2'd3) ? arg : pc + 4'd1;
            case (opcode)
                2'd0: acc <= arg;
                2'd1: acc <= acc + arg;
                2'd2: out <= acc;
                default: ;
            endcase
        end

    assign acc_out = acc;
endmodule
