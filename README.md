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

## Try it

The quickest way is the playground: pick an example or paste your own module,
and compile.

To build locally on macOS or Linux, with uv and Node 26 installed:

```sh
tools/setup.sh --accept-eula
cargo run -q -p redfab-cli -- build designs/nor2/top.sv --top top -o build/designs/nor2
```

Running a Minecraft server binds whoever runs it to
[Mojang's EULA](https://aka.ms/MinecraftEULA), and `--accept-eula` records your
acceptance. The build writes `world.nbt` (the structure), `report.txt` (the
footprint, where each lever and lamp is, the timing) and `design.json`.
`./check` runs the gate in about ten minutes; two of its checks need a Chromium.

[`docs/REFERENCE.md`](docs/REFERENCE.md) has the rest: what compiles and what is
refused, the example designs, the limits, putting a design in your world, and
running the playground and the live server yourself.

NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR
MICROSOFT. redfab downloads the official server jar for its own tests and never
redistributes it; the browser view draws flat colours, not the game's textures.

## License

MIT; see `LICENSE`. The Minecraft server jar and the Fabric libraries that
`tools/setup.sh` downloads are not part of this project and keep their own
terms.
