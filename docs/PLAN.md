# redfab: plan and stage tracker

This is the approved staged plan (2026-10-01) plus the status of each stage. Design rationale is in `DESIGN.md`.

## Status

| # | Stage | Status | Result |
|---|---|---|---|
| 0 | Setup and synthesis check | **done** 2026-10-01 | All 11 checks pass. YoWASP Yosys reads SystemVerilog, with `--threads 1` |
| 1 | Vanilla test harness | **done** 2026-10-04 | 25 mechanics fixtures recorded in both redstone modes; all expectations met except the wiki's burnout threshold (game burns out on the 8th turn-off, not the 9th) |
| 2 | Simulator, part 1 | **done** 2026-10-05 | Block-level simulator matches all 417 recorded traces (36 mechanics, 381 random); structure files load in the game |
| 3 | First circuit: one NOR gate | **done** 2026-10-05 | `not1`, `nor2`, `nor3` go from SystemVerilog to working circuits in the game; every check from the plan passes |
| 4 | Simulator, part 2 | **done** 2026-10-05 | Repeaters, locking and tick priorities; all 60 mechanics traces and both random corpora (764 traces) match |
| 5 | Full combinational tile set | **done** 2026-10-05 | Eight tiles, 39 variants; all hold in the game and match the simulator; the tile contract holds for all 3,442 pairs and groups checked |
| 6 | Placement and routing | **done** 2026-10-06 | Any combinational netlist places and routes; the 4-bit adder (49 cells, 1,273 tiles) is correct on all 512 input combinations in the simulator and in the game; 180 random netlists build and compute correctly |
| 7 | Timing analysis | not started | |
| 8 | Flip-flop characterization | not started | |
| 9 | Counter and state machine | not started | |
| 10 | Browser playground, no server | not started | |
| 11 | Server mod and live stream | not started | |
| 12 | Live playground, local | not started | |
| 13 | Put it online | not started | |

## Stage log

Findings and surprises from each stage go here, newest last.

### Stage 0 (2026-10-01)

**Pinned tools** (`versions.env`, `rust-toolchain.toml`, lock files): Minecraft server 26.3 (world version 5023, protocol 777, read from the jar), Temurin JDK 25.0.4.1, Rust 1.99.0, Verilator 5.052, YoWASP Yosys 0.68 (PyPI `0.68.0.0.post1208`, npm `0.68.1207`, same Yosys commit `38e001a6f`).

**Findings:**

1. **`read_slang` works in the WebAssembly Yosys build only with `--threads 1`.** Without it, it aborts with "thread constructor failed: Not supported". With it, the Python and Node builds produce byte-identical netlists. YoWASP is therefore the only synthesizer; OSS CAD Suite is not installed.
2. **The server's RCON drops the connection if two requests arrive in one socket read.** My first client sent a command and an end-marker packet back to back and failed in 5 of 6 runs. The client now waits for the first reply before sending the end marker. Requests are also limited to 1446 bytes. Stage 1 must batch long command sequences with this in mind.
3. **The server closes the connection while handling `stop`**, so the reply may never arrive. The harness treats that as normal and falls back to SIGTERM.
4. The server starts in about 6–7 seconds on this machine, and a fresh world directory is about 2 MB.

**Notes for stage 1** (seen in the generated `server.properties` and log):

- `level-type=minecraft:flat` with no `generator-settings` logs "No key layers" and falls back to the default flat world. Stage 1 needs an explicit empty-world preset.
- `pause-when-empty-seconds` defaults to 60, and the log shows "Server empty for 60 seconds, pausing". It must be disabled before any timing test.
- `initial-enabled-packs` exists; it is the likely way to turn on Redstone Experiments at world creation.
- `max-chained-neighbor-updates` exists and may matter for large circuits.
- 26.3 has a built-in `management-server-*` (off by default). Worth a look in stage 11.

**Other:**

- The Mac sleeps when idle, which stalls long test runs (wall-clock gaps of 15+ minutes between 7-second tests). `./check` runs under `caffeinate -i` to prevent idle sleep while it runs.
- `brew install verilator` also ran Homebrew's routine cleanup, which removed outdated copies of `nettle`, `p11-kit` and `pango` and some download caches.
- Disk after stage 0: about 1.2 GB (`.tools` 420 MB, `~/.rustup` 570 MB, Python and Node packages 175 MB).

### Stage 1 (2026-10-04)

**Built:** `harness/` can start a void-world server in either redstone mode, freeze it, place blocks with or without updates, step exact tick counts, and snapshot every block state and pending block tick by saving the world and parsing the region files. `harness/runner.py` runs many fixtures side by side on one tick clock, each at three placements (chunk-aligned, straddling a chunk border, rotated 90°), and maps results back to the fixture's frame. `python -m harness.mechanics record|verify` drives the 25 experiments in `fixtures/mechanics/`; the format is in `trace-format.md` and the results in `mechanics.md`.

**Findings that changed the plan or the design doc:**

1. **Torch burnout happens on the 8th turn-off in 60 game ticks**, not the 9th as the wiki says. The torch then relights 160 ticks later by itself. The design budget stays at 7.
2. **A stimulus issued between ticks acts as the last event of the previous tick.** The trace format now says so, and every delay expectation is written in those terms.
3. **`setblock lever[powered=true]` does not power anything.** Inputs are driven with `redstone_block`, which also fixes how the online playground will flip levers.
4. **`setblock redstone_wire` makes a dot, not a cross.** Lone dust placed by commands powers nothing beside it.
5. **Saved chunk files omit default property values, and `save-all` skips chunks whose only change is a new scheduled tick.** The harness restores defaults from the server's own registry and touches a marker block in every plot chunk before each save.
6. The two redstone modes agreed on all 25 fixtures.

**Harness performance:** 75 plots, 220 ticks, both modes: about 45 s per mode plus the 66 s idle-pause proof. Snapshot parsing was the bottleneck (0.5 s per tick) until the NBT reader became lazy and decoded only the plot boxes (0.05 s).

**Still unmeasured:** `/place template` above 48³ (stage 2), flip-flop timing (stage 8).

### Stage 2 (2026-10-05)

**Built:** `redfab-core` now has typed blocks with the game's state strings, a world grid, a block-level simulator (`src/sim`: signal queries, wire shapes, the wire power evaluator, torches with burnout, lamps, scheduled ticks with priorities and de-duplication), trace replay with a side-by-side text report (`redfab replay`), and a structure-file writer. `harness/fuzz.py` generates random circuits and records them from the game; `harness/place_template.py` loads redfab's structure files in the game.

**Results:**

- The simulator reproduced the 36 in-scope mechanics traces on its first run, block states and pending ticks.
- Across about 1,000 random circuits (seeds 11–23 and the corpus) the only real disagreements came from two causes, both understood: races (below) and a generator bug that placed dust in mid-air.
- The committed corpus is 200 circuits, 381 traces, all matched.

**Decisions and deviations from the plan:**

1. **The simulator runs on blocks, not on a component graph.** The extractor moves to stage 3, where the layout-versus-netlist check needs it.
2. **Wire power follows the Redstone Experiments rules.** 6% of random circuits behave differently in default mode. Tiles and designs must be mode-independent; the harness keeps checking both.
3. **Races are a first-class result.** The replay tries other orders of same-moment events (scheduled ticks due together; one update delivered to several neighbours). A trace that some order reproduces is a race in the circuit; one that no order reproduces is a simulator bug. The same shuffling is what later stages use to reject racy designs.
4. **Torch burnout was done here** instead of stage 4, since the stage 1 measurements made it simple.
5. **Unsupported dust and torches are refused**, not modelled.

**Measured along the way:** the 26.3 structure format uses `{id, properties}` palette entries; `/place template` reads `generated/<ns>/structure/`; a 70×2×70 structure loads, so there is no 48-block limit there.

**Not done:** the SVG layer view (the text view in `redfab replay` has been enough so far).

### Stage 3 (2026-10-05)

**Built:** the whole path once, for single-gate designs.

- `cells/`: four tiles as text (`input`, `output`, `wire`, `nor`) with their rules in `cells/README.md`; the parser rejects a tile that touches its border or has a non-glass floor.
- `redfab build top.sv -o DIR`: Yosys maps to the library and proves the mapped netlist equal to the source; the core reads the netlist, places the tiles, computes the settled all-inputs-off state, reads the circuit back from the blocks, and checks it against the netlist. Output: `design.json`, `world.nbt`, `report.txt`.
- `harness/design.py`: Verilator versus the block simulator on every input combination; the same in the game; broken copies. `harness/show.py NAME` starts a server you can join to look at a build.
- `crates/redfab-wasm`: the core as a WebAssembly module with a three-function interface.

**Results** (designs `not1`, `nor2`, `nor3`):

- Simulator and Verilator agree on every input combination (2, 4 and 8).
- In the game the outputs are right at three placements in both redstone modes, and the recorded traces equal the simulator's.
- `world.nbt` placed strictly does not move in 20 ticks; placed with updates it settles to the same state.
- Broken copies are rejected: four by the netlist check (cut wire, missing torch, redstone block in place of the torch, shorted inputs) and two in the game (cut wire, stuck output). A tampered netlist fails the equivalence proof.
- The WebAssembly build's `design.json` and `world.nbt` are byte-identical to the native ones.
- With time running normally on an ordinary flat world, the NOR gate answers correctly through the server console.

**Findings:**

1. **ABC does not run in the PyPI build of YoWASP Yosys** under its Python runtime: the process exits during the `abc` pass, with status 0 and no error. The npm build under Node works, so all synthesis goes through Node. The stage 0 checks that compare the two builds on `read_slang` stay as they are.
2. **ABC needs a buffer cell in the library** ("Cannot find buffer gate"). `BUF` is in the Liberty file with a high area; the importer turns buffers into wire.
3. **The order of stimuli within one tick matters.** When two inputs of the NOR swap values in the same tick, the gate sees one of the intermediate states and may schedule a torch tick that changes nothing. The trace format now fixes that order (height, then x, then z), and the harness and the replay both follow it.

**Limits, by design:** the placer handles one gate fed directly by inputs. Anything else is refused with a message until stage 6. Constants are refused until stage 5 adds tie tiles.

### Stage 4 (2026-10-05)

**Built:** repeaters in the simulator (`sim/repeater.rs`): the four delays, pulse stretching, locking from the side, the three tick priorities, the `locked` property, and the updates a repeater sends when it changes. The fuzzer now builds repeaters and latches. Torch burnout was already done in stage 2.

**Results:**

- All 60 mechanics traces replay, none skipped (stage 2 had to skip the 12 with repeaters). Five new repeater experiments were recorded first; all five behaved as the wiki says.
- About 770 fresh random circuits with repeaters (seeds 31–34 and the new corpus): every trace matched. No simulator change was needed after the first run.
- The new corpus `fixtures/fuzz-repeaters/` has 200 circuits and 383 traces; 3 need another order of same-moment events.
- Five deliberately wrong repeater rules (delay off by two ticks, lock ignored, no pulse stretching, one priority for everything, no strong power in front) are each caught by both the corpus and the mechanics fixtures.

**Also:** the Rust code now has a `rustfmt.toml` (120 columns, fewer forced line breaks); the one-time reformat touched every file.

**Not in this stage:** reading repeaters back out of a layout for the netlist check. That comes with the repeater tile in stage 5.

### Stage 5 (2026-10-05)

**Built:** the full combinational tile set (`cells/`): `wire` (any two to four edges: straight, turn, fan-out), `cross`, `repeater` (any of four orientations, settings 1 to 4), `nor` (one to three inputs), `tie_high`, `tie_low`, plus `input` and `output`. Tiles can be placed rotated. Three new checks: a fixture per tile variant with drivers and readers on its ports and expectations from its description (`redfab tile-fixtures`, `check-fixtures`); the static tile-contract check over every pair of variants and random 2×2 groups (`redfab tile-adjacency`); and `harness/tiles.py`, which runs every fixture and random tile groups in the game. `harness/show.py tiles` places the whole library in a row to look at.

**Results:**

- The tile size is fixed at 5 × 5 × 4: the crossing fits, and nothing smaller could hold it.
- All 39 variants and 24 random groups hold in the game at three placements in both modes (126 traces), and every trace matches the simulator.
- The static contract check passes for all 3,442 pairs and groups.

**Two defects caught before the game saw them:**

1. The static check flagged the **input tile**: its lever's block sat on the tile border, so as a redstone block it would have powered a neighbouring tile's wire. The block moved one cell in, and the border rule no longer makes an exception for it.
2. The simulator flagged the **crossing**: the north-south wire climbed onto its bridge but could not come down, because a signal only steps down onto a block that conducts and the steps stood on glass. The steps are stone now; the west-east wire beside them is unaffected because a wire reads neither weakly powered blocks nor, with a conducting block above it, anything diagonal.

**Not in this stage:** reading repeaters and constants back out of a layout for the netlist check, and using the new tiles in a build. Both come with placement and routing in stage 6.

### Stage 6 (2026-10-06)

**Built:** placement (`place.rs`): gates in columns by logic depth, four rows per gate (north feed, gate, south feed, a free row), rows ordered by barycenter sweeps, inputs and constants on the west edge, outputs on the east, pin sides chosen per gate to shorten feeds. Routing (`route.rs`): PathFinder negotiated congestion over tile tracks (each tile has an X and a Z track; a crossing gives one to each of two nets), then exact signal-strength tracking along each net with repeaters inserted at the last tile that can take one, and a re-route with penalties when a signal dies inside a run of crossings. Routing room grows (one to six routing columns between gate columns) until a netlist routes. The netlist check (`lvs.rs`) now follows nets through repeaters, anchors constants at their tie tiles, and matches cells with identical inputs as a group. New commands: `redfab layout` (a text picture of a routed layout), `redfab pnr-random` (random netlists must build and compute correctly), `redfab extract` (what the checker reads back from a design), and `redfab build MAPPED.json` (build from a netlist without synthesis). New designs: `fa1`, `adder4`, `consts`. In the game: `harness.design exhaustive` steps every input combination and reads the lamps; the traced run samples 32 vectors of large designs.

**Results:**

- `adder4`: 49 cells, 1,273 tiles (73 × 50), 455 repeaters, settles in 144 game ticks; all 512 input combinations correct in the simulator and in the game in both redstone modes.
- 180 random netlists (three seeds of 60, up to 40 gates, with constants, fan-out, inputs wired to outputs and gates reading one net twice) place, route, pass the layout-versus-netlist and strength checks, and compute what the netlist says on up to 16 vectors each.
- The crossing can now carry a repeater in its flat arm (three crossing variants, 54 variants in all); all hold in the game.

**Four defects the random netlists found, all fixed:**

1. A signal running through consecutive crossings had nowhere for a repeater (crossings could not hold one) and died. Now the flat arm of a crossing takes a repeater like a wire; the tile is turned when the climbing signal is the one that needs it.
2. In a column of 3-input gates every row held a gate or a feed, so nets heading further east had no way through, and routing never converged. The placer now sizes rows by the nets that must cross each gate column.
3. An input that nothing reads left an open port. Its tile now has a lever and no dust.
4. The netlist check matched `NOT(a)` to the torch of `NOR2(a, a)` and failed everything downstream. Cells with the same inputs are now matched as a group.

**A harness defect found by the first large design:** `save-all flush` replies before the game has finished writing the region files, and while it moves chunk records around inside a file, a header entry can point at nothing or at another record for a moment. A snapshot read in that moment saw a chunk empty, or one block out of place, for a single tick (three times in about 3,500 chunk-ticks of the adder). Every chunk the harness reads is force-loaded and was changed that tick, so the reader now insists that each chunk has a record, that the record decodes and names that chunk, and that its `LastUpdate` is the current game time; otherwise it reads again (`harness/anvil.py`, `harness/session.py`). Fixtures big enough to show this also needed plot cells sized per fixture instead of the fixed 32 blocks, force-loading in strips (the game takes at most 256 chunks per command), and a limit on the ticks a traced run records (the adder's 32-vector trace would have taken hours; it records two vectors, and `exhaustive` covers the rest without traces).

**Known costs, left for later stages:** the adder is 365 × 250 blocks, about 3× the hand-layout density the review predicted, and 144 ticks of settle time come mostly from 455 repeaters on long feeds. Stage 7's timing analysis replaces the settle estimate; density is a later compaction pass.

---


## Context

`DESIGN.md` reviews the redfab idea (SystemVerilog → vanilla redstone) and recommends an architecture, but no code exists yet. Your end goal is an online playground: write Verilog in a browser and watch it run on a real Minecraft server.

This plan breaks that into stages I program and set up myself. I can't see or play the game, so every stage ends in checks I can run from the terminal, and the real vanilla server is the judge of whether a circuit works.

Your decisions so far: Rust core, a local vanilla 26.3 server (EULA accepted), I stop and report after each stage, and I make a local git commit when a stage's checks pass (no pushing).

## Rules that apply to every stage

- **One command, `./check`, runs every check from every stage so far.** A stage is done only when it passes. Checks that need the Minecraft server run under `./check --vanilla`.
- **I write and commit a stage's checks before its implementation**, so I can't quietly weaken them later.
- **Every experiment on the server has a control that must pass and a control that must fail.** A test that can't fail proves nothing.
- **If the game contradicts the wiki or `DESIGN.md`, I stop and show you** instead of silently changing my model. The main risk is that I write both the model and the tests from the same wrong belief.
- **The core library must build for WebAssembly from the first commit**, so the playground doesn't need a rewrite later.
- **Recorded game traces are only regenerated by one command** that stores the server jar's hash next to them.

## Stages

| # | Stage | What exists when it's done |
|---|---|---|
| 0 | Setup and synthesis check | Tools installed, repo skeleton, `./check` works |
| 1 | Vanilla test harness | I can place blocks, step ticks and record what the real game does |
| 2 | Simulator, part 1 | My simulator matches the game for dust, blocks, torches, lamps |
| 3 | First circuit: one NOR gate | `y = ~(a \| b)` goes from Verilog to a working circuit in the game |
| 4 | Simulator, part 2 | Repeaters, locking, tick order and torch burnout match the game |
| 5 | Full combinational tile set | Turns, fan-out, crossings, repeaters, NOR1–3, constants |
| 6 | Placement and routing | Full adder and 4-bit adder build and work in the game |
| 7 | Timing analysis | Tool reports the worst-case settle time, and it's exact |
| 8 | Flip-flop characterization | A flip-flop tile with measured setup, hold and delay |
| 9 | Counter and state machine | 4-bit counter and an FSM run correctly for many clock cycles |
| 10 | Browser playground, no server | Editor + 3D view running entirely in the browser |
| 11 | Server mod and live stream | A small server mod streams block changes to the browser |
| 12 | Live playground, local | Type Verilog in the browser, watch it run on your local server |
| 13 | Put it online | Public site (needs your accounts and decisions) |
| Later | Extras | `.litematic` export, on-board clock, 8-bit ALU, tiny CPU |

### Stage 0: Setup and synthesis check
- **Build:** `git init`; move `DESIGN.md` to `docs/`; add `docs/PLAN.md` as the stage tracker; Rust workspace with two crates, `redfab-core` (no file, thread or clock access) and `redfab-cli`.
- **Install:** see "What gets installed" below.
- **Risk check:** confirm the WebAssembly build of Yosys (YoWASP) can read SystemVerilog through `read_slang`, in both Python and Node. If it can, YoWASP becomes the only synthesizer, so your machine and the browser always produce the same netlist. If not, fall back to the native OSS CAD Suite.
- **Checks:** `read_slang` parses a trivial file under both YoWASP builds at one pinned version; Verilator compiles and runs a trivial module; the server starts, answers a remote command and shuts down leaving no stray process; the core builds for native and WebAssembly.
- **Also:** update `docs/DESIGN.md` for what changed (two crates, YoWASP, an online-playground section).

### Stage 1: Vanilla test harness
- **Build:** Python harness in `harness/` that starts a fresh empty world, places blocks without triggering updates, freezes time, steps exact tick counts, reads block states and records traces. It runs many test plots side by side on one shared tick clock.
- **Define once:** `docs/trace-format.md`, a starting snapshot plus per-tick block changes. The game recordings, my simulator, the browser viewer and the server mod all use this one format.
- **Experiments:** settle the seven facts marked UNVERIFIED in `DESIGN.md` Appendix A (burnout threshold, lamp turn-off delay, same-tick repeater locking, and so on).
- **Checks:** measured repeater delays, torch delay and 15-block signal decay match the documented values; the tick counter advances by exactly N on every step; a test gives identical traces at three positions (one across a chunk border), in two orientations, after a restart, and in both redstone modes; a deliberately wrong expectation is reported as a failure.
- **Output:** committed trace files and `docs/mechanics.md` listing what the game actually does.

### Stage 2: Simulator, part 1
- **Build in `redfab-core`:** typed blocks and world, dust shape rules, a writer for vanilla structure files (`.nbt`), an extractor that turns blocks into a circuit graph, a tick-accurate simulator, and a text/SVG layer view for debugging.
- **Scope:** dust, solid blocks, torches, lamps, redstone blocks only.
- **Checks:** simulator output equals every Stage 1 recording, block for block, tick for tick; random small block arrangements give the same result in the game and in the simulator; a deliberately wrong rule injected into the simulator is caught by that random test.

### Stage 3: First circuit, one NOR gate
- **Build:** four tiles written as text templates (wire, NOR2, input, output); the cell library file generated from them; the Yosys script; netlist import; trivial placement; a consistent starting state baked into the output; the layout-versus-netlist check; `redfab build top.sv`.
- **Checks:** Yosys proves the synthesized netlist equals the source; the extracted circuit matches the netlist; simulator and Verilator agree on all four inputs; the game gives the right output at three positions; placing it without updates changes nothing over 20 ticks, and placing it with updates settles to the same state; three deliberately broken copies fail; native and WebAssembly builds produce identical output.
- **You (optional):** look at it in the game by joining the local server, or by pasting the `.nbt` file with Litematica.

### Stage 4: Simulator, part 2
- **Build:** repeaters with delays 1–4, repeater locking, the game's tick-priority order, torch burnout counting.
- **Checks:** same as Stage 2 with the wider block set; burnout triggers at the threshold measured in Stage 1.

### Stage 5: Full combinational tile set
- **Build:** turn, fan-out, crossing, repeater, NOT, NOR3, constant-high and constant-low tiles; fix the tile size.
- **Checks:** each tile's truth table and delay in the game match its description; every pair of neighbouring tiles, and 2×2 groups, behave as intended in the game, in both redstone modes and at several positions.

### Stage 6: Placement and routing
- **Build:** column placement by logic depth, pass-through tiles for long connections, ordering to reduce crossings, the PathFinder router on the tile grid, repeater insertion from exact wire lengths.
- **Checks:** random netlists always produce layouts whose extracted circuit matches the netlist; the full adder and the 4-bit adder are correct on every input combination, in the simulator and in the game.

### Stage 7: Timing analysis
- **Build:** worst-case and best-case arrival times in game ticks, plus a report.
- **Checks:** on the adders, the reported worst-case time equals the slowest settle time actually observed across all inputs; routing is deterministic for the same input.

### Stage 8: Flip-flop characterization
- **Build:** two candidate flip-flop tiles; sweeps in the game that move the data edge one tick at a time against the clock edge, with several clock pulse widths.
- **Checks:** the simulator reproduces the whole sweep table, including the region where the flip-flop fails; the chosen design keeps at least one redstone tick of margin that doesn't depend on same-tick ordering.
- **Output:** setup, hold, clock-to-output and minimum pulse width written into the tile's description.

### Stage 9: Counter and state machine
- **Build:** flip-flop mapping in synthesis, a register column with a clock line, hold-time and clock-skew analysis that fixes problems by changing repeater delays, initial values and synchronous reset, the torch-burnout budget check, and a test that shuffles same-tick event order.
- **Checks:** a 4-bit counter matches Verilator for at least 64 clock cycles in the game, in both redstone modes and at three positions; a deliberate hold-time violation is flagged by the tool and also fails in the game; a deliberately glitchy circuit is flagged for burnout and also burns out in the game; then a small state machine passes the same checks.

### Stage 10: Browser playground, no server
- **Build:** the core compiled to WebAssembly, YoWASP Yosys in a background worker, a code editor, a 3D view driven by the in-browser simulator, clickable input levers, limits on design size and compile time.
- **Checks:** an automated headless browser types Verilog, compiles, toggles inputs and reads lamp states that match the expected values; a screenshot I inspect; the browser simulator's trace equals the native one; oversized or hostile inputs are rejected within the limits.
- **You:** try it in your browser.

### Stage 11: Server mod and live stream
- **Build:** a small Fabric server mod that only observes and streams block changes with their tick number over a WebSocket. First task is confirming a trivial mod loads on 26.3.
- **Checks:** all Stage 1 recordings re-run on the modded server give identical traces, which shows the mod doesn't alter redstone; the stream agrees with direct block queries.
- **Fallback if Fabric fails on 26.3:** poll block states over the remote console (slower), and tell you.

### Stage 12: Live playground, local
- **Build:** a backend that recompiles submissions inside a WebAssembly sandbox with CPU and memory limits, assigns each one a plot, places it, and maps web levers to blocks in the world; the browser view switches from simulator to live server data.
- **Checks:** a scripted browser session submits the counter and the live stream matches the simulator's trace tick for tick; hostile inputs and out-of-plot placements are rejected; server tick time stays within budget under several plots.

### Stage 13: Put it online (needs you)
- Hosting provider, domain, budget, usage limits, the required "not an official Minecraft product" notice, and textures that aren't Mojang's. I'll bring options and costs when we get here.

### Later
`.litematic` and `.schem` export (Litematica already loads the `.nbt` files from Stage 3 on), an on-board clock generator, the 8-bit ALU with speed benchmarks, and the register-file/ROM generators and tiny CPU as a showcase.

## What gets installed

| What | Where | Why |
|---|---|---|
| Rust via rustup, plus the WebAssembly target | Your home folder (`~/.cargo`, `~/.rustup`) | Compiler core |
| Verilator | Homebrew | Reference simulator for Verilog |
| JDK 25 and the official Minecraft 26.3 server jar | `.tools/` inside the project | Minecraft 26.x needs Java 25; you only have Java 21 |
| YoWASP Yosys, Python test libraries | A project-local Python environment (via `uv`) | Synthesis and the harness |
| Node packages, Playwright's browser | Inside the project (Stage 10) | Playground and its tests |
| Gradle and Fabric tooling | Inside the project (Stage 11) | Server mod |

Your disk has 28 GB free (94% full). I expect this to use roughly 6–8 GB by Stage 12, mostly Rust build output. I'll report disk use at each stage.

## Where things live

- `crates/redfab-core/`: blocks, world, extractor, simulator, tiles, placement, routing, timing, export
- `crates/redfab-cli/`: the `redfab` command
- `cells/`: tile templates as text, with their descriptions and recorded game traces
- `harness/`: Python server harness and experiments
- `designs/`: `nor2`, `fa1`, `adder4`, `counter4`, `fsm`
- `web/` (Stage 10), `mod/` (Stage 11), `server/` (Stage 12)
- `docs/`: `DESIGN.md`, `PLAN.md`, `trace-format.md`, `mechanics.md`

## How each stage is verified

- `./check` after every change; `./check --vanilla` before I call a stage done.
- At each stop I report: which checks passed, anything the game did that I didn't expect, disk use, and what the next stage will do.
- Your in-game looks at Stages 3, 5 and 9 are optional; the checks don't depend on them.
