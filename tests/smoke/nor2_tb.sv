module nor2_tb;
    logic a, b, y;
    nor2 dut (.a(a), .b(b), .y(y));
    initial begin
        for (int i = 0; i < 4; i++) begin
            {a, b} = i[1:0];
            #1;
            if (y !== ~(a | b)) begin
                $display("FAIL a=%b b=%b y=%b", a, b, y);
                $fatal(1);
            end
        end
        $display("PASS");
        $finish;
    end
endmodule
