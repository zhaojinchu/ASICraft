# redfab reference

Setup, building a design, what compiles, the limits, putting a design in your
world, and running the playground and the live server. `README.md` is the short
version.

## Prerequisites

- macOS or Linux. `versions.env` pins the JDK for arm64 and x86_64 Macs and for
  x86_64 and aarch64 Linux. Development and the full `./check` runs were on an
  arm64 Mac.
- [uv](https://docs.astral.sh/uv/) for Python. `.python-version` pins Python 3.14;
  uv installs it if it is missing, and the gate checks it.
- Node 26 (`node --version`). Synthesis runs the npm build of YoWASP Yosys under
  Node, also for the native build, because ABC does not run in the Python build.
- A Chromium (Brave, Chrome or Chromium) for the two headless-browser checks.
  Set `REDFAB_BROWSER` to its path if it is not in a standard place.
- Verilator 5.052, from Homebrew or apt. `tools/setup.sh` installs it and the
  gate checks the version.
- About 6 GB of disk for the Rust build output, the JDK, the server jar and Yosys.

## Setup

```sh
tools/setup.sh --accept-eula
./check
```

`tools/setup.sh` installs the pinned tools from `versions.env`: Temurin JDK 25
and the Minecraft 26.3 server jar into `.tools/` (hash-checked), Fabric Loader
for the observer mod, rustup and the Rust toolchain from `rust-toolchain.toml`,
Verilator, and the locked Python and Node packages. It links `.tools/java` to
the JDK's java binary, and everything else uses that link.

Running a Minecraft server binds whoever runs it to
[Mojang's EULA](https://aka.ms/MinecraftEULA). `--accept-eula` records your
acceptance in `.tools/eula-accepted`; setup, the test harness and the live
backend will not start a server without that file.

`./check` is the gate. It runs every check that does not need the game in about
ten minutes. `./check --vanilla` adds the checks that run the real server and a
100-gate random netlist, about an hour and a quarter. `./check --only NAME` runs
one check, and `--clean` removes old server run directories first. Every run
writes `build/check/SUMMARY` with the time, each check's verdict and seconds,
and the verdict of the run; `REDFAB_CHECK_SUMMARY` names another file, so two
runs at once do not overwrite each other's.

## Building a design

```sh
cargo run -q -p redfab-cli -- build designs/nor2/top.sv --top top -o build/designs/nor2
```

`--top` names the top module (the examples all call theirs `top`). The output
directory gets three files, plus the Yosys scripts and logs:

| File | What it is |
|---|---|
| `design.json` | Every block of the circuit, where each input's lever and each output's lamp is, the settle time, the timing report, the clock (if any) |
| `world.nbt` | The same blocks as a vanilla structure file, ready for `/place template` |
| `report.txt` | Cell, tile, repeater and block counts, the footprint, the timing summary, and the checks that passed |

The build stops with a message if synthesis fails, if the proof cannot show the
mapped netlist equal to the source, if the layout does not implement the
netlist, or if routing does not converge.

`target/release/redfab` (after `cargo build --release`) takes the same arguments
and runs about 1.2 times as fast. Synthesis needs Node, `node_modules/` and the
Yosys runner `synth/yosys.mjs`. The binary looks for `synth/yosys.mjs` beside
itself and in its parent directories, so it works from `target/release` or from
a copy with a `synth/` directory next to it; `REDFAB_SYNTH=path` names the
runner directly.

The examples are under `designs/`: `not1`, `nor2`, `nor3`, `consts`, `fa1`,
`adder4`, `adder8`, `mul2`, `mux4`, `prio3`, `cmp4`, `param_add`, `signed_sub`,
`divmod`, `hier`, `rom4`, and the clocked `counter4`, `fsm`, `shift2`, `lfsr3`,
`negedge`, `init_reg`, `mem2`. `./check` builds every one, proves it and
compares it with Verilator. Between them they cover arithmetic (including `/`,
`%` and signed values), a mux by index, `always_comb` with `case`, parameters,
hierarchy, a constant array, a memory, a synchronous reset, a falling-edge clock
and an initial value.

## What compiles, and what is refused

redfab takes the synthesisable subset that Yosys's slang frontend reads and maps
it to NOR gates, inverters, constants and one flip-flop, with the rules below.
Each item that compiles is in an example under `designs/` or a source in
`tests/smoke/`, and the gate builds all of them.

Compiles:

- Combinational logic: `assign`, `always_comb`, `case`, operators including `*`,
  `/`, `%`, comparisons, signed arithmetic (`logic signed`), parameters
  (`module top #(parameter int WIDTH = 4)`) and hierarchy (instantiated modules).
- One clock: a single input that feeds nothing but `always_ff` clocks, on
  `posedge` or `negedge`. Synchronous reset and enable (`if (rst) ... else if
  (en) ...` inside the clocked block), and `initial` values on registers.
- Memories and constant arrays (`logic [3:0] mem [0:3]`, lookup tables). They
  become flip-flops and muxes, so only small ones fit: a 2x4-bit register file is
  57 cells.
- Port ranges of any shape (`[7:4]`, `[0:3]`). Levers and lamps are named by the
  declared index.
- Registers start at 0 unless the source gives an initial value. The build
  compares the initial values with the source after the proof.
- At most 64 input bits (a 32-bit adder with carry in has 65).

Refused before anything is placed, each with a one-sentence message, most with
the source line. `tests/smoke/*.sv` holds one source per refusal, and the CLI
test checks the exact text:

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

## Limits (measured 2026-11-01)

A build lays out each design several ways (placement styles, and routing seeds
for small designs) and keeps the best one that passes its checks, so build time
grows quickly with size.

| Limit | Value | Where it comes from |
|---|---|---|
| Build time | the 8-bit adder (102 cells, 17 inputs) in under 35 s; the 4-bit CPU (132 cells, clocked) in about 20 minutes, with the release binary | the router and the candidate layouts (`crates/redfab-core/src/route.rs`, `build.rs`) |
| Playground | warns above about 100 cells (`measuredCells`) and stops a compile at 90 s | `playground/compile.js` |
| Fits a plot on the live server | footprint up to 512 x 512 blocks; every example fits, the widest is `adder8` at 250 x 105 | `server/live.mjs` |
| Inputs | 64 bits | `build.rs` (`MAX_INPUTS`) |
| Clock period | `counter4` needs 62 game ticks (3.1 s) per cycle, `mem2` 52 (2.6 s) | repeaters on long feedback paths |

Sizes of the examples and of 2 larger designs, for scale: cells after mapping,
blocks, footprint in blocks, and clock period in game ticks. Every example builds
from source in under 35 s with the release binary. The register file and the CPU
are from `tests/capacity/`, not examples.

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
   (`blocks: ... in 250 x 4 x 105` means 250 blocks east, 4 up and 105 south of
   the origin). The structure file leaves out air, so anything already inside
   the footprint stays there, and a solid block next to dust conducts: place it
   in open air, or `/fill` the box with air first.
3. Keep the whole footprint loaded. Redstone only ticks in chunks within the
   simulation distance (10 chunks, 160 blocks, by default), so the far end of a
   wide design freezes when you walk to the levers. Force-load the box with
   `/forceload add X Z X+width Z+depth`. The game takes at most 256 chunks per
   command, so split a bigger box.
4. `/place template redfab:<name> X Y Z none none 1.0 0 strict`. `strict`
   places every block exactly as written, without block updates, which is what
   the baked state is made for. Placed without `strict` it settles to the same
   state; the checks test both.

`report.txt` lists every input's lever and every output's lamp as `x,y,z` from
the origin (x runs east, z south, y up; the levers stand on their probe blocks
on the west edge, and the lamps are on the east edge), the footprint in chunks,
and the `/forceload add` line for a design placed at 0, 0. `design.json` has the
same positions as data. `uv run --frozen python -m harness.show build/designs/nor2`
starts a local server with the design already placed and prints the levers and
lamps in world coordinates.

`report.txt` also says how fast you can drive it:

- A combinational design gets the line `outputs settle A to B game ticks after
  an input change; inputs may change every N ticks (found over the stress
  walks)`. Wait B ticks (20 ticks is 1 s) before reading the lamps, and flip
  inputs no closer together than N ticks, or a torch burns out (it relights after
  8 s). N is found by sampling, so a few multi-bit changes on a larger design may
  need more.
- A clocked design gets a line starting `in the world: give clk a period of at
  least`, with the clock's period and high time, the window after the clock edge
  in which the other levers may move, and when to read the lamps, in ticks and
  seconds. The clock lever is one of the inputs. An input changed late in the
  period can reach some flip-flops and not others, and a counter then skips.

## The playground

```sh
cargo build -p redfab-wasm --target wasm32-unknown-unknown --release
node tools/playground_serve.mjs
```

Open http://127.0.0.1:8765/. The page opens with the counter example already
built and counting (the server builds it once from `designs/counter4`). Your own
source compiles in a worker in your browser, with YoWASP Yosys (67 MB, fetched on
the first compile) and the core built for WebAssembly, so nothing is sent
anywhere. The page draws the blocks and runs them with the same simulator the
game recordings check. Click a lever in the view or in the panel; a bus shows its
value beside its bits. The page drives a clocked design's clock at the reported
period, counted in simulated ticks. The top module is the one named `top` or the
only module in the source, and the `top` field names another. `playground/README.md`
has the rest.

## The live server

```sh
tools/build_mod.sh
node server/live.mjs
```

Open http://127.0.0.1:8766/ and click Run in the game. The backend starts a
Fabric server with the observer mod (`mod/`), compiles your source in a
sandboxed child process, places the result in a free plot, and streams the
game's own block changes back to the page. The page's levers reach the probe
blocks in the world and nothing else. Everything since placement is recorded,
and `GET /api/plots/N/trace` (with the plot's token) returns a recording that
`redfab check-fixtures` can replay.

`server/README.md` covers running it behind a reverse proxy (`--host`,
`--trust-proxy`, and TLS at the proxy, since the plot token travels in the URL);
the game port stays on 127.0.0.1. `deploy/README.md` is the runbook for putting
it online on one machine with Caddy in front, and `tools/setup.sh --server`
installs only what that machine needs.

## Where to read more

- `docs/PLAN.md`: the staged plan and the log of what each stage found.
- `docs/DESIGN.md`: the design document the plan came from.
- `docs/mechanics.md`: what the game does, measured, with a fixture per fact.
- `docs/trace-format.md`: the recording format every part shares.
- `docs/REVIEW-2026-11-01.md`, `docs/REVIEW-2026-10-31-optimisation.md` and
  `docs/REVIEW-2026-10-07.md`: the three reviews, newest first.
- `cells/README.md`: the tile library and the rules every tile follows.
