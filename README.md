# redfab

redfab compiles a SystemVerilog module into a redstone circuit for vanilla
Minecraft Java 26.3 and writes it as a structure file you place in a world with
`/place template`. The compiler is written in Rust and uses Yosys for synthesis.
The playground at https://redstonefab.zhaojinchu.com/ runs the same compiler in
the browser, and its Run in the game button places the circuit on a live server.

## How it works

Each build:

1. synthesises the source to NOR gates, inverters, constants and one flip-flop
2. proves the mapped netlist equal to the source
3. places and routes it on a grid of hand-made redstone tiles
4. reads the circuit back from the blocks and checks it against the netlist
5. works out its timing in game ticks

The block simulator behind the playground replays all 826 recordings in
`fixtures/`, made on a vanilla 26.3 server, tick for tick, and `./check --vanilla`
runs every tile and 18 of the 23 example designs in the game. The examples
include an 8-bit adder (102 cells, built in under 35 s), and a 4-bit CPU in
`tests/capacity/cpu4` builds in about 20 minutes.

