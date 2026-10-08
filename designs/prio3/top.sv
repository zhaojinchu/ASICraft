// 3-bit priority encoder: index of the highest set request, and a valid flag.
module top (
    input  logic [2:0] req,
    output logic [1:0] idx,
    output logic       valid
);
    always_comb begin
        valid = 1'b1;
        case (1'b1)
            req[2]:  idx = 2'd2;
            req[1]:  idx = 2'd1;
            req[0]:  idx = 2'd0;
            default: begin idx = 2'd0; valid = 1'b0; end
        endcase
    end
endmodule
