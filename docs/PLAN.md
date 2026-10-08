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
| 6 | Placement and routing | **done** 2026-10-06 | Any combinational netlist places and routes; the 4-bit adder (49 cells, 921 tiles since the repair pass) is correct on all 512 input combinations in the simulator and in the game; 180 random netlists build and compute correctly |
| 7 | Timing analysis | **done** 2026-10-06 | Best and worst case per output: a bound by path and exact figures over every pair of input combinations from a timed model of the parts; the model agrees with the block simulator on every lamp change of 10k input changes of the adder, and the game stays within the report |
| 8 | Flip-flop characterization | **done** 2026-10-06 | Two candidates built and swept tick by tick; the master/slave pair of locked repeaters chosen (setup 1, hold 5, clock-to-output 8, period ≥ 12); the sweep tables are reproduced by the simulator and the game |
| 9 | Counter and state machine | **done** 2026-10-06 | Flip-flops synthesized, placed and routed; sequential timing with hold fixes and a burnout budget; the 4-bit counter and a state machine match Verilator for 64 cycles in the game at three placements in both modes; a hold-broken copy and an over-driven inverter fail as predicted |
| 10 | Browser playground, no server | **done** 2026-10-06 | The page compiles SystemVerilog with YoWASP Yosys and the WebAssembly core in a worker and runs the result in the 3D view with the same simulator the game was checked against; a headless browser types each design, compiles, flips levers and reads the right lamps; the pipeline under Node gives byte-identical designs; limits hold |
| 11 | Server mod and live stream | **done** 2026-10-06 | Fabric Loader 0.19.5 runs on 26.3; the observer mod (two Mixins, a built-in WebSocket server, no Fabric API, compiled with javac since 26.3 ships unobfuscated) streams every block change with its tick; all mechanics recordings replay exactly on the modded server and the stream agrees with the disk tick for tick |
| 12 | Live playground, local | **done** 2026-10-07 | A Node backend compiles submissions in a sandboxed child process, places them in plots on the modded server and relays the stream to the page; the counter driven from a headless browser records 946 ticks and 16 lever changes that replay natively tick for tick; six counters running cost 3.4 ms per tick |
| – | Repair pass (2026-10-07 review) | **done** 2026-10-07 | Every finding of `docs/REVIEW-2026-10-07.md` fixed or answered: the router's ceiling went from 76 cells to about 150 (an 8-bit adder builds in 10 s), a lint refuses every unsupported construct in one sentence before placing, memories and initial values build and are proven, 13 new example designs pin the subset, the live backend survives a dead game, and the page tells the truth about size. The fast gate grew from 37 to 41 checks (`python-lint`, `negatives-can-fail`, `js-syntax`, `docs-agree`), and every design check now covers 22 designs instead of 9 (23 with the capacity target `adder8`) |
| 13 | Put it online | not started | |

## Stage log

Findings and surprises from each stage go here, newest last.

### Stage 0 (2026-10-01)

**Pinned tools** (`versions.env`, `rust-toolchain.toml`, lock files): Minecraft server 26.3 (world version 5023, protocol 777, read from the jar), Temurin JDK 25.0.4.1, Rust 1.99.0, YoWASP Yosys 0.68 (PyPI `0.68.0.0.post1208`, npm `0.68.1207`, same Yosys commit `38e001a6f`). Verilator 5.052 and Node 26 come from the system (Homebrew, nodejs.org); `versions.env` records their versions and `./check` asserts them, but `tools/setup.sh` cannot choose them.

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

1. **Torch burnout happens on the 8th turn-off in 60 game ticks**, not the 9th as the wiki says. The torch then relights 160 ticks later by itself. The design budget (stage 9, `BURNOUT_BUDGET`) is 6 turn-offs in any 60 ticks, one to spare.
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

### Stage 7 (2026-10-06)

**Built:** `timing.rs`, on the circuit read back from the blocks (so it sees the real repeaters, not the netlist). Two answers per output, in game ticks after an input change:

- **By path:** earliest and latest arrival over the part graph, rising and falling edges followed separately (a torch answers a rising input with a falling output 2 ticks later and the other way round, a repeater passes either edge after twice its setting, a lamp lights at once and goes dark 4 ticks late). Quick and safe; the worst case becomes the design's `settle_ticks`. The report names the parts on the worst-case path.
- **Exact:** a timed model of the same parts with the block simulator's rules for torches, repeaters and lamps on the net graph, run over every ordered pair of input combinations (4ⁿ pairs; done up to ten inputs). For the 4-bit adder that is 261,632 pairs in about ten seconds.

`redfab timing DESIGN.json [--paths]` prints the report; `redfab timing-measure` is the check: the timed model must agree with the block simulator on every lamp change of a walk through every single-bit input change from every input combination plus random multi-bit changes, the exact figures must be what that walk shows, and the bound must contain everything. In the game, `harness.design timing-traces` reads the traces `design vanilla` recorded: no lamp changes outside its report, and the full adder reaches its worst case. `build-deterministic` builds the adder twice and compares the files.

**Results:**

- The timed model agrees with the block simulator on all 10,000 input changes of the adder's walk (every lamp change, to the tick), and on every other design.
- The game agrees too: the full adder's lamps change 18 to 34 ticks (`cout`) and 14 to 36 ticks (`sum`) after an input change, in both redstone modes, exactly the exact figures.
- The adder: `sum[0]` 48–78, `sum[1]` 64–106, `sum[2]` 60–124, `sum[3]` 70–138, `cout` 76–140 ticks (exact). By path, `cout` is 144: a **false path**. Over all 261,632 ordered pairs of input vectors, in the timed model and in a 63-minute run of the block simulator, `cout` never changes later than 140. This is the textbook ripple-carry case, where the longest path cannot be driven by any input change, and the reason the report keeps both figures: `settle_ticks` stays the safe bound (144), the exact figures say what will happen.
- Building the adder twice gives identical files.

**Surprise:** the plan's check, "the reported worst case equals the slowest settle time observed", does not hold for a plain longest-path analysis on the adder (the false path). The check now demands equality of the *exact* figures with what the simulator shows, and containment for the bound; the timed model is what makes the exact figures affordable.

### Stage 8 (2026-10-06)

**Built:** two flip-flop tiles, each two cells wide (10 × 5 × 4) because a side-locked repeater needs its lock's input one cell in from the border, which a one-cell tile cannot give (`cells/README.md`). `dff` is a master/slave pair of locked repeaters: the clock weakly powers a stone, a torch on it gives NOT clock to the master's lock, a second torch gives the clock back four ticks late to the slave's lock; it acts on the falling clock edge. `dff_alt` is a pulse latch: a single locked repeater opened for a few ticks after each rising edge by `NOT(clock delayed 2) OR (clock delayed 8)`. Sweep fixtures (`dfftest.rs`) join the tile fixtures: the data edge moved a tick at a time from 6 ticks before to 12 after the active clock edge, in both directions, at clock periods 16, 12 and 8; and pulse fixtures with the clock high or low for 2, 4, 6 and 8 ticks. The game runs them with the other tile fixtures and their traces must equal the simulator's. `redfab characterize` replays them, reads off when Q changed in every trial, derives setup, hold, clock-to-output, the minimum high and low times and the minimum period, compares them with the tile's `timing` line, and replays everything again under shuffled same-tick orders (ignoring the game's priorities) to find every trial whose outcome depends on ordering.

**Results (game ticks after the active edge):**

| | `dff` (master/slave) | `dff_alt` (pulse latch) |
|---|---|---|
| captures data changes up to | +1 | +7 |
| ignores them from | +3 | +9 |
| setup / hold (with 2 ticks of margin) | 1 / 5 | −5 / 11 |
| clock to Q | 8 | 8 |
| minimum high / low | 4 / 4 | 4 / 2 |
| minimum period | 12 | 16 |
| ordering-dependent trials | offset +2 only | offset +8 only |

Both are flip-flops. The master/slave is better on every figure, so it is `dff`. The review expected it to be a zero-margin design: the slave's lock and the master's last change can indeed land on the same tick at the inactive edge. Measured, that is harmless either way round, because a locked repeater ignores a pending tick, so the lock wins even when it is processed second; under 20 shuffled orders the only trial that changes is the capture boundary itself, which every flip-flop has.

**Caught on the way:** the first version of the master/slave tile had its second torch on dust instead of stone (a typo in the layer), and the quick replay used for characterization did not check block support; it measured a torch floating in the air as "transparent for three ticks". The replay now checks support like the fixture runner does. A clock of period 8 burns the torches out (eight turn-offs in sixty ticks), which is why those sweeps end with a long quiet time.

### Stage 9 (2026-10-06)

**Built:**

- *Synthesis.* A `DFF` cell in the Liberty library (`clocked_on: "!CLK"`, since the tile acts on the falling edge); `dfflegalize` turns a design's rising-edge flip-flops into it with an inverter on the clock, and the sequential equivalence proof (`equiv_induct`) covers the result. Initial values come from Yosys's `init` attributes. The clock must be one input that reaches every flip-flop straight (through the inverter) and feeds nothing else.
- *Layout.* Flip-flops form a column of their own, two cells wide, before the first gates; their D inputs are fed back from wherever the logic ends, and those feedback nets count when the placer sizes the pass-through rows. The router knows the flip-flop's ports and strengths. A flip-flop's stored value is baked in by setting its state repeaters' `powered` (`state` lines in the tile), with the lock and the second torch stamped in their rest state (`rest` lines), so the baked world is a fixed point without any settling race.
- *Checks.* The netlist check treats each flip-flop as one part: its blocks must be the tile's, its three ports must carry the right nets, and its insides stay out of the torch matching. Timing (`analyze_sequential`): the lever edge that makes every flip-flop's clock port fall is the active edge; clock arrival per flip-flop; D arrivals from the other flip-flops (launched at clock arrival + 8) and from the inputs; hold and setup slack with the tile's figures (setup 1, hold 5); the shortest period, the window in which inputs may change, and when outputs are read. A hold violation is fixed by lengthening that flip-flop's D path: a repeater's setting goes up, or a wire tile on its own branch becomes a repeater, and the analysis runs again. `redfab build --break hold` builds the opposite in on purpose. The burnout budget: every design reports the fewest ticks between input changes at which no torch turns off more than six times within sixty (`input_spacing`); clocked designs are checked at their period.
- *Driving.* `redfab simulate` on a clocked design treats each vector as one cycle, driven as the report says; `--shuffle` runs the ticks that fall due together in a random order within each priority. `harness.design cycles` compares 64 cycles with a clocked Verilator testbench, and with shuffled orders; `cycles-vanilla` does the same in the game by stepping and probing, at three placements in both modes.

**Results:**

- `counter4` (4 flip-flops, 22 gates, 806 tiles): period 132 ticks, clock arriving 18 to 34 ticks after the edge, hold slack ≥ 49. `fsm` (2 flip-flops): period 74. `shift2`: period 40. All three agree with Verilator on all 64 cycles in the simulator, unchanged under shuffled same-tick orders; the counter and the state machine agree in the game at every placement in both modes.
- The hold-broken shift register (its second flip-flop's clock slowed by 24 ticks) is flagged by the build and gets 40 of 64 cycles wrong, the same 40 in the simulator and in the game. The inverter driven every 4 ticks instead of its reported 8 is flagged by `redfab stress` (8 turn-offs within sixty ticks) and burns its torch out in the simulator and in the game (its lamp stays dark when it should be lit).

**Two things I'd planned differently:**

1. The plan's "deliberately glitchy circuit that burns out" cannot be made from a design at a legal input rate: in NOR logic with repeaters on every path, a single delayed input makes a torch pulse at most once per input change, which is not enough to reach eight turn-offs in sixty ticks. Burnout here is a matter of how fast the inputs are driven, so the budget is reported as a minimum input spacing, and the negative is a design driven faster than its report.
2. The first hold negative the tool flagged did not misbehave: by the tile's conservative figures the data arrived two ticks after the clock, which is the ordering-dependent boundary tick, and the simulator happened to capture the old value. The negative now slows the clock past the hold margin, so the new value is captured beyond doubt.

**Known costs, left for later stages:** the counter's period of 132 ticks (6.6 s) comes from its long feedback paths; the clock skew of up to 16 ticks across four flip-flops comes from the repeaters the clock net needs. Both are density problems for the later compaction pass; the analysis makes them safe, not small.

### Stage 10 (2026-10-06)

**Built:** `playground/` (see its README): a page with an editor, Compile, a panel of levers and lamps, and a three.js view of the blocks; a worker that runs YoWASP Yosys and the WebAssembly core; `compile.js`, the same pipeline as `redfab build` (synthesis, the equivalence proof, placement, routing, the checks) with the limits (source 64 KB, 400 cells, 80,000 blocks, 90 s, after which the page terminates the worker); and the core's one-call interface (`redfab_call`) with `build`, `library`, and `sim_new`/`sim_set`/`sim_step`/`sim_trace`/`sim_free`, so the view is driven by the same simulator the game was checked against. The synthesis scripts moved into the core (`synth.rs`) so the command line and the browser cannot drift apart. A trace recorder (`trace::Recorder`) lets what the browser simulated be replayed natively. `node tools/playground_serve.mjs` serves it locally.

**Checks:** `tools/playground_test.mjs` runs the pipeline under Node with the same Yosys and core the page uses: every design (including the clocked ones) comes out byte-identical to the native build; a stimulus simulated in the core's browser interface traces exactly as `redfab check-fixtures` replays it; a source over the size limit, a design over the cell limit and a bad module name are refused with a message; a compile that never finishes is stopped at the time limit and the worker is usable afterwards; a hostile source (a generate loop of four million gates) is refused within the limit. `tools/playground_check.mjs` drives the page in headless Brave: for the inverter, the NOR, the full adder and the constants it loads the source, compiles, flips the levers (the first through a click on the lever in the 3D view, the rest through the panel), steps the simulator and reads the lamps, which match the native simulator on every input combination; the counter compiles and runs at the game's 20 ticks a second with the page driving its clock. Screenshots land in `build/playground/`.

**Results:** everything above passes. Compiling the full adder in the browser takes under a second; the 4-bit adder about 14 s (its all-pairs timing check); the counter 3 s.

**Try it:** `node tools/playground_serve.mjs`, then open http://127.0.0.1:8765/ in a browser.

### Stage 11 (2026-10-06)

**Built:** `mod/` (see its README): a Fabric server mod of four Java files. A Mixin on `LevelChunk.setBlockState` collects every block state change in the overworld (the method returns the old state when something changed); a Mixin at the tail of `MinecraftServer.tickServer` sends the batch with the overworld's game time, which is what `/time query gametime` reports and the harness steps by (since stage 12 there is a second injection at the head of `tickServer`, which flushes what changed between ticks under the number of the tick just completed). A WebSocket server of its own (RFC 6455, text frames out, close and ping in; one writer thread per client so the server thread never waits on a socket) serves clients on localhost. No Fabric API. `tools/build_mod.sh` compiles with `javac` alone against the server jar, Fabric Loader and Mixin, which `tools/setup.sh` installs by running the pinned Fabric server launcher once (`.tools/fabric`). The harness gained `VanillaServer(modded=True)`, `harness/observe.py` (a receive-only WebSocket client) and `--modded` for the mechanics verification, under which the runner also lays the stream's changes over the first snapshot and compares with every later snapshot read from disk.

**Results:**

- Fabric Loader 0.19.5 loads on Minecraft 26.3; the mod is listed and both Mixins apply.
- Minecraft 26.3 ships unobfuscated (Fabric lists intermediary `0.0.0` and no Yarn for it), so the mod compiles against the real class names with no remapping and no Gradle, which keeps the toolchain to the JDK already pinned.
- On the modded server, every mechanics recording from stage 1 replays exactly in both modes at three placements, and the stream agrees with every snapshot, tick for tick.

**Fallback not needed:** the plan allowed polling block states over the remote console if Fabric failed on 26.3. It did not fail.


### Stage 12 (2026-10-07)

**Built:** `server/` (see its README): the backend behind the page. A submission is source only; it is compiled in a child process (the page's own pipeline under Node) that is killed past the time limit or the memory limit, placed with `/place template … strict` in a free plot of a row of fixed cells, and driven by web levers that reach nothing but the probe blocks. The observer mod's stream is relayed to viewers filtered to their plot and translated to design coordinates; a clocked design's clock is driven from the stream, so each edge lands on the tick it is due. Everything since placement is a recording in the trace format, exported as a fixture and a trace, and the checks hold the live server to the same standard as everything else: the native replay of the fixture must equal the recording tick for tick. The page gained **Run on the server**: the view then shows the game's blocks. Two small changes underneath: the mod also flushes what changed *between* ticks, under the number of the tick just completed (that is when commands run, and what the trace format calls the last thing in that tick), and sends the end of every tick; and `sim_set` reports what changed at once, so the native simulator can play the part of the game in the fast checks.

**Checks:** `tools/live_test.mjs` drives the backend against a fake world under Node, eight tests in about 35 s: placed designs are the native builds at chunk-aligned origins and the stream shows them placed exactly; levers are refused until the design settles, then reach only their probe block, and bad lever messages, a viewer without the token, and the clock input write nothing; the stream reaches viewers filtered and translated; with the native simulator playing the game, the recording of the inverter and of the shift register (clock driven by the backend) equals the simulator's own trace and replays natively; a design too big for a plot is refused before anything is placed; a source over the limit, a bad module name, every unexpected field, a hostile generate loop, a compile that hangs and one that eats memory are refused, and the backend is fine afterwards; plots are limited, released when their controller leaves, cleared, and reused; compiles run one at a time behind a bounded queue. `tools/live_check.mjs` runs the real backend on the real modded server: an inverter driven through the API (two swings and a 3-tick pulse) and the counter driven through a headless browser (enable, reset, five periods) both record traces that replay natively tick for tick; the page shows the server's lamps; six counters running keep the tick time within budget.

**Results:**

- Both recordings replay natively tick for tick: the counter's 946 ticks with 16 lever changes (its clock included), the inverter's 87 ticks with 4. The between-ticks flush is what makes this exact: without it a command's changes would be labelled with the next tick.
- Tick time with six counters running at once: 3.4 ms average, 6.9 ms at the 95th percentile, against a budget of 25 and 45 ms.
- A normal compile peaks near 900 MB of memory (Yosys in WebAssembly), which set the sandbox's limit at 2 GB.
- The modded mechanics replay (stage 11's check) fell from 12 minutes to 45 seconds per mode: the stream check in the harness replayed every change since the base on every tick; it now applies them incrementally.

**Gate note (2026-10-07 review):** the last full `./check --vanilla` run (16:14 to 17:09) failed `mechanics-modded` on the hello-before-broadcast race that commit c52b837 fixed; the check passes on its own since, but the full gate has not been re-run after the fix. `./check` now writes `build/check/SUMMARY` (time, commit, each check's verdict and seconds, the run's verdict), so a passing gate is a record, not an inference from log times. Also from the review: `./check --only NAME`, `--clean`, fixed seeds for `fuzz-vanilla` (3) and `tiles-vanilla` (1), negative controls that grep for the expected message, `sim-mutations` failing on a short list, `pins-agree` extended to the recordings' jar hash, the mod's Minecraft pin, the lock files and Node, `python-lint` (ruff), and the EULA gate (`tools/setup.sh --accept-eula`).

**Try it:** `node server/live.mjs`, then open http://127.0.0.1:8766/ and click **Run on the server**.

### Repair pass (2026-10-07)

`docs/REVIEW-2026-10-07.md` reviewed the whole project and listed every finding with a suggested fix; five owners (core, playground, backend, harness and docs, new designs) fixed their areas in one afternoon and an integration step merged them. What changed:

**Core (`crates/`).** Routing capacity, which was the review's main finding: every gap between gate columns now has its own routing-column count, started from a channel-density estimate and widened only where the router reports contended tracks (up to 64 columns, with the history costs carried over); the router is a resumable PathFinder negotiation (congested nets only per round, A* with a per-net jitter, an early exit when the contended count stops falling, strength retries that continue instead of restarting, repeaters in the net's own wire tiles before crossing slots); inputs and outputs are spread to the rows of what they connect to. Measured ceiling: a 150-cell, 20-input random netlist in about 4 minutes natively (236 s, release binary), 100 cells in about 30 s, the 8-bit adder (102 cells, 17 inputs) in 10 s where it failed after 85 s before; a 200-cell, 24-input netlist did not finish in 12 minutes. The shipped layouts shrank (`adder4` 73 to 50 columns, `counter4` period 156 to 92 ticks). A lint (`lint.rs`) reads the netlist the map script writes after `proc` and `memory` and refuses, before placing and in one sentence with the source line: inout and tristate, latches, asynchronous resets, `$display`, assertions, leftover memories, unclocked registers, several, derived or data-used clocks, and more than 64 input bits; a failed Yosys script is reduced to its first error line without colour codes, and the CLI prints usage. `memory` and `setundef -init -zero` in both scripts make memories, constant arrays and initial values build, and `gold.json` lets the initial values be compared after the proof (which ignores them). Port bits are named by their declared index. `report.txt` lists lever and lamp positions, the footprint in chunks with the `/forceload` line, routing columns per gap, and for clocked designs the in-world rule in ticks and seconds. Two latent bugs of the timed model were found by the new layouts and fixed with a regression test (per-tick lamp observation, drained running set). The CLI test `crates/redfab-cli/tests/smoke.rs` builds every `tests/smoke/*.sv` (20 sources) with the real binary and asserts each file's expected messages or report lines.

**Playground.** The top module is detected (`top`, or the only module) or typed; an edited example becomes a `custom` draft kept in local storage; Ctrl/Cmd+Enter compiles; refusals are the core's one-line messages (the `lint` op with `pre.json`, `check_inits` after the proof); the status shows the phase and seconds, then the cell count against the measured capacity (`measuredCells`, 100) and whether the design fits the server's plot, which is checked before submitting; the timeout message survives the worker restart; the clock is counted in simulated ticks; lever flips recolour at once; port names are not HTML; live mode offers the structure download and gives the plot back on Stop and on leaving the page; packages are served under `/pkg/NAME@VERSION/` with immutable caching (the pins in `index.html` and `worker.js` are checked against `package.json`). The headless check covers all of it, including eight counter cycles compared with the native simulator and the download's bytes.

**Live backend and mod.** The stream reconnects once per drop with a backoff and releases every plot as stale when it comes back; RCON re-dials after a close or timeout and rejects the queue at once; a dead Java process makes `live.mjs` exit 2 and `/api/status` says `live: false` with the reason; lever changes are paced by the design's timing (one per `input_spacing` for combinational designs, at `input_change_at` after the edge for clocked ones), coalesced and bounded; the clock keeps one flip in flight and times the next from where the stream shows it landed (a late edge stretches, never shortens); the mod forgets each template after placement and the file is deleted; a plot whose clear fails is `broken` until a retry clears it; viewers are capped and dropped under backpressure; per-address limits, a compile cache, an Origin and Content-Type check, a 60 s grace for unconnected plots; plots are 512 by 512 so every shipped design fits; `GET /api/fits` and `GET /api/plots/N/structure`. The mod's writer threads end with their client and it logs through slf4j. `live_test` grew from 8 to 24 tests. Found by the integration run of `live_check`: the vanilla server runs every RCON connection's commands through one shared console buffer (`DedicatedServer.runCommand`, no lock), so the backend's second connection for the clock got a lever's reply glued to a placement's and judged the placement failed, and the failure path then crashed the backend on an unawaited waiter; there is one RCON connection now, clock commands jump its queue, and a refused placement is an error answer, not an exit.

**Harness, gate and docs.** A root `README.md` and `LICENSE` (MIT, in every manifest); the EULA is accepted explicitly (`tools/setup.sh --accept-eula` writes `.tools/eula-accepted`; nothing starts a server without it); the JDK is pinned for four platforms and reached through `.tools/java`; negative controls assert their message; `sim-mutations` fails on a short list; the vanilla seeds are fixed; `pins-agree` covers the recordings' jar hash, the mod's pin, the lock files and Node; `./check --only NAME`, `--clean`, per-check seconds and `build/check/SUMMARY`; a racy replay fails; `ruff` in the gate; `harness.show` takes a path and prints the lever positions report.txt prints; the Verilator cycle testbench drives the design's active edge (a falling-edge design passed before only by coincidence); port bits are named by the declared index as the core names them. After the re-verification: `docs-agree` (`harness/docs_check.py`) holds README.md, LICENSE and docs/ to the tree (the tool versions against `versions.env` and `.python-version`, the refusal table against `tests/smoke/*.sv`, the design list against `designs/`, the paths, the limits against `measuredCells`, `MAX_INPUTS` and `plotSize`, the burnout budget against `BURNOUT_BUDGET`, the fixture counts against `fixtures/mechanics`); `negatives-can-fail` proves the negative-control helper fails on a crash without the message; `js-syntax` parses every JavaScript file; Python is pinned (`.python-version`, asserted by `pins-agree`); the Fabric libraries tree is digested (`FABRIC_LIBRARIES_SHA256`); `harness/tests/test_check.py` runs the script itself (`--only`, an unknown name, the SUMMARY format, with `REDFAB_CHECK_SUMMARY` pointing elsewhere); the fuzz run names the placement-dependent arrangements it drops; `harness.tiles` runs the two redstone modes on two servers at once (`--serial` for the old order; measured with the tile variants alone: both modes in 1042 s together where one mode alone took 842 s, so the check should take about 19 minutes instead of 29, to be read off the next `./check --vanilla`); Verilator builds in a temporary directory when the build directory's path has a space (GNU Make refuses such a directory) and reports its own error; the stray-server `pkill` passes its pattern after `--` (BSD pkill read `-Dredfab...` as options and killed nothing). The `lever_mount` fixture records a powered lever strongly powering its mounting block and its removal updating the neighbours. Thirteen designs were promoted into `designs/` and the gate: `mul2 mux4 prio3 cmp4 param_add signed_sub divmod hier rom4` (combinational, 1,058 vectors against Verilator) and `lfsr3 negedge init_reg mem2` (clocked, 64 cycles each); `cycles-vanilla` also runs `negedge` now, the one in-game exercise of a falling-edge clock (not yet run in the game at the time of writing).

**Second round (2026-10-08).** A strict re-verification of the pass listed what was still open, mostly behaviour that was right but unguarded, and four owners plus an integration step closed it. Core: every router claim now has a test (a congested placement stops within a few stall rounds with `Congested` naming the narrow gap and a seeded history steers nets off its tracks; the build loop widens exactly the gaps the log names; `designs/adder8` is built by the gate and must need a gap wider than six columns; `pnr-random` runs clocked, 16-input and one 100-cell netlist; the CLI finds `synth/yosys.mjs` beside the binary or through `REDFAB_SYNTH`, tested with a copied binary); the simulator updates the mounting block's neighbours when a powered lever is removed (`LeverBlock.onRemove`), so `lever_mount` replays, and a trace replay now also requires a settled, update-placed recording to be a fixed point of the simulator's rules, which is what lets the new `lever-powers-nothing` mutation (15 now) be caught; the in-world rule of report.txt has a unit test with its seconds, and `tests/smoke` gained a zero-input source and the `/forceload` line. Playground: the headless clock assertions read the design's own period and compare four free-running cycles with the native simulator; Pause and Run inside a pulse keep the level and no hand-driven step is dropped; a design over 400 cells is refused right after synthesis and the page states the measured size before compiling; `report.txt` and `design.json` download beside the `.nbt`; a Content-Security-Policy with hashes computed from the served page (and on every API answer of the live backend); on a clocked design a click waits for the cycle's input window, as the server does, and the check holds a mid-cycle click against the native run. Backend: the server configuration is one file (`harness/data/server_profile.json`) that both the harness and the backend read, with their differences explicit and tested; a lever flood on a clocked design costs one command per input window; `--host` and `--trust-proxy` with X-Forwarded-For tested; the Origin check, the no-query-string log rule, the pid-file kill, the child's environment allow-list and the close-reason cut all have tests; the mod's writer threads are held to zero by `mod/test/WebSocketServerTest.java`, run by `tools/build_mod.sh`. Harness and docs: `docs-agree`, `negatives-can-fail`, `js-syntax`, the Python and Fabric pins, `test_check.py` on the script itself, the sampled `sim` check (`adder8` is in it with a fixed 256-vector sample), the two-server `tiles` run, the `redfab` wrapper in one place (`harness/server.py`) and `gen_block_defaults.py` reading `versions.env` through `read_versions`. Measured for the record: a 150-cell, 20-input random netlist routes in 236 s with the release binary, not the "2 to 3 minutes" the first round wrote down.

**Not done, and why:** rows do not grow when routing fails (no measured netlist needed it); 200-cell netlists (the time goes into PathFinder rounds on 10k-tile grids and the 67 s layout check and timing on 150k-block worlds, the next things to speed up, then the four-rows-per-gate density); `$abc$` gate names stay (flip-flops are labelled by their register bit); `./check --vanilla` has not been re-run since the pass (owed before stage 13; single vanilla checks were run by hand: `mechanics-negative`, `tiles verify --groups 0`, the `lever_mount` recording); the core's log lines are not streamed to the page while it routes (the wasm reply carries them whole); the plot token still travels in the URL (TLS at the proxy is required and written down); the forceloaded chunks of a released plot stay loaded; Litematica has not been tried; no Linux machine has run `tools/setup.sh`. On the question of a CPU in Verilog: the subset is there (memories, hierarchy, parameters, signed arithmetic, division, one clock on either edge, initial values) and the gate pins it; what is missing is capacity, about 150 cells today against the several hundred a tiny CPU needs, so the next work is router speed and placement density, in that order.

**Gate:** `./check` (41 checks, 474 s on 2026-10-08) passes; `node tools/live_check.mjs` passes on the real modded server (tick time P95 10.5 ms with 6 counters, budget 45); the headless playground check passed three times in a row; `harness.mechanics verify --modded` passed on the real server before this round.

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
- The player-facing server policy, none of which the backend decides today: the game port stays on 127.0.0.1 and the backend's server runs with `online-mode=false` (the harness profile's one difference, `server/README.md`), so opening the game to players means a whitelist or `online-mode=true`, spectator or adventure mode for visitors, and spawn protection off (plots are placed by command, players must not edit them). The web side needs TLS terminated at the reverse proxy (`--host`, `--trust-proxy`), because the plot token travels in the URL.

### Later
`.litematic` and `.schem` export (Litematica should load the `.nbt` files as they are, since it pastes with updates suppressed; untested, as no Minecraft client has run in this project), an on-board clock generator, the 8-bit ALU with speed benchmarks, and the register-file/ROM generators and tiny CPU as a showcase.

## What gets installed

As built (`tools/setup.sh --accept-eula`; the plan-time table listed Playwright and Gradle, neither of which is used):

| What | Where | Why |
|---|---|---|
| Rust via rustup, plus the WebAssembly target | Your home folder (`~/.cargo`, `~/.rustup`) | Compiler core |
| Verilator | Homebrew (or apt); the version is asserted by `./check` | Reference simulator for Verilog |
| JDK 25 and the official Minecraft 26.3 server jar | `.tools/` inside the project; `.tools/java` links to the JDK's java binary | Minecraft 26.x needs Java 25; you only have Java 21 |
| Fabric Loader and Mixin | `.tools/fabric`, installed by running the pinned Fabric launcher once (Stage 11) | Server mod; it compiles with `javac`, no Gradle |
| YoWASP Yosys, Python test libraries | A project-local Python environment (via `uv`) | Synthesis and the harness |
| Node packages (YoWASP Yosys, three.js, puppeteer-core) | `node_modules/` (Stage 10) | Synthesis runs under Node; the playground and its tests |
| Your EULA acceptance | `.tools/eula-accepted` | No server starts without it |

Not installed by setup, needed before it: `uv`, Node 26, and a Chromium (Brave, Chrome or Chromium) that puppeteer-core drives for the two browser checks.

Your disk had 28 GB free (94% full) at the start. Disk use at stage 12: `target/` 3.7 GB, `build/` 1.3 GB (mostly server run directories; `./check --clean` removes them), `.tools/` 550 MB.

## Where things live

- `README.md`: the user's entry point (setup, build, the supported subset, the in-world recipe)
- `crates/redfab-core/`: blocks, world, extractor, simulator, tiles, placement, routing, timing, export (flat `src/*.rs` files, not the module directories DESIGN.md §7 sketched)
- `crates/redfab-cli/`: the `redfab` command; `crates/redfab-wasm/`: the core for the browser
- `cells/`: tile templates as text (`*.tile`), with their descriptions and the rules in `cells/README.md`
- `synth/`: the Yosys runner under Node; the scripts themselves live in the core (`synth.rs`)
- `harness/`: Python server harness and experiments; `harness/tests/`: its unit tests
- `fixtures/`: committed game recordings (`mechanics/`, `fuzz/`, `fuzz-repeaters/`)
- `designs/`: `not1`, `nor2`, `nor3`, `consts`, `fa1`, `adder4`, `adder8`, `mul2`, `mux4`, `prio3`, `cmp4`, `param_add`, `signed_sub`, `divmod`, `hier`, `rom4` and the clocked `counter4`, `fsm`, `shift2`, `lfsr3`, `negedge`, `init_reg`, `mem2`
- `tests/smoke/`: one source per refusal and per report feature, built by `crates/redfab-cli/tests/smoke.rs` with the expected lines in each file's header
- `tests/`: smoke sources and data for stage 0 and the CLI test; `tools/`: setup and the check drivers
- `playground/` (Stage 10), `mod/` (Stage 11), `server/` (Stage 12)
- `docs/`: `DESIGN.md`, `PLAN.md`, `trace-format.md`, `mechanics.md`, the dated reviews

## How each stage is verified

- `./check` after every change; `./check --vanilla` before I call a stage done.
- At each stop I report: which checks passed, anything the game did that I didn't expect, disk use, and what the next stage will do.
- Your in-game looks at Stages 3, 5 and 9 are optional; the checks don't depend on them.
