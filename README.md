# redfab

redfab turns a SystemVerilog module into a working redstone circuit for vanilla
Minecraft Java 26.3. It synthesizes the source with Yosys, proves the mapped
netlist equal to the source, places and routes it on a grid of hand-made
redstone tiles, analyses the timing of the result, and writes a structure file
you can place in your own world with one command. The output is checked against
the real game: every tile, every example design and the simulator behind the
browser playground were recorded on a vanilla 26.3 server, tick for tick.

What it is good for today, measured on 2026-10-07: small designs. An 8-bit
adder, a 4-bit counter, a 2x4-bit register file, a divider, a state machine,
a small memory. About 100 cells build in under a minute and 150 in a few
minutes; the sections below say exactly what compiles, what is refused, and
how big a design can be. A CPU is the goal and is not reachable yet: it is a
capacity question now, not a language one; see "Limits".

NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR
MICROSOFT. redfab downloads the official server jar for its own tests and never
redistributes it; the browser view draws flat colours, not the game's textures.

## Prerequisites

- macOS (arm64 or x86_64) or Linux (x86_64 or aarch64). Developed and gated on
  an arm64 Mac; the Linux paths are pinned in `versions.env` but have not been
  exercised end to end yet.
- [uv](https://docs.astral.sh/uv/) for Python. `.python-version` pins Python 3.14;
  uv installs it if the machine lacks it, and the gate asserts it.
- Node 26 (`node --version`). Synthesis runs the npm build of YoWASP Yosys
  under Node even for the native build: ABC does not run in the Python build.
- A Chromium (Brave, Chrome or Chromium) for the two headless-browser checks.
  Set `REDFAB_BROWSER` to its path if it is not in a standard location.
- Verilator 5.052, from Homebrew or apt (`tools/setup.sh` installs it; the gate
  asserts the version).
- About 6 GB of disk: Rust build output, the JDK, the server jar, Yosys.

## Setup

```sh
tools/setup.sh --accept-eula
./check
```

`tools/setup.sh` installs the pinned tools from `versions.env`: Temurin JDK 25
and the Minecraft 26.3 server jar into `.tools/` (hash-checked), Fabric Loader
for the observer mod, rustup and the Rust toolchain from `rust-toolchain.toml`,
Verilator, and the locked Python and Node packages. It links `.tools/java` to
the JDK's java binary; everything else uses that link.

Running a Minecraft server binds whoever runs it to
[Mojang's EULA](https://aka.ms/MinecraftEULA). `--accept-eula` records your
acceptance in `.tools/eula-accepted`; setup, the test harness and the live
backend refuse to start a server without that file.

`./check` is the gate: every acceptance check from every stage, about five
minutes without the game. `./check --vanilla` adds the checks that run the real
server, about an hour. `./check --only NAME` runs one check; `--clean` removes
old server run directories first. Every run writes `build/check/SUMMARY` with
the time, each check's verdict and seconds, and the verdict of the run; set
`REDFAB_CHECK_SUMMARY` to write it elsewhere (two runs at once would overwrite
each other's).

## Building a design

```sh
cargo run -q -p redfab-cli -- build designs/nor2/top.sv --top top -o build/designs/nor2
```

(`target/release/redfab` after `cargo build --release` is the same command and
about a third faster on large designs. Synthesis needs Node, `node_modules/`
and the Yosys runner `synth/yosys.mjs`: the binary looks for `synth/yosys.mjs`
beside itself and in its parent directories, so it works from `target/release`
or from a copy with a `synth/` directory next to it; `REDFAB_SYNTH=path` names
the runner outright.)

`--top` names the top module (the examples all call theirs `top`). The output
directory gets three files you care about, plus the Yosys scripts and logs:

| File | What it is |
|---|---|
| `design.json` | Every block of the circuit, where each input's lever and each output's lamp is, the settle time, the timing report, the clock (if any) |
| `world.nbt` | The same blocks as a vanilla structure file, ready for `/place template` |
| `report.txt` | Cell, tile, repeater and block counts, the footprint, the timing summary, and the checks that passed |

The build fails, with a message, if synthesis fails, if the proof cannot show
the mapped netlist equal to the source, if the layout does not implement the
netlist, or if routing does not converge.

The examples are under `designs/`, every one built, proven and compared with
Verilator by `./check`: `not1`, `nor2`, `nor3`, `consts`, `fa1`, `adder4`,
`adder8`, `mul2`, `mux4`, `prio3`, `cmp4`, `param_add`, `signed_sub`, `divmod`,
`hier`, `rom4`, and the clocked `counter4`, `fsm`, `shift2`, `lfsr3`, `negedge`,
`init_reg`, `mem2`. Together they pin the supported subset: arithmetic
(including `/`, `%` and signed values), a mux by index, `always_comb` with
`case`, parameters, hierarchy, a constant array, a memory, a synchronous
reset, a falling-edge clock and an initial value.

## What compiles, and what is refused

The synthesizable subset Yosys's slang frontend reads, mapped to NOR gates,
inverters, constants and one flip-flop, with these rules:

Compiles (every item exercised on 2026-10-07):

- Combinational logic of any shape: `assign`, `always_comb`, `case`, operators
  including `*`, `/`, `%`, comparisons, signed arithmetic (`logic signed`),
  parameters (`module top #(parameter int WIDTH = 4)`), `generate`, hierarchy
  (instantiated modules).
- One clock: a single input that feeds nothing but `always_ff` clocks, on
  `posedge` or `negedge`. Synchronous reset and enable (`if (rst) ... else if
  (en) ...` inside the clocked block). `initial` values on registers.
- Memories and constant arrays (`logic [3:0] mem [0:3]`, lookup tables) become
  flip-flops and muxes, so they are small ones only: a 2x4-bit register file
  is 57 cells and builds in 9 s; a 16x8 one would not route.
- Port ranges of any shape (`[7:4]`, `[0:3]`): levers and lamps are named by
  the declared index. Registers start at 0 unless the source gives an initial
  value, and the proof checks the initial values too.
- At most 64 input bits (a 32-bit adder with carry in has 65).

Refused before anything is placed, each with one sentence that names the
construct and its source line (`tests/smoke/*.sv` holds one source per
refusal, and the CLI test asserts the exact text):

| Source | Message |
|---|---|
| `always_ff @(posedge clk or posedge rst)` | ``register `q` has an asynchronous reset (at async_reset.sv:4): asynchronous resets are not supported; write a synchronous one: always_ff @(posedge clk) if (rst) q <= 0; else ...`` |
| An `always_comb` that leaves `q` unassigned on some path | `` `q` depends on itself through logic with no register in the loop (at latch_comb.sv:4): an always_comb that leaves `q` unassigned on some path makes a latch, which is not supported; assign it on every path (an else branch or a default), or make it a register`` |
| `always @(en) if (en) q = d;` | `` `q` is a latch (at latch_always.sv:4): latches are not supported; assign `q` on every path of its always block, or make it a register with always_ff @(posedge clk)`` |
| `inout`, `1'bz` | ``port `bus` is inout: inout and tristate ports are not supported; use separate input and output ports`` and `` `y` is driven with 'z, a tristate (at tristate.sv:5): tristate drivers are not supported; drive every signal with 0 or 1`` |
| `$display`, `$write` | `$display and $write are not supported (at display.sv:6): remove them; the design becomes a circuit, which cannot print` |
| `assert` | `assertions are not supported (at assert_immediate.sv:6): remove the assert` |
| Two `always_ff` blocks on different clocks | `registers are clocked by 2 different signals (clk1, clk2): one clock only; clock every always_ff block from the same input` |
| A derived or gated clock | ``register `q` is clocked by `half`, which is not an input (at derived_clock.sv:6): derived and gated clocks are not supported; clock every register straight from the clock input`` |
| The clock used as data (`assign y = clk & d`) | ``the clock `clk` is also used as data by and logic (at clock_as_data.sv:5): the clock input may only clock registers; use another input for data`` |
| A top module not named `--top` | ``synthesis failed: no module named `top` in the source: name the module top or pass --top NAME`` |
| A syntax error | `synthesis failed: syntax_error.sv:4:18: expected ';'` |
| 65 or more input bits | `the design has 65 input bits; at most 64 are supported (a 32-bit adder with carry in has 65): narrow the inputs or split the design` |

The playground shows the same messages (its page has a `top` field instead of
`--top`).

## Limits (measured 2026-10-31)

The compiler is correct for everything it builds; no reviewer found a wrong
circuit that passed its checks. The ceiling is capacity, not correctness:

| Limit | Value | Where it comes from |
|---|---|---|
| Builds at all | about 150 cells with up to 20 inputs, in 2 to 3 minutes natively (random netlists, two seeds each); 100 cells in about 30 s; an 8-bit adder (102 cells, 17 inputs) in about 35 s. A combinational design is laid out in two placement styles (three under 64 cells) and a clocked one in two (the four-row pitch and stretched), and once more in each packing style with the searched rows when they promise a smaller footprint, and all of these again with the sparse trailing logic levels folded into shared gate columns where that changes the columns, and the best kept; the candidates are laid out in the order they can win and a combinational one whose footprint can only lose is given up on before it is routed, so a build is one to several layouts (the four-row pitch is seldom laid out, and the levels' candidates are given up where the fold wins), each routed three ways under 48 cells (two tie-break seeds, and once charging the router for rows below the gates) and the smallest kept; the 4-bit CPU of the capacity review (132 cells, clocked) builds in about twenty minutes, since signals keep dying in its shallow rows until some are made a cell deep again and a clocked design's candidates, folded and not, are all laid out to the end. A 200-cell, 24-input netlist did not finish in 12 minutes | the router's rounds on grids of 10k tiles, then the layout check and timing on 150k-block worlds (`crates/redfab-core/src/route.rs`, `build.rs`) |
| Routes inside the playground's 90 s compile limit | about 100 cells (`measuredCells`) | `playground/compile.js` |
| Fits a plot on the live server | footprint up to 512 x 512 blocks: every shipped design fits, the widest is `adder8` at 250 x 105 | `server/live.mjs` |
| Inputs | 64 bits | `build.rs` (`MAX_INPUTS`) |
| Clock period | long: `counter4` needs 66 game ticks (3.3 s) per cycle, `mem2` 64 | repeaters on long feedback paths; the compaction steps and the two wire levels (`docs/PLAN.md`) shortened them |

Sizes of the examples and of larger designs, for scale (cells after
mapping, blocks, footprint in blocks, clock period in game ticks; every
example builds in 1 to 35 s from source with the release binary; the
register file and the CPU are the capacity review's, not shipped):

| Design | Cells | Blocks | Footprint | Period |
|---|---|---|---|---|
| `nor2` | 1 | 47 | 17 x 4 x 6 | |
| `fa1` (full adder) | 11 | 848 | 64 x 4 x 18 | |
| `mul2` | 12 | 898 | 59 x 4 x 27 | |
| `lfsr3` | 15 | 1785 | 72 x 4 x 42 | 38 |
| `counter4` | 26 | 4266 | 94 x 4 x 68 | 62 |
| `divmod` | 30 | 4127 | 117 x 4 x 51 | |
| `mem2` (2x2-bit memory) | 31 | 5492 | 102 x 4 x 77 | 52 |
| `cmp4` | 32 | 2833 | 87 x 4 x 54 | |
| `adder4` | 49 | 5431 | 125 x 4 x 63 | |
| 2x4-bit register file | 57 | 13330 | 135 x 4 x 145 | 100 |
| 8-bit adder with carry | 102 | 17609 | 250 x 4 x 105 | |
| 4-bit CPU, 16-word ROM | 132 | 53374 | 317 x 4 x 284 | 240 |

## Putting it in your world

`world.nbt` is a vanilla structure file. On a world where you have operator
rights (or cheats on, in single player):

1. Copy `world.nbt` to `<world>/generated/redfab/structure/<name>.nbt`. In
   single player the world folder is under `saves/`; on a server it is the
   `level-name` directory. Create the directories if they are missing.
2. Pick an origin `X Y Z` with empty space for the footprint `report.txt` gives
   (`blocks: ... in 250 x 4 x 250` means 250 blocks east, 4 up, 250 south of
   the origin). The structure writer omits air, so anything already inside the
   footprint stays there, and a solid block next to dust conducts: place into
   the void, or `/fill` the box with air first.
3. Keep the whole footprint loaded. Redstone only ticks in chunks within the
   simulation distance (10 chunks, 160 blocks, by default), so the far end of a
   wide design freezes when you walk to the levers. Force-load the box:
   `/forceload add X Z X+width Z+depth` (the game takes at most 256 chunks per
   command; split a bigger box).
4. `/place template redfab:<name> X Y Z none none 1.0 0 strict`. The `strict`
   flag places every block exactly as written, without block updates, which is
   what the baked state is made for. (Placed without `strict` it settles to the
   same state; the checks test both.)

Where the levers and lamps are: `report.txt` lists every input's lever and
every output's lamp as `x,y,z` from the origin (x runs east, z south, y up;
the lever stands on its probe block on the west edge, the lamps are on the
east edge), and gives the footprint's chunk count with the exact `/forceload
add` line. `design.json` has the same positions as data. `uv run --frozen
python -m harness.show build/designs/nor2` prints them in world coordinates
and starts a local server with the design already placed.

`report.txt` says how fast you may drive it:

- Combinational designs: `outputs settle 44 to 102 game ticks after an input
  change; inputs may change every 8 ticks (found over the stress walks)`. Wait
  the settle time (102 ticks is 5.1 s) before reading the lamps; flip inputs
  no closer together than the spacing, or a torch burns out (it recovers after
  8 s). The spacing is a sampled measurement: a few multi-bit changes of a
  larger design may need more.
- Clocked designs: the report states the rule in game ticks and seconds:
  `in the world: give clk a period of at least 92 ticks (4.6 s), high for 46
  (2.3 s); move the other levers 0 to 17 ticks (0.0 s to 0.8 s) after its
  rising edge, and read the lamps 91 ticks (4.5 s) after it`. The clock lever
  is one of the inputs. An input changed late in the period can be seen by
  some flip-flops and not others, and the count skips.

The checks drive the levers' probe blocks by command; a lever flipped by hand
(which powers its mounting block strongly, as the `lever_mount` recording
shows) is exercised only by `harness.show`, with time running normally.

Litematica (0.29.1 for 26.3) is documented to load vanilla `.nbt` files and
paste them with updates suppressed, which is what the baked state wants, so it
should work as is. It has not been tried: no Minecraft client has run in this
project.

## The playground

```sh
cargo build -p redfab-wasm --target wasm32-unknown-unknown --release
node tools/playground_serve.mjs
```

Open http://127.0.0.1:8765/. The page compiles the source in a worker in your
browser (YoWASP Yosys plus the core built for WebAssembly; nothing is sent
anywhere), draws the blocks, and runs them with the same simulator the game was
checked against. Click a lever in the view or in the panel. A clocked design's
clock is driven by the page at the reported period, counted in simulated
ticks. The top module is the one named `top` or the only module in the
source; the `top` field names another. See `playground/README.md`.

## The live server

```sh
tools/build_mod.sh
node server/live.mjs
```

Open http://127.0.0.1:8766/ and click **Run on the server**. The backend starts
a Fabric server with the observer mod (`mod/`), compiles your source in a
sandboxed child process, places the result in a free plot, and streams the
game's own block changes back to the page; the levers reach the probe blocks in
the world and nothing else. Everything since placement is recorded, and
`GET /api/plots/N/trace` returns a recording that `redfab check-fixtures` can
replay. See `server/README.md`, which also covers running it behind a reverse
proxy (`--host`, `--trust-proxy`, and TLS at the proxy, since the plot token
travels in the URL). The game port stays on 127.0.0.1.

## Where to read more

- `docs/PLAN.md`: the staged plan and the log of what each stage found.
- `docs/DESIGN.md`: the design review the plan came from, with what changed.
- `docs/mechanics.md`: what the game does, measured, with a fixture per fact.
- `docs/trace-format.md`: the recording format every part shares.
- `docs/REVIEW-2026-10-07.md`: the latest whole-project review and its roadmap.
- `cells/README.md`: the tile library and the rules every tile follows.

## License

MIT; see `LICENSE`. The Minecraft server jar and the Fabric libraries that
`tools/setup.sh` downloads are not part of this project and keep their own
terms.
