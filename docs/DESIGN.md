# redfab: design review and architecture

This is a review of the redfab plan (a SystemVerilog to vanilla-redstone compiler) written before any code exists.
Date: 2026-10-01. Tool facts in §9 and redstone facts in Appendix A were checked against upstream sources on that date. Everything else is my opinion, and I've tried to label it that way.

The staged build plan and its status are in `PLAN.md`. Decisions made after this review are listed next; where they conflict with the text below, they win.

## Changes since the review

| Topic | The review said | What we do now | Why |
|---|---|---|---|
| Goal | A compiler that writes a schematic file | An online playground: write Verilog in a browser, watch it run on a real Minecraft server (§11) | Your stated end goal |
| Language | Rust recommended | **Rust**, confirmed | Decision on 2026-10-01 |
| Crates | One crate | Two: `redfab-core` (pure computation, also builds to WebAssembly) and `redfab-cli` (files and processes) | The core has to run unchanged in the browser |
| Yosys | Native Yosys 0.69 from OSS CAD Suite | **YoWASP Yosys 0.68**, the WebAssembly build, from PyPI natively and npm in the browser | One synthesizer everywhere, so your machine and the browser produce the same netlist. 0.68 is the newest release published on both PyPI and npm |
| `read_slang` | Works out of the box | Needs **`--threads 1`** | Measured in stage 0: the WebAssembly build can't create threads, and `read_slang` aborts with "thread constructor failed" without it |
| Equivalence | EQY | Yosys's built-in equivalence commands | EQY needs a native Yosys; it isn't part of YoWASP |
| Verilator | From OSS CAD Suite | Homebrew (5.052) | OSS CAD Suite is no longer needed |
| Milestones | §8 "Revised milestones" | Stages 0–13 in `PLAN.md` | Each stage ends in checks runnable from a terminal |
| Export order | `.nbt`, `.litematic`, `.schem` | `.nbt` from stage 2; the other two after the playground | Litematica loads `.nbt`, and the playground doesn't need the others |
| Simulator | One simulator over a component graph, fed by the netlist or by extracted blocks (§3, §6) | **A block-level simulator that follows the game's own rules** (`redfab-core/src/sim`). Netlist-level simulation, when needed, is Verilator on the mapped netlist with generated cell models | Only a block-level model can be compared with the game tick for tick. It matches 417 recorded traces exactly (stage 2) |
| Wire power rule | Not decided | The game's **Redstone Experiments** evaluator: a wire network settles before any neighbour is notified | The default mode's intermediate dust states depend on block position. Circuits must behave the same in both modes, and the harness checks both |
| Races | "Order fuzzing" as a test (§7) | Built into the simulator: tick order and update-delivery order can be shuffled, and a circuit whose result changes has a race | Stage 2 found the game itself is order-dependent on about 1–2% of random circuits, by position in default mode and by a random choice in experiments mode |
| Extractor / circuit graph | Stage 2 | Stage 3, with the layout-versus-netlist check that uses it | The simulator no longer needs it |
| Where Yosys runs | The PyPI build natively, the npm build in the browser | **The npm build everywhere**, under Node on your machine (`synth/yosys.mjs`) | Stage 3 found that technology mapping (ABC) silently kills the PyPI build under its runtime. The npm build is the one the browser uses anyway |
| Cell library | NOT, NOR, OR, AND, BUF, DFF | `NOT`, `NOR2`, `NOR3` (one NOR tile with one to three inputs) plus `BUF` | ABC refuses a library without a buffer. `BUF` is priced so it is never preferred, and a buffer in a netlist becomes plain wire |
| Tile grid | "Pick P and H after building NOR3, crossing and DFF" | **5 × 5 blocks, 4 high**, glass floor, ports at edge midpoints at y = 1, gates take inputs on W/N/S and drive E (`cells/README.md`). Fixed in stage 5 | 5 is the smallest pitch at which one wire can bridge another inside a tile; the crossing tile exists and works in the game |
| How the layout is read back | A rule-based extractor | **By asking the simulator**: raise one net, or switch one torch or input on, and see what it powers (`extract.rs`) | The extractor then cannot disagree with the rules that were checked against the game |
| Driving inputs | Levers | The lever sits on a stone block; tests and the playground swap that block for a redstone block | A command that changes a lever's state powers nothing (stage 1) |
| Repeater tile | A `Repeater(from, to, delay)` tile | **No repeater tile: any wire tile, and the flat arm of a crossing, can carry a repeater in the arm the signal enters by** (`cells/README.md`). The build turns a crossing a quarter when the climbing signal is the one that needs the repeater | Stage 6: with repeaters only in straight tiles, a signal that turns or crosses often died with nowhere to put one. Crossings in a row (an input running down the first routing column) were the case that could not be fixed any other way |
| Placement rows | Three rows per gate (north feed, gate, south feed) | **Four rows per gate**, and at least as many free rows in a gate column as there are nets that must pass it (nets driven before the column with a sink after it); since the compaction stage a second, packed style gives a gate only the rows its pins use and the build keeps whichever measures smaller (§4.3, point 6) | Stage 6: a net can only pass a gate column through a row holding neither a gate nor a feed, so columns of 3-input gates were walls, and random netlists with many outputs did not route |
| Routing strength | "Repeater insertion from exact wire lengths" (§4.4) | Exact per-tile strengths: a gate drives 14 into the next tile, an input or repeater 12, a tie 13; every tile hop costs 5; a gate pin needs 2 at its port, a lamp 3, a repeater 1. Repeaters go at the last tile before the signal would fall short. When a path dies inside a run of crossings, the crossings get a penalty and the net is re-routed (up to 30 times) | Measured on the tiles; the layout check (`weak_signals`) confirms every pin in the baked world |
| LVS and identical gates | Match each cell to a torch "powered by exactly its input nets" | Cells with the same input nets are matched **as a group**, and their output nets become one class | Stage 6: random netlists contain `NOT(a)` beside `NOR2(a, a)`; the torches are interchangeable, and picking the wrong one derailed every match downstream |
| Reading the game's state | `save-all` then parse the region files | The same, but **every chunk read must have a record that decodes, names that chunk, and carries the current game time as its `LastUpdate`**, with the read repeated until it does | Stage 6: `save-all flush` replies before the files are fully written and the game moves records inside them; a large design showed chunks read empty or out of place for a tick |
| Timing | Graph min/max arrival over the routed design (§4.5) | **Both** a min/max arrival by path (the safe bound, used as the settle time) and **exact figures from a timed model** of the extracted parts run over every ordered pair of input combinations (up to ten inputs) | Stage 7: the 4-bit adder has a false path; by path `cout` is 144 ticks, exactly it is 140. The model uses the block simulator's part rules and is checked against it change by change |
| Flip-flop | "Build two candidates in-game: master/slave repeater-lock, and a pulse-unlock; pick the one with the larger hold margin" (§10.5) | **Both built and swept tick by tick** (stage 8). The master/slave pair is `dff`: setup 1, hold 5, clock-to-output 8, period ≥ 12, acting on the falling edge; the pulse latch samples late (hold 11) and needs period 16. The tile is **two cells wide** | A side-locked repeater's lock input lands on the border of a one-cell tile. The "zero-margin" same-tick coincidence the review feared is harmless: a locked repeater ignores a pending tick, so the lock wins whichever is processed first |
| Clock polarity | Rising-edge flip-flops | The tile acts on the falling edge; `dfflegalize` adds one inverter on the clock, so a design's `posedge clk` is the lever's rising edge | Stage 9: one inverter at the clock root costs 2 ticks and keeps the Verilog semantics |
| Sequential timing | Setup, hold, per-DFF clock arrival, fix hold by raising repeater delays (§4.5) | **As planned**, on the circuit read back from the blocks with flip-flops as parts; plus the window in which inputs may change and when outputs are read, which the harness and the playground drive by | Stage 9: the counter needs a period of 132 ticks; hold slack is large because the feedback paths are long |
| Burnout budget | A torch-burnout budget check (stage 9) | **The fewest ticks between input changes** at which no torch turns off more than six times in sixty, found by driving the design; clocked designs are checked at their period | A design at a legal input rate cannot be made to burn out through a glitch in NOR logic; only driving it too fast does |
| Playground | A browser code editor and a 3D view (§11) | **As planned, with no server** (stage 10): YoWASP Yosys and the WebAssembly core run in a worker; the view is driven by the core's own simulator; limits on source size, cells, blocks and compile time, enforced by terminating the worker | The pipeline under Node gives byte-identical designs to the native build, so the browser and the command line are one compiler |
| Server mod | A small Fabric mod that only observes (§11) | **As planned** (stage 11): two Mixins and a built-in WebSocket server, no Fabric API; compiled with `javac` against the unobfuscated 26.3 server jar | 26.3 ships without obfuscation, so no mappings, remapping or Gradle are needed |
| Live backend | "The server recompiles from source inside a WebAssembly runtime with CPU and memory limits" (§11) | **Node, with a child process per compile** (stage 12): the page's own pipeline (YoWASP Yosys and the WebAssembly core) under Node, killed past the time and memory limits by the parent; one compile at a time behind a bounded queue | The pipeline under Node was already proven byte-identical to the native build in stage 10; a process boundary is what limits memory (WebAssembly memory sits outside V8's heap limits) and contains a crash |
| Live clock | Web levers reach probe blocks (§11) | **The backend drives a clocked design's clock from the mod's stream**: an edge due at tick T goes out when tick T's message arrives, so the command lands between T and T+1 | The mod sends the end of every tick, and what changes between ticks is flushed under the number of the tick just completed, so a command's effects are labelled exactly as the trace format counts them |
| Live correctness | The stream in the trace format, so sources can be compared (§11) | **The recording of each plot is a fixture and a trace**; the checks replay the fixture natively and require equality tick for tick | The same standard as every other stage: the live server is checked against the simulator the game was checked against |
| Routing columns | "Estimate k_i from the max cut between levels and grow it if routing fails" (§4.3); one count for every gap, up to 6 (stages 6 to 12) | **Each gap between gate columns has its own count**, started from a channel-density estimate of that gap (overlapping row changes; a feedback net is charged to the gap before its sink) and widened only where the router reports contended tracks, up to 64 columns and 40 attempts, with the history costs carried over (`place.rs`, `build.rs`) | The 2026-10-07 review: the router gave up at 6 columns on an 8-bit adder (102 cells) after 85 s, and the time went into attempts that restarted from nothing. Now the adder builds in 10 s, 100 cells with 16 inputs in about 30 s, and a 150-cell, 20-input random netlist in about 4 minutes (236 s with the release binary, `pnr-random --gates 150 --inputs 20 --seed 1`) |
| PathFinder | A* per net with history and present-congestion costs, rip-up and re-route over iterations (§4.4) | **As planned, resumable**: a negotiation that re-routes only the congested and dirty nets each round, A* with a deterministic per-net jitter, an early exit after 12 rounds without the contended-track count falling, a report of which nets want which tracks; strength retries continue the same negotiation (present factor softened, penalties growing) rather than restarting, and a repeater goes in a plain wire tile of the net's own before a crossing slot (`route.rs`) | Measured in the repair pass: greedy use of crossing slots for repeaters capped random netlists at about 100 cells; restarting the negotiation threw its history away |
| Input and output rows | Inputs, constants and outputs packed at the top of their columns | **Spread to the rows of what they connect to** (`place.rs` `spread()`) | The row swaps in front of the outputs were the one congestion the widened gaps did not remove |
| Lint list | "Treat this as your lint list in `synth/`" (§9) | **`crates/redfab-core/src/lint.rs`**, run on the netlist the map script writes right after `proc` and `memory` (`pre.json`), before placing: inout and tristate, latches (and an `always_comb` self-loop, which abc would turn into a loop), asynchronous resets, `$display`, assertions, leftover memories, registers without a clock, several clocks, derived clocks, a clock used as data, more than 64 input bits. Each refusal is one sentence with the source line; a Yosys failure is reduced to its first error line without colour codes. The same checks run in the browser and in the live backend through the wasm `lint` op | Before, these surfaced as raw Yosys text, as a failed proof or as a layout problem after a full place and route |
| Synthesis scripts | `proc; flatten; opt; techmap; abc -liberty; dfflibmap` (§3) | **`memory` after `proc`** (memories and constant arrays become flip-flops and muxes), **`setundef -init -zero`** (every register starts at 0 unless the source says otherwise, so `q <= 1'b1` stays a flip-flop), `pre.json` for the lint, and `gold.json` from the proof script so the flip-flop initial values, which the equivalence commands ignore, are compared bit by bit (`lint::check_inits`) | A 2x2 memory, a ROM and a 2x4 register file were refused in the proof (`Gold module contains memories`); an undefined initial value was optimised as don't-care, which the proof could not see |
| Timed model | Exact figures from a timed model of the parts (stage 7) | **Two rules fixed in the repair pass**: a lamp change is observed once per tick (a lamp that goes dark and relights within one tick is no change to a tick-by-tick observer), and a part told of a change in the tick it already ran in takes a new tick, as in the game and the simulator (`timing.rs` `Model::apply`) | The new, smaller layouts of `fa1` and `adder4` made the model disagree with the block simulator; a regression test runs the full adder over 256 random multi-bit changes |
| Tile contract check | "Prove it in the harness exhaustively once per library change" (§4.2) | **Static, from the blocks**: every pair of tile variants side by side and random 2×2 groups are read back with the extractor, and any connection between tiles other than through matched ports is an error (`tiletest.rs`). The game runs every variant's fixture and random groups as well | The extractor uses the simulator's rules, which the game checked; the static check runs in seconds for all 3,442 groups |

---

## 0. Verdict

**The front half of the plan is sound. The back half is naive.**

The front half is Yosys + yosys-slang, then a small cell library, then levelized placement. Keep it. (yosys-slang is now sv-elab and is built into Yosys; see §9.)

The back half is the problem. It routes freely at block level, polices the result with a hand-written rule-list DRC, and verifies it with a *netlist* simulator. The project will stall there, for one structural reason: **redstone has no insulation.** Every block talks to its neighbours through a dozen rules: dust auto-connects, dust steps diagonally up and down, weak and strong power behave differently, repeaters lock each other from the side, and so on. So the main way this fails is "the blocks don't implement the netlist I think they do", and nothing in the current plan ever looks at the blocks.

Six changes, in priority order:

1. **Use correct-by-construction tiles instead of DRC.** Lay the design out on a coarse grid of hand-designed, exhaustively verified tiles: gate, wire, turn, fanout, crossing, repeater, I/O. Design the tile borders so that no two legal neighbours can interact. DRC then becomes "every tile is legal and every port is matched". That check is decidable and fits in about 50 lines.
2. **Verify the blocks, not the netlist.** Write an *extractor* that turns a block world into a component graph. Use it for LVS (the extracted graph must be isomorphic to the mapped netlist), and simulate the *extracted* circuit against Verilator. Your planned netlist simulator becomes the same simulation engine with a different input.
3. **Make vanilla Minecraft the oracle.** A headless vanilla server driven over RCON, stepped with `/tick`, gives fully automated ground truth with no mods. MCHPRS is a fast playground, not a reference. It's still on MC 1.20.4, and its redpiler doesn't implement torch burnout.
4. **Route with PathFinder on the tile grid, and use crossing tiles instead of a general via.** The left-edge algorithm assumes two-layer HV routing with cheap vias. Redstone vias are large, behave differently going up and going down, and cost both delay and signal strength.
5. **Sequential timing is about hold, skew and pulse width, not just fmax.** Expect your first counter to fail by racing through a DFF, not by being too slow.
6. **Write the whole core in Rust, and don't use nextpnr.** §5 has the reasoning.

**You also aren't first.** There have been about 20 HDL→redstone attempts, and most died in routing. One of them, MinecraftHDL (2016), is almost exactly your current plan: levelized columns plus left-edge routing. It never got sequential logic working. Two tools from Aug–Sep 2026, **REDA** and **RTL2MC**, independently arrived at most of the changes above: NOR-only cells, hard invariants, and conformance runs on a real server. Read both before writing code (Appendix B).

---

## 1. What will break first

Ordered by when you'll hit it.

| # | Failure | When | Why the current plan doesn't catch it |
|---|---|---|---|
| 1 | **The pasted circuit isn't the intended circuit.** Examples: dust auto-connects to a neighbouring net; a lever's mounting block powers dust next to it; one net's repeater side-locks another net's repeater; dust steps diagonally onto a wire one level up. | Milestone 1 (adder) | The DRC is a hand-written rule list, and its "etc." *is* the problem. Nothing reads the generated blocks back. |
| 2 | **The pasted state is inconsistent.** Torches are stored `lit=true` on powered blocks, or dust is stored `power=0` next to a source. Nothing re-evaluates a block until an update reaches it, so the circuit sits in a wrong state. Latches come up with arbitrary values. | First paste | Export writes default block states. There is no reset or initial-state story. |
| 3 | **The counter races.** The Q → inverter → D path is shorter than the DFF's transparent window plus clock skew, so a bit toggles twice or oscillates while the clock is high. | Milestone 2 | STA only computes max delay. There's no hold, skew or pulse-width check, and the DFF is never characterized. |
| 4 | **The router fails on the first real netlist.** Nets that span several columns need feedthroughs, which LEA doesn't create. Cyclic vertical constraints need doglegs. In the HV model every trunk-to-branch junction is a via, and a redstone via is several blocks long and changes the signal's timing and strength. | 4-bit adder → ALU | LEA plus a "hand-designed via cell" assumes VLSI economics. |
| 5 | **Torches burn out from glitches, not from the clock.** A torch deep in a ripple-carry chain can turn off several times in one cycle while the logic settles. The vanilla limit is 8 turn-offs in 60 gt. At a 1 Hz clock, a torch can afford about **one** turn-off per cycle. A burned-out torch stays dark until something sends it a block update, so the circuit breaks silently and stays broken. | ALU | Burnout is treated as a static bound on the clock period. |
| 6 | **Version drift.** Today Minecraft is 26.3, MCHPRS is on 1.20.4, litemapy writes the 1.18.2 data version, and Litematica writes format v7 while litemapy writes v6. Block states and Yosys flags change too (Yosys v0.69 removed `abc -fast`). | Any time | Not mentioned in the plan. |

### Notes on each

**(1) Accidental connectivity.** Your DRC lists "no adjacent dust from different nets" and "no strength reaching 0, etc." Here is a partial list of other interactions that also create unintended connections or logic:

- Diagonal step-up/step-down connections between dust at different Y levels.
- Dust powering the block it sits on, and the block it points into. That block then powers adjacent repeaters and torches of *other* nets.
- A torch strongly powering the block above it.
- A repeater strongly powering the block in front of it. That block then powers *all* adjacent dust.
- A lever strongly powering the block it's mounted on.
- A powered repeater locking any repeater it faces sideways into.
- An unconnected dust rendering as a "+" and pointing into all four neighbours (Appendix A).

You won't enumerate all of these correctly by hand, and a rule-list DRC fails silently when a rule is missing. The fix has two parts:

- **(a) Make the problem small.** Use tiles with an isolation contract, so the only cross-tile interaction happens at designed ports.
- **(b) Make the check generic.** Extract the actual connectivity from the blocks and diff it against intent. In redstone, **DRC and LVS collapse into one check**, because almost every "design rule violation" shows up as a connectivity difference.

**(2) Initial state.** Pasting doesn't re-run physics on every block. I checked the Litematica source: single-player paste and Servux paste place blocks **with neighbour and shape updates suppressed**, and command paste uses `setblock … strict`. There's no option to turn updates on. Vanilla `/place template` *does* update by default, unless you pass `strict` (Appendix A). Two fixes:

- Bake a **consistent quiescent state** into the export: simulate to a fixed point with all inputs at 0 and every DFF at its `init` value, then write `lit`, `power`, `powered` and `locked` from that state.
- Provide an explicit synchronous reset input.

A side benefit: this is how you get `initial`/`init` register values for free. The harness should place each design **both ways**. `strict` mimics Litematica and tests that the baked state is self-consistent. Non-strict tests that the circuit converges to the right state anyway.

**(3) Sequential timing.** Redstone delays are large, integer, and comparable to clock pulse widths. Every redstone DFF has an internal race:

- **Master/slave with an inverted clock**, for example the wiki's standard rising-edge DFF ("design H": four repeaters and a torch). The inverter puts 2 gt of skew between the master and slave enables. By my reading of the tick rules, on the **falling** clock edge both latches are transparent for 2 gt, and the master's output change lands on the *same game tick* the slave locks. It probably holds only because a locked repeater ignores its pending ticks. That's a zero-margin design that depends on tick-ordering rules, and the harness must confirm it before you trust it (Appendix A).
- **Pulse-latch:** the clock edge makes a short pulse that briefly unlocks one latch, so any path from Q back to D that is shorter than the pulse width races through.

On top of that, a clock spine needs a repeater at least every 15 blocks, and each one adds at least 1 rt. That gives clock skew along the register column. A 4-bit counter is the first design with a short Q→D path, so it is the first thing that breaks. You need:

- A characterized DFF: setup, hold and clk→q, measured by sweeping the data arrival time in 1-game-tick steps in vanilla.
- A per-DFF clock arrival time.
- Min-delay (hold) STA.

**(4) Routing.** See §3 and §4.4.

**(5) Burnout.** The rule is "more than 8 turn-offs in 60 gt". The decompiled check may be ≥ 8, so budget **≤ 7 turn-offs in any 61-gt window** (derived before stage 1 measured it; the budget in the code is 6, see *As built* below). With clock period P gt and g turn-offs per cycle, you need g·⌈61/P⌉ ≤ 7:

- At 1 Hz (P = 20), g ≤ 1.
- At 0.5 Hz, g ≤ 3.

Count turn-offs per torch in sliding windows during random-stimulus simulation, and fail the build if any torch exceeds the budget. A static bound (turn-offs per cycle ≤ the number of distinct arrival times at that torch) is a fine early warning, but it isn't the check. Neither MCHPRS nor redpiler implements burnout, so this check has to be yours.

*As built:* stage 1 measured burnout on the **8th** turn-off within 60 gt, so the game allows 7, and the budget in the code (`BURNOUT_BUDGET`, stage 9) is **6**, one to spare; the check is the input-spacing search described in "Changes since the review".

---

## 2. What you're underestimating

- **The tile library is the product, and probably ~40% of the total effort.** Each tile needs four things: a block template, a port contract, min/max delay per arc, and a truth table verified in vanilla. On top of that, every *pair* of legal neighbours must be shown not to interact. With about 15 tiles this is only tractable once the vanilla harness exists. **Build the harness before the compiler.**
- **The DFF is a research project of its own.** The wiki's standard master/slave DFF looks like a zero-margin design on paper (§1, note 3). Build two candidates in-game: master/slave repeater-lock, and a pulse-unlocked single latch. Sweep setup and hold in 1-gt steps, then write down setup, hold, clk→q and minimum clock pulse width as integers in game ticks. If a design passes only because of same-tick ordering, add 1 rt to its slave-lock path and pay the delay. Don't let the router or placer touch its internals: it's a hard tile.
- **Clocking.** Restrict to one global clock, posedge only, no gating, no derived clocks. Use a **lever as the clock** for the early milestones, which removes all clock-generation risk. Later, use an enable-gated torch/repeater ring oscillator whose period comes out of STA. For distribution, a repeater's delay can be set to 1–4 rt *at the same footprint*, which gives you a free knob for both clock-skew balancing and hold fixing. Use it.
- **Vertical signal movement is expensive and asymmetric.** Going up and going down need different structures (Appendix A), each costs footprint, strength and sometimes delay, and a torch tower inverts at every stage. That's why I recommend a single logic plane plus crossing tiles for v1, rather than multi-layer routing.
- **Debug tooling.** You'll spend more time looking at layouts than writing algorithms. Two tools pay for themselves within a week:
  - A per-Y-layer SVG/HTML viewer that colours blocks by net.
  - An extractor query that answers "why is this block powered / which nets touch this block".

  Debugging in-game is the last resort.
- **Speed expectations.** A torch delay is 1 rt (100 ms). An 8-bit NOR-logic ripple adder is roughly 20–30 rt before routing repeaters, and a tiny CPU will clock somewhere around 0.1–0.5 Hz. That's fine for the game. It does mean block-level random-stimulus testing needs a *fast* simulator, which feeds into the language choice.
- **Scale effects for the CPU milestone.** Everything has to stay within simulation distance of the player, a large paste is slow on servers, and a full-size CPU is big enough that you'll want stacked planes. That's a density concern, so leave it until after the ALU.

---

## 3. Point-by-point on your plan

| Your plan | Verdict | Change |
|---|---|---|
| Yosys + yosys-slang (`read_slang`) | **Keep** | yosys-slang is now **sv-elab**, built into Yosys ≥ 0.67, so you don't need a plugin. Pin one Yosys version (§9). Define and enforce an SV subset: single clock, posedge, **synchronous reset only**, no latches, no tristate, no multiple drivers. Reject anything outside it with a clear error. `dfflegalize` to `$_DFF_P_` will emulate sync reset in logic, but it cannot emulate async reset. Treat that as a feature. |
| `abc -liberty` + `dfflibmap` | **Keep, with a better cell list** | (a) **Generate the .lib from the tile library**, so the timing and area ABC sees are exactly what you'll build. (b) Your cell list is shaped like CMOS, not redstone. The native redstone gate is **NOR**: dust from up to three sides powers one block, and a torch inverts it. That gives NOR1 (NOT), NOR2 and NOR3 at 1 rt each, with inputs naturally isolated. **AND is not a primitive.** It's a NOR of inverted inputs (2 rt, three torches), and an AND cell only hides that from ABC. OR is either a NOR followed by a NOT (2 rt) or a merge of inputs through repeaters acting as diodes (1 rt); offer both and let ABC choose by delay and area. (c) **BUF/repeater is not a synthesis cell.** It's a physical element that routing and timing insert. (d) Add **TIEHI/TIELO** with `hilomap` and run `setundef -zero`, because your first counter will have constant nets. |
| Formal equivalence with `equiv_*` | **Keep, but it proves the less risky half** | `equiv_make`/`equiv_simple` match by structure and names. They struggle once ABC has restructured the logic and FSM recoding or register merging has renamed the state. Use **EQY**, or a `miter -equiv` plus `sat -tempinduct`, and disable FSM recoding while you bring the flow up. More importantly, add the check that is actually missing: **LVS of the layout against the mapped netlist.** |
| Tick-accurate netlist simulator vs Icarus/Verilator | **Re-target it** | Synthesis is already proved formally, so simulating the netlist mostly re-checks that. Build **one** simulator over a *component graph* IR, with two front ends: (a) netlist + cell timing with ideal wires, and (b) the world extracted from real blocks. The golden model is **Verilator**: Icarus's SV support is narrower than slang's, and you'll write SV that Icarus can't read. |
| Levelized columns + barycenter | **Keep. It's correct and debuggable.** | This is Sugiyama layered drawing; finish the method. (a) Insert dummy **feedthrough** nodes for any edge that spans more than one level; LEA won't do this for you. (b) Treat DFF D-input nets as back edges and route them in a dedicated return region. (c) Do several barycenter sweeps in both directions. Simulated annealing on the tile grid can come later, when area matters. |
| All DFFs in one register column | **Fine for v1** | Model the clock skew along the column explicitly (above). Later, allow a register column per pipeline stage. |
| Channel routing (LEA), Y-levels as layers, hand-designed via | **Replace** | Use negotiated-congestion maze routing (**PathFinder**) on the tile grid, with one logic plane and **crossing tiles**, the model that field-coupled-nanotech tools use (§5). PathFinder handles multi-column nets, back edges, strength cost and rip-up without special cases. If routing fails, add a routing column and retry; that loop always terminates. Keep stacked planes as a later density upgrade. |
| Repeater insertion every ≤15 blocks | **Keep the rule, but move it into routing** | Tile path lengths are fixed, so the strength along a route is exact. Insert repeater tiles where the 15-block budget runs out. Then let STA use repeater delays (1–4 rt) to fix hold and balance skew. |
| DRC: no adjacent dust from different nets, etc. | **Replace** | Use extraction-based LVS plus tile legality: every tile is in the library, ports match across every shared edge, and nothing is placed in an isolation ring. |
| STA: longest path → min clock period | **Expand** | Check max delay (setup) **and min delay (hold)**, the clock arrival time at each DFF, clock high/low pulse width against the DFF minimum, and the lamp's off delay when observing outputs. Integer tick delays make STA *exact*, so use that as a test: STA's bound must be at least every settle time seen in simulation, and if it isn't, STA has a bug. |
| Torch burnout in STA | **Move it to simulation** | Count turn-offs over sliding windows in random-stimulus simulation (§1, note 5). |
| litemapy → `.litematic` | **Keep the format, replace the library** | litemapy is perpetually in beta, writes format v6 with the 1.18.2 data version, and its packing is pure Python (§9). Write your own serializers in Rust on `fastnbt`, as thin writers over one `World`: vanilla structure **`.nbt`** first (vanilla `/place template` *and* Litematica both load it), then **`.litematic` v7** for convenience, then Sponge **`.schem`** only for `rilc`/MCHPRS. Keep **litemapy as an independent decoder in tests**: it reads your output back, which catches bit-packing bugs. |
| MCHPRS spot-check | **Demote it to a playground and a second extractor** | It targets MC 1.20.4, so it can't serve a 26.3 client. It loads only Sponge `.schem`. Its redpiler has no torch burnout, and the maintainers say it isn't vanilla-exact (§9). Use its headless **`rilc`** CLI as an *independent extractor* to cross-check yours, and the server to *play with* a big CPU, not to sign off on correctness. Ground truth is the vanilla harness. |
| Milestones | **Mostly keep** | Add **M0: tile library + vanilla harness**. Split the counter into a lever-clocked version and an oscillator-clocked version. Treat the register file and ROM as **parametric generators** ("macro compilers", like OpenRAM) that emit a tile-aligned template plus a timing model plus a Verilog model, not as hand-drawn macros. |

---

## 4. Recommended architecture

### 4.1 Pipeline

```
 cells/ (tile library: templates + port contracts + timing + function)   ← single source of truth
   │ gen .lib + Verilog cell models      │ templates, timing
   ▼                                     ▼
 top.sv ─► [1 Yosys+slang] ─► mapped.json ─► [2 Import] ─► Netlist IR
                 │                                            │
                 └─► [EQY / miter: RTL ≡ mapped]               ▼
                                         [3 Place] levelize → feedthroughs → order → tile coords
                                                              ▼
                                         [4 Route] clock spine, then PathFinder on tile grid
                                                              ▼
                                         [5 Time]  strength budget → repeaters; STA max/min → set repeater delays
                                                              ▼
                                         [6 Realize] stamp templates → World; dust shapes; bake quiescent state
                                                              ▼
                                         [7 Verify] Extract → Component Graph ─┬─► LVS vs Netlist IR
                                                                               ├─► Sim vs Verilator(top.sv, vectors)
                                                                               └─► burnout margin, STA ≥ sim settle
                                                              ▼
                                         [8 Export] .nbt  .litematic  .schem   + reports
```

Any failure in step 7 fails the build. You never ship a schematic the tool hasn't simulated from its own blocks.

### 4.2 The tile grid (the central decision)

- **Tiles.** The world is a 2D grid of tiles at a fixed pitch (P × P blocks horizontally, H blocks tall). Pick P and H after you've built the NOR3, crossing and DFF tiles in-game; expect P around 4–6. (Built: P = 5, H = 4, and each column has its own width and each row its own depth: routing columns take the narrow wiring tiles, 3 × 5, whose crossing climbs only north to south; rows no flip-flop sits in take the shallow tiles, 5 × 3, whose crossing climbs west to east; where both meet the tiles are 3 × 3 and no crossing fits. A row where signals keep dying is made a cell deep again. Two wire levels share the height, y = 1 and y = 3.)
- **Tile kinds.** `Empty`, `Gate(lib cell, part)` (multi-tile cells allowed), `Wire(from, to)` (straight or turn), `Repeater(from, to, delay)`, `Fanout(from, {to…})`, `Cross(x-net, z-net)`, `Input(lever)`, `Output(lamp)`, `ClockSpine`.
- **Port contract.** A signal crosses a tile edge only at the midpoint of that edge, at a fixed Y, as dust. The outer ring of each tile is air except at those midpoints. That makes any two tiles that share a matched port, or share no port at all, non-interacting **by construction**.
- **Proving it.** The contract is a *theorem* about the library: for every tile and every pair of adjacent tile kinds, the extracted connectivity equals the intended connectivity. The harness checks this exhaustively once per library change, and the claim gets checked instead of assumed.
- **Dataflow.** Gates take inputs on W/N/S and drive outputs on E, so data flows toward +X, which matches the levelized placement. Repeaters also act as diodes, which stops backflow.
- **Exact accounting.** Each tile has a fixed dust length and a fixed delay. Strength and delay along a route are therefore exact sums, with no extraction needed for timing.

The cost is density, maybe 2–4× worse than hand layout. You chose correctness over density, and this is what that choice looks like when you take it seriously. A later compaction pass can win area back *without* giving up the safety guarantees: every compaction move is followed by re-extraction and LVS.

### 4.3 Placement

1. Levelize, using the topological order of the combinational DAG. DFFs go at level 0 as sources, and their D inputs become sinks at the last level.
2. Insert a feedthrough for every edge that spans more than one level.
3. Order each column by barycenter sweeps (each gate by the average row of its drivers, four passes, the last kept: the *swept* rows). The sweeps do not settle, they oscillate, and the channel estimate (step 4) is cheap, so the build also settles the rows a second way: eight sweeps scored by the estimate (the footprint with every gap as wide as its channel density, then the summed vertical span of the nets), the best kept and improved by a local search over each column's order that takes a swap or a move of one gate only when the score falls (the *searched* rows, `place::Placer`). The estimate cannot see what the router pays for ramps and for two nets turning into one gate's pins, and placements chosen on the estimate alone measured worse on half the designs, so the searched rows are one more candidate for the pick (step 7), not a replacement: the router has the last word.
4. Assign coordinates: gate column `i` goes to tile column `i·(1+k_i)`, where `k_i` routing columns sit between gate columns. Estimate `k_i` from the max cut between levels and grow it if routing fails.
5. Place I/O: levers on the west edge, lamps on the east edge, and the register column with its clock spine on the far west.
6. Rows, by placement style (the compaction stage, `docs/PLAN.md`). At the four-row pitch (`pitch4`) every gate takes a north feed row, its own row, a south feed row and a free row. Packed (`packed`), a gate takes only the rows its pins use: a pin pays for a feed row, so it reads on the west whenever that is nearly as short, two neighbouring gates that would each take the same net on a feed side share one feed tile between them, and a free row is left after every few gates for the nets that must pass the column. Stretched (`stretched`), the rows are packed and then every column shorter than the tallest is drawn out over the tallest's rows, which leaves room for a clocked design's feedback and clock nets to pass. In both packed styles the clock inverter sits halfway down the register column instead of in a gate column of its own.
7. The pick. The build lays a design out in every style that applies, with the swept rows, and in the packing style with the searched rows too when they promise a smaller footprint; it checks and times each and keeps the best: for a combinational design (`pitch4` or `packed`) the smallest footprint, then the shortest settle, provided its inputs may change as often as under the four-row pitch; for a clocked design (`pitch4` or `stretched`; `packed` never won on one) the least footprint times clock period, then the shorter period. The pick's order is fixed and ties go to the earlier (the styles' order, the swept rows before the searched), so builds stay deterministic. The candidates are laid out in the order they can win, though: the packing style's searched rows, then its swept rows, pitch4 last; and for a combinational design a candidate whose footprint floor (its width, which routing adds no column to, by the depth down to the lowest row anything placed sits in, `Placement::footprint_floor`) already exceeds the footprint kept is given up on before it is routed, since it can only lose, so pitch4 is seldom laid out at all. It is laid out after all when the kept layout's inputs may change less often than every eight ticks, the one way it can still be taken. A clocked design's pick multiplies by the period, which the footprint does not bound, so its candidates are all laid out. `tests/baselines/area.json` records what every shipped design came to, and the gate fails any build that gets bigger or slower without the baseline being edited in the same commit.

### 4.4 Routing

- **Resources.** Each tile offers an X-track and a Z-track on the lower wire level, and the upper level (two wire levels, `docs/PLAN.md`). A straight wire uses one lower track, a turn or fanout both, and a `Cross` tile gives its X-track to one net and its Z-track to another. The upper level is one net's, any shape, above any lower wiring but a crossing (a stack); a ramp takes a net between the levels and holds its whole tile. The router uses the upper level where crossings run out: a net whose signal dies in a run of crossings (whose climbing arm cannot carry a repeater) hops up a level instead. In a routing column, three blocks wide, the crossing cannot turn to give its climbing net the flat arm, so that net can climb at most two in a row after a repeater: the router charges a narrow crossing more and counts a longer run as contended, so the negotiation moves a net off it instead of the strength check failing after it, and a narrow turn entered from the west or east carries its repeater in the arm it leaves by (the arm it enters by is the centre).
- **Clock first.** Route the clock first, as a deterministic spine with repeater taps.
- **Signals.** Route everything else with **PathFinder**: A* per net and per sink, with a history cost plus a present-congestion cost, then rip-up and re-route over iterations.
- **Costs.** Crossings get a cost because they add strength loss and maybe delay. Long detours get a cost through strength (repeater count).
- **Repeaters.** Strength is exact on the grid (each tile's dust steps, read off its file), so repeaters go only where a signal would arrive too weak: first each at the latest tile before the signal dies, which gives every path as few as it can have alone, then per net the fewest the tree allows without any sink getting more on its path than that (one repeater before a fork instead of one in each branch). The run of three straight tiles a repeater reaches is the physics; fewer repeaters come only from shorter wires.
- **Fallback.** If PathFinder doesn't converge in N iterations, widen the routing columns and retry. Optionally run fiction's `ortho` algorithm as a constructive baseline that always succeeds on combinational partitions. It needs fan-in ≤ 2 and its area grows quadratically, so it's a baseline and a sanity check, not the main engine (Appendix B).

### 4.5 Timing

- **Unit.** Use game ticks (`u32`) internally and report in rt. Vanilla schedules in game ticks and `/tick step` counts them. Short-pulse behaviour is only expressible at gt resolution: repeaters stretch 0–1 gt pulses, torches ignore some 2 gt pulses, and lamps turn off 4 gt late.
- **Same-tick order.** The simulator must implement vanilla's scheduled-tick priorities: a repeater facing a diode runs at −3, a repeater turning off at −2, other repeaters at −1, and torches at 0. Within one priority, ticks run in FIFO order.
- **Analysis.** Graph-based min/max arrival times over the routed design. Per-DFF clock arrival comes from the spine.
- **Setup.** `max_arrival(D) + t_setup ≤ T + clk_arrival(dff)`.
- **Hold.** `min_arrival(D) ≥ clk_arrival(dff) + t_hold`.
- **Fixing.** Fix hold by raising repeater delays on the offending paths, which costs no area: a tile of the flip-flop's own D branch first, else one it shares with another sink's path, and a layout that leaves a D path nothing to lengthen is refused. Fix skew by setting tap repeater delays on the clock spine. A period too fast for the burnout budget is lengthened two ticks at a time until no torch turns off more than the budget allows.

### 4.6 Realize and export

- **Stamp.** Copy the tile templates into a dense `World`.
- **Dust shapes.** Compute dust connection shapes with the same rules vanilla uses for the subset of blocks you emit.
- **Quiescent state.** Bake in the state from §1, note 2.
- **Serialize.** Write `.nbt`, then `.litematic`, then `.schem` (§9 explains the order). Index order inside `World` should match Litematica's `(y·Z + z)·X + x`, so writing `.litematic` is a straight pack.

---

## 5. Language, frameworks, other tools

### Recommendation: Rust for the whole core

- **Performance where it matters.** At CPU scale (a few thousand cells, a tile grid in the tens of thousands of nodes, about 20 PathFinder iterations), routing is around 10⁸ node expansions. That's seconds in Rust and tens of minutes in Python. Block-level random-stimulus simulation of a CPU over thousands of cycles is out of reach in Python. You'd hit both walls around the ALU milestone, and porting the core halfway through costs more than starting in Rust.
- **Types that prevent this domain's bugs.** Newtypes keep `BlockPos` and `TilePos` from being mixed up. A `Facing` enum avoids the sign errors that come with axis conventions. Block states as a typed `enum` mean the extractor and the legality checker are exhaustive `match`es: if you add a block kind, the compiler shows you every place that needs to handle it.
- **The ecosystem fits.** `serde_json` reads Yosys JSON, `fastnbt` handles NBT, `petgraph` covers graphs if you want it, and `proptest` gives property tests. MCHPRS is Rust, so its redstone code is a readable reference for edge cases.
- **Python, but only for test glue.** The vanilla RCON harness and the litemapy read-back test are I/O-bound and benefit from litemapy being an *independent* implementation.

**Why not C++?** In-process access to ABC, mockturtle, fiction or nextpnr. You need none of them in-process: Yosys runs as a subprocess, and fiction runs offline through its Python bindings. If you know C++ much better than Rust, C++20 is a reasonable second choice. **Python is not** a reasonable choice for the core.

### nextpnr (Viaduct / Himbächel): no

Both model a **fixed device**: bels at fixed sites, and a pre-built routing graph of wires and pips with exclusive ownership. Your target has no fabric. To use them you would:

- invent a fabric, which is essentially the tile grid above, and then
- fight nextpnr's conflict model (exclusion between neighbouring routes, directional repeaters, a path-dependent strength budget), its timing model, and its C++ build.

What you'd get in return is a placer (HeAP/SA) and router2. At your scale each of those is a few hundred lines of your own code, which you can then fit to redstone. OpenROAD has the same mismatch plus LEF/DEF overhead.

**Writing your own P&R is the right call here, not NIH.** Borrow the *algorithms*: Sugiyama, PathFinder, and ortho.

### Tools and prior art you missed

- **EQY** (YosysHQ) for equivalence. **SymbiYosys** if you want to prove properties of your SV.
- **Verilator** as the golden simulator.
- **fiction** (cda-tum), a physical-design framework for field-coupled nanotechnologies. Its problem is the same as yours: a clocked tile grid with gate tiles, wire tiles and crossing tiles, quantized delays, and no insulation. Its `ortho` algorithm is a linear-time, constructive P&R that always succeeds, but only for combinational DAGs with fan-in ≤ 2, and its area grows quadratically (Appendix B). Read the paper even if you never use the code.
- **MCHPRS redpiler and `rilc`.** Read the source as a reference for compiling a block world into a component graph, which is exactly your extractor. Run `rilc` as a subprocess to get a second, independent extraction for diffing.
- **YoWASP** (Yosys compiled to WebAssembly) as the synthesizer, on your machine and in the browser. See "Changes since the review".
- **The vanilla server harness** (`/tick freeze|step`, `/place template`, `/execute if block` over RCON): automated ground truth with no mods. The commands only drive the test harness; the circuits themselves stay command-free.
- **Prior art** from earlier HDL→Minecraft projects (Appendix B).

---

## 6. Core data model

Principles:

- Arenas with newtype indices (`CellId(u32)`), not `Rc<RefCell<…>>`.
- Every stage's output is immutable and serializable to `build/<design>/NN_<stage>.json`, so any stage can be re-run, diffed or snapshot-tested.
- Block states are typed, never strings, until export.

```rust
// ---------- Netlist IR (bit-level, post-synthesis) ----------
struct Netlist {
    cells:   IndexVec<CellId, Cell>,
    nets:    IndexVec<NetId, Net>,
    inputs:  Vec<Port>,              // Port { name, bit, net }
    outputs: Vec<Port>,
    clock:   Option<NetId>,          // exactly one, enforced on import
}
struct Cell { lib: LibCellId, name: String, pins: SmallVec<[NetId; 4]>, init: Option<bool> }
struct Net  { name: String, driver: Driver, sinks: Vec<PinRef> }
enum   Driver { Cell(PinRef), Input(PortIdx), Const(bool) }
struct PinRef { cell: CellId, pin: u8 }

// ---------- Tile library (loaded from cells/) ----------
struct LibCell {
    name: String,
    function: CellFn,                 // Nor(n) | OrDiode(n) | Dff | TieHi | TieLo …
                                      //   → generates .lib entry + Verilog model + sim model
    footprint: (u8, u8),              // in tiles
    pins: Vec<PinDef>,                // PinDef { name, dir: In|Out|Clk, tile: (u8,u8), side: Side }
    arcs: Vec<Arc>,                   // Arc { from, to, min_gt, max_gt }  (DFF: setup/hold/clk2q)
    template: BlockTemplate,          // relative blocks; no rotation in v1 (dataflow is +X)
}

// ---------- Layout (tile level) ----------
struct TilePos  { x: i32, z: i32 }                 // tile grid
struct Layout {
    grid:      Grid2<Tile>,                        // dense
    placement: IndexVec<CellId, TilePos>,
    routes:    IndexVec<NetId, RouteTree>,         // tree of TilePos with entry/exit sides
}
enum Tile {
    Empty,
    Gate     { cell: CellId, part: (u8, u8) },
    Wire     { net: NetId, from: Side, to: Side },
    Repeater { net: NetId, from: Side, to: Side, delay_rt: u8 },
    Fanout   { net: NetId, from: Side, to: SideSet },
    Cross    { x_net: NetId, z_net: NetId },
    Input    { port: PortIdx }, Output { port: PortIdx }, ClockSpine { tap: Option<u8> },
}

// ---------- World (block level) ----------
struct BlockPos { x: i32, y: i32, z: i32 }        // MC axes: +X east, +Y up, +Z south
struct World {
    size:   (u16, u16, u16),
    blocks: Vec<Block>,      // index = (y*sz + z)*sx + x  (Litematica order). Dense: a 400×24×400 CPU ≈ 3.8M cells, trivial
    intent: Vec<Owner>,      // which net/cell the generator *meant* this block to belong to → LVS diagnostics, viewer
}
enum Block {                 // closed set: everything we are allowed to emit
    Air, Solid(Material), Glass, Slab { top: bool },
    Dust { power: u8, n: DustSide, e: DustSide, s: DustSide, w: DustSide },
    Torch { lit: bool }, WallTorch { facing: Dir4, lit: bool },
    Repeater { facing: Dir4, delay: u8, locked: bool, powered: bool },
    Lever { face: Face, facing: Dir4, powered: bool }, Lamp { lit: bool },
}
enum Owner { None, Net(NetId), Cell(CellId), Clock, Structural }

// ---------- Component graph (shared by netlist-sim and extracted-sim) ----------
struct Cg { nodes: IndexVec<NodeId, Node> }
struct Node {
    kind:   NodeKind,            // Torch | Repeater{delay} | Lever | Lamp  (Comparator later, maybe)
    inputs: Vec<Edge>,           // Edge { from: NodeId, loss: u8 }  (dust distance → strength)
    side:   Vec<Edge>,           // repeater lock inputs
    origin: Origin,              // BlockPos (extracted) or (CellId, pin) (netlist-expanded)
}
```

Notes:

- **One component graph, two producers.** That shared IR is what makes the debugging ladder in §7 possible. The simulator is event-driven at game-tick resolution and makes same-tick ordering explicit (see the order fuzzing in §7).
- **Solid blocks by meaning.** Use different `Solid(Material)`s for gate bodies, wire supports and spine supports, for example different concrete or wool colours. The block type makes no difference to the logic, but it makes in-game debugging far easier. Never emit gravity blocks.
- **Dense storage.** Dense arrays beat hash maps at every scale you'll reach. Don't build a sparse voxel structure.

---

## 7. Repo structure and test strategy

### Layout

*As built (2026-10-07):* the core is flat `crates/redfab-core/src/*.rs` files (plus `src/sim/`), not the module directories below; tiles are `cells/*.tile` text files with no `meta.toml`; the SVG layer view was never built (`redfab layout` and `redfab replay` print text); Python tests live in `harness/tests/` and the smoke sources in `tests/`. `docs/PLAN.md` "Where things live" is the current map.

Two crates from the start: the core has to run in a browser, so everything that touches files or processes stays out of it. Modules are added to the core stage by stage (`PLAN.md`).

```
redfab/
  check                  # runs every acceptance check so far; --vanilla adds the server checks
  versions.env           # pinned tool versions and hashes
  crates/
    redfab-core/         # pure computation; also builds for wasm32. clippy.toml bans files, threads, clocks, hash maps
      src/
        netlist/         # Yosys JSON import, validation (single clock, single driver, known cells)
        lib/             # tile library; generators for .lib and Verilog cell models
        place/  route/  timing/
        world/           # Block enum, World, dust-shape computation, stamping
        extract/         # World → Cg  (+ "why is this powered" query)
        sim/             # Cg simulator, burnout counter
        lvs/             # Cg ≅ Netlist
        export/          # structure_nbt.rs first; litematic.rs and sponge_schem.rs later
        view/            # per-layer text and SVG dump
    redfab-cli/          # the `redfab` command: reads files, calls the core
  cells/                 # THE library: one dir per tile: template (ASCII layers), meta.toml, vanilla traces
  synth/                 # Yosys script template, SV-subset lint
  designs/               # nor2/, fa1/, adder4/, counter4/, fsm/  each with top.sv + stimulus spec
  harness/               # Python: vanilla-server lifecycle, RCON, experiments
  tools/                 # setup.sh, Yosys runners
  tests/                 # smoke designs, integration tests
  web/  mod/  server/    # playground, Fabric mod, backend (stages 10–12)
  docs/                  # DESIGN.md, PLAN.md, trace-format.md, mechanics.md
```

Write tile templates as **ASCII art, one grid per Y layer** with a legend, in text files. They're diffable, reviewable and greppable. Generate the in-game preview from them, never the other way round.

### Test pyramid

| Layer | What | Oracle | Runs |
|---|---|---|---|
| Unit | Coordinate and facing transforms; dust-shape computation; strength accounting; STA on small hand-built graphs; simulator semantics on micro-circuits (inverter, repeater delays 1–4, lock/unlock) | Hand-written expectations, cross-checked against vanilla traces | Every build |
| Format | Write `.litematic`/`.nbt`/`.schem`, read back with **litemapy** / `amulet-nbt` (independent decoders), and compare block by block. Include palettes of size 2, 3, 5, 17 and 65 so the packing crosses `u64` boundaries. Also compare against a **golden `.litematic` saved in-game** by Litematica 0.29.1 | Independent implementation + Litematica | Every build |
| Extractor cross-check | Export the `.schem`, run MCHPRS `rilc --export`, and diff its graph against your extractor's graph on every tile and every design | Independent extractor | Every build (once `rilc` is wired in) |
| Router/placer properties | `proptest` over random DAGs: every sink reached, no track used twice, strength ≥ 1 everywhere, repeater spacing respected, deterministic for the same seed | Invariants | Every build |
| **Layout ≅ netlist** | Random small netlists → full P&R → extract → **LVS must pass** | Isomorphism | Every build |
| **Tile library** | Each tile: exhaustive inputs and measured delays in *your* simulator **must equal recorded vanilla traces** (committed fixtures). Each pair of adjacent tile kinds: extracted connectivity = intended | Vanilla (recorded) | On library change |
| **Design golden** | For each design: random stimulus vectors → Verilator per-cycle outputs vs block-level simulation of the **extracted** world sampled at a fixed tick after each clock edge | Verilator | Every build |
| Order fuzzing | Re-run the design simulations with the scheduling order of **same-tick, same-priority** events shuffled. In vanilla that order comes from the neighbour-update order, which depends on location and orientation unless Redstone Experiments is on. Any output difference means the design depends on update order, which is a race and a bug | Self-consistency | Every build |
| STA soundness | Max settle time seen in simulation ≤ STA bound, for every design | STA vs sim | Every build |
| Vanilla end-to-end | Milestone designs on a headless vanilla 26.3 server, **in both the default and Redstone Experiments modes**. Steps: `/forceload` the area (1.21.9 removed spawn chunks), `/tick freeze`, `/place template` (strict and non-strict), drive inputs, `/tick step N`, then read lamps with `execute if block …[lit=true]`. Set `pause-when-empty-seconds` so the server doesn't pause with no players. Include a **negative control**: a deliberately broken copy must fail (copied from RTL2MC) | Vanilla (live) | Manual / nightly |

### Debugging ladder

When a design fails, go down the ladder:

1. RTL in Verilator ≠ mapped netlist (simulated through the netlist→Cg front end with ideal wires)? **Synthesis or cell-model bug.**
2. LVS fails? **Layout bug.** The LVS report names the block.
3. LVS passes, but the extracted simulation ≠ the netlist simulation? **Timing bug.** Look at STA, hold and skew.
4. Everything passes but vanilla disagrees? **Your simulator or extractor is wrong about a mechanic.** Write a micro-circuit fixture for that mechanic and fix the model.

---

## 8. Smallest end-to-end slice

**Slice 1: `assign y = ~(a | b);`**, which is one NOR2 cell. Take it through *every* stage and *both* oracles:

1. Yosys + slang → mapped JSON with one `NOR2` cell, with the .lib generated from `cells/`.
2. Import the netlist, then do trivial placement: levers in column 0, the gate in column 1, the lamp in column 2.
3. Trivial routing with straight wire tiles only. Fail loudly if a turn would be needed.
4. Realize, including the baked quiescent state.
5. Extract → LVS (one torch node) → simulate the four input vectors and compare with Verilator.
6. Export `.litematic` and `.nbt`. litemapy reads the `.litematic` back.
7. Harness: start the vanilla 26.3 server, `/forceload` the area, `/place template` (strict, then non-strict), drive all 4 vectors over RCON, and read the lamp. Then run the negative control: flip the gate to NOR-with-one-input-unconnected, and the harness must report a failure.

Explicitly **not** in slice 1: PathFinder, real levelization, STA beyond a sum, DFFs, and a Liberty file with real timing.

It's worth doing first because it builds every *interface* and both verification loops once. Every later feature then plugs into a loop that already works. Most of the risk is in the loops, not in the algorithms.

Before slice 1 comes **slice 0**: build NOT, NOR2, a wire, a lever and a lamp tile by hand in-game, record their traces with the harness, and commit them as fixtures. That's half the harness.

### Revised milestones

| M | Content | New things it forces |
|---|---|---|
| M0 | Tile library v0 + vanilla harness + extractor/simulator validated against the recorded traces | Ground truth |
| M1 | NOR2 slice → 1-bit full adder → 4-bit ripple adder | Fanout, crossing tiles, PathFinder, repeater insertion, max-delay STA |
| M2a | DFF tile characterized (setup/hold sweep) → T flip-flop → 4-bit counter, **lever clock** | Hold STA, clock spine, skew, quiescent state, `init` |
| M2b | Same counter with an on-board ring-oscillator clock | Clock generator, pulse width, burnout margin |
| M3 | FSM (sync reset, one-hot and binary) | Reset, wider fan-in, back-edge routing at scale |
| M4 | 8-bit ALU | Congestion, long wires, glitch burnout, runtime (time it here) |
| M5 | Macro generators: register file, ROM | Fixed-block placement, macro timing models |
| M6 | Tiny CPU | Scale, stacked planes (maybe), paste logistics |

---

## 9. Tool status (checked 2026-10-01)

Facts were read from the GitHub API, crates.io, PyPI, Modrinth, Mojang's version manifest, and the 26.3 server jar. "Inferred" means I read it from code and didn't test it.

### Four of your assumptions are out of date

1. **yosys-slang is now called sv-elab, and it's built into Yosys.** Yosys has included it since **v0.67** (2026-07); it's enabled by default, and the command is still `read_slang`. You don't need a plugin, so just pin a Yosys release.
2. **Minecraft uses year-based versions now.** The current release is **26.3** (2026-09-15): protocol 777, data version **5023**, Java 25. The last 1.x release was 1.21.11.
3. **MCHPRS is still on 1.20.4.** A draft PR for 1.21.8 is open, and nothing targets 26.x. A 26.3 client can't connect, and MCHPRS can't load `.litematic` files.
4. **litemapy writes an old format version.** It writes Litematica format **v6** with data version 2975 (MC 1.18.2). Litematica itself writes **v7**. Litematica will still load and upgrade a v6 file.

### Status table

| Tool | Status | Evidence | Verdict |
|---|---|---|---|
| **Yosys** | **v0.69** (2026-09-09), monthly releases. v0.69 removed `abc -fast` and `synth -retime/-noabc9` | [releases](https://github.com/YosysHQ/yosys/releases) | Pin one version, e.g. 0.69, and record it in the build report. |
| **sv-elab** (formerly yosys-slang) | Active: last commit 2026-09-29 (slang v12.0). Irregular date-named tags since 2026-07. Bundled into Yosys ≥ 0.67 at `frontends/slang/lib`. The standalone plugin supports Yosys 0.52–0.66 | [povik/sv-elab](https://github.com/povik/sv-elab), Yosys v0.67 notes | **Healthy. Use the built-in version.** Known gaps that will affect you are listed under "SV subset" below. |
| **OSS CAD Suite** | Nightly builds, including **darwin-arm64** (latest 2026-09-30). Bundles Yosys, sv-elab, EQY, SBY and Verilator | [releases](https://github.com/YosysHQ/oss-cad-suite-build/releases) | **This is how you install the frontend on your Mac.** None of these tools are installed on this machine yet, and neither is a Rust toolchain. |
| **EQY** | Maintained (last commit 2026-09-16) but rarely tagged (last tag `yosys-0.47`, 2024-11). Ships in OSS CAD Suite | [YosysHQ/eqy](https://github.com/YosysHQ/eqy) | Use the OSS CAD Suite copy. |
| **litemapy** | Last commit 2026-04-05. Last PyPI release **0.11.0b0** (2025-06-14); every release is a beta. Writes v6 with data version 2975. Pure-Python bit packing (slow on big files). `region[x,y,z]=…` silently drops block-entity data (PR #64 is open). Depends on `nbtlib`, last released 2021 | [repo](https://github.com/SmylerMC/litemapy), [PyPI](https://pypi.org/project/litemapy/) | **Not a production dependency, but a good test oracle.** As an independent decoder in the format test it's exactly right. Don't put it on the export path. |
| **MCHPRS** | Active: v0.5.2 (2026-08-14), last commit 2026-09-28. Repo is **`MCHPR/MCHPRS`** (`MCHPRS/MCHPRS` returns 404). MIT. **MC 1.20.4** / protocol 765. `//load` reads Sponge v2/v3 only. Redpiler has **no torch burnout** (inferred), no observers, no pistons; "running without redpiler is not the same as running in vanilla" (maintainer, #157). Crates aren't on crates.io (git dependency only). **`rilc`** is a headless CLI: Sponge schematic → redpiler passes → `--export` graph | [repo](https://github.com/MCHPR/MCHPRS), [Redpiler.md](https://github.com/MCHPR/MCHPRS/blob/master/docs/Redpiler.md) | **Don't use it as an oracle; use it as a second extractor (via `rilc`) and as a playground.** Running it as a server needs a separate 1.20.4 client. |
| **Litematica** | Maintained by **sakura-ryoko** (fork, default branch `26.3`), published on Modrinth under masa's project. **26.3 → 0.29.1** (2026-09-27), 26.2 → 0.28.8, 1.21.11 → 0.26.16. Fabric only. Writes format **v7.1** and can still write v6. **Loads Sponge `.schem` and vanilla `.nbt` as well** | [sakura-ryoko/litematica](https://github.com/sakura-ryoko/litematica), Modrinth | Supports current versions. Because it loads `.nbt`, `.litematic` is a convenience format, not a requirement. |
| **Minecraft Java** | **26.3** (2026-09-15). Scheme is YY.D.H. **Redstone Experiments** (since 1.21.2) is *still experimental*. When enabled it gives deterministic wire update order (nearest-to-source first, then left first) and fewer block updates | [manifest](https://piston-meta.mojang.com/mc/game/version_manifest_v2.json), [wiki](https://minecraft.wiki/w/Redstone_Experiments) | Pin **26.3**. Run the harness in **both** redstone modes: if the result differs, the design depends on update order, which is a bug. |
| **nextpnr** | 0.11.1 (2026-08-11), very active. Himbächel (gowin, gatemate, xilinx, ng-ultra) and Viaduct (FABulous) are both maintained. Both assume a fixed device: bels at sites plus a pre-built wire/pip graph with one net per wire | [releases](https://github.com/YosysHQ/nextpnr/releases) | Healthy, but **the wrong abstraction** (§5). I found nobody using it for anything except FPGAs and eFPGAs. |
| Verilator | Ships in OSS CAD Suite | — | Golden simulator. |
| **fiction** | v0.8.0 (2026-09-02), MIT, active. Python bindings: `mnt.pyfiction` | [repo](https://github.com/cda-tum/fiction) | Use offline as a baseline and for ideas (Appendix B). |

### Libraries for the Rust core

| Crate | Version / date | Use |
|---|---|---|
| `fastnbt` | 2.6.3 (2026-08-08), active | **NBT read and write for all three formats.** |
| `rustmatica` | 0.5.2 (2025-02-04), **stale**. Writes v6 with data version 2975; its `mcdata` dependency stops at 1.21.7 | **Don't use it.** Write your own `.litematic` writer, about 150 lines. |
| `valence_nbt`, `quartz_nbt` | 2023 and 2024, dormant | Don't use. |
| `simdnbt` | 0.10.0 (2026-03-28) | An alternative if `fastnbt` ever gets in the way. |
| `mchprs_redpiler`, `redpiler_graph` | git dependencies only | Optional. Prefer running `rilc` as a subprocess, so their churn doesn't reach your build. |

For Python test glue: `amulet-nbt` 2.1.8 (2026-04-10) is the maintained NBT library. Use it instead of `nbtlib` in your own harness code. `mcschematic` 11.4.4 only knows block versions up to 1.21.5; skip it.

### Export format order (revised from the plan)

1. **Vanilla structure `.nbt`** first. It's the simplest format: a palette plus a list of `{pos, state}` with no bit packing. **Both** vanilla `/place template` and Litematica load it.
2. **`.litematic`** second, as a convenience for Litematica. Write **v7 with data version 5023**, and verify it against a golden file: save a 3-block schematic with Litematica 0.29.1 in-game, commit it, and require your writer to produce equivalent NBT.
3. **Sponge `.schem` v2** third, and only for `rilc` and MCHPRS.

### SV subset: sv-elab gaps that will bite

Treat this as your **lint list** in `synth/`, so the frontend's limits show up as clear errors rather than mysterious netlists. Known as of 2026-09:

- **Open bugs:**
  - A `case … inside` that covers every value still produces a combinational loop (#387).
  - A function called from several procedural blocks infers a spurious `$dlatch` (#376).
- **Unsupported:** unions, `inout` (unless you keep hierarchy, #143), non-constant `casex`/`casez` items (#186), SR latches, and `tri0`/`tri1`/`supply*`.
- **Enums:** `.next` and `.prev` aren't supported (#295, #158).

The `$dlatch` bug alone is a reason to make "no latches in the netlist" a **hard error** after synthesis.

---

## 10. Open decisions

1. **Which Minecraft version to pin.** I recommend **26.3**, with Litematica 0.29.1, the vanilla 26.3 server jar and Java 25. MCHPRS's 1.20.4 doesn't constrain this, because it's no longer in the correctness loop. Bump the version deliberately, and only after re-running the tile-library vanilla traces.
2. **Tile pitch P and plane height H.** Decided in stage 5: 5 × 5, 4 high. The DFF (stage 8) has to fit that; if it can't, it becomes a two-tile cell rather than a larger pitch.
3. **Comparators and copper bulbs.** I recommend **no** for v1. Comparators make strength analog, which complicates extraction and legality. A waxed copper bulb plus a comparator is a T flip-flop that toggles in 0 gt plus the comparator's 2 gt, and it can't burn out. That's tempting for counters, but it's a T-FF, not a D-FF, and redpiler and `rilc` can't model it, so you'd lose the second extractor. Revisit both later, for XOR, ROM and counter density.
4. **Rust.** If you're not willing to write Rust, use C++20, not Python (§5).
5. **Which DFF design.** Master/slave repeater-lock, or a pulse-latch. Build both in-game and characterize both, then pick the one with the larger hold margin, not the smaller footprint.

---

## 11. Online playground

The end goal: a web page with a code editor and a live view of the circuit running on a real Minecraft server.

- **Untrusted code runs only as WebAssembly.** The browser compiles with YoWASP Yosys and the WebAssembly build of `redfab-core`, so previews need no server. The server recompiles from source inside a WebAssembly runtime with CPU and memory limits. It never accepts blocks from a browser.
- **Verilator never runs on user code.** Its inline C++ (`$c`) is arbitrary code execution. It stays in the test suite.
- **The view is drawn in the browser, not streamed as video.** The browser already has the geometry from its own compile. A small observe-only Fabric server mod streams block-state changes with their tick number over a WebSocket, in the trace format from `trace-format.md`. Streaming a real game client would need about one GPU machine per camera.
- **Web levers** place or remove a `redstone_block` at each input's probe position, the same mechanism the test harness uses.
- **Server:** Fabric, because Paper can change redstone behaviour. One plot per submission. Limits on gate count, footprint and minimum clock period. All plots share the game's 20 ticks per second, so server tick time is monitored.
- **Legal:** "Minecraft" can't be the site's main name, the site needs the "NOT AN OFFICIAL MINECRAFT PRODUCT" notice, any game footage must be free to view, and a public site can't ship Mojang's textures.

Things still to confirm, each at the start of the stage that needs it:

- A Fabric mod loads on 26.3 (stage 11). Fabric lists 26.3 as stable, but I haven't built against it.
- Whether 26.3's built-in management server (`management-server-*` in `server.properties`, seen in stage 0) can replace part of the mod.

---

## Appendix A: Redstone facts this design depends on

Java Edition, current through 26.3. Most of these come from [minecraft.wiki](https://minecraft.wiki). Where a fact comes from source code or from my own derivation, I say so. Items marked **MEASURED** were checked against the real game in stage 1 (repeater details in stage 4, the random-circuit comparison of the two modes in stage 2, the flip-flop in stage 8); `mechanics.md` has the numbers and the fixtures. 1 gt = 50 ms; 1 rt = 2 gt.

| Fact | Source | What it means for redfab |
|---|---|---|
| **Delays.** Torch: 2 gt. Repeater: 2, 4, 6 or 8 gt. Comparator: 2 gt. Dust: 0. **Lamp: turns on instantly, turns off after 4 gt.** Repeaters stretch shorter on-pulses to their delay. Torches don't respond to every 2 gt pulse. **MEASURED** (torch, repeater, lamp, dust decay) | [Torch](https://minecraft.wiki/w/Redstone_Torch), [Repeater](https://minecraft.wiki/w/Redstone_Repeater), [Lamp](https://minecraft.wiki/w/Redstone_Lamp) | Use integer gt in STA and the simulator. Sample output lamps at least 4 gt after the latest possible falling edge. Treat any pulse shorter than 4 gt as having an *undefined* effect. Clock pulses and lock pulses must be ≥ 4 gt by design. Glitch pulses count toward burnout, but no logic may depend on them. |
| **Burnout.** **MEASURED:** a torch burns out on its **8th** turn-off within 60 game ticks (the wiki's "more than eight" is wrong). It then ignores input and relights when a 160-gt tick scheduled at burnout fires (observed every time, with no other updates arriving) | [Torch](https://minecraft.wiki/w/Redstone_Torch); `mechanics.md` | Budget: at most 6 turn-offs per 60-gt window (`BURNOUT_BUDGET`, one to spare; §1, note 5 derived 7 before the measurement). MCHPRS doesn't implement burnout (source search, 2026-09-28). |
| **Dust connections.** Dust connects diagonally one block up or down, "unless there is a conductive block above the lower redstone wire". **Non-conductive blocks never cut a connection**: conductivity matters, transparency doesn't. **MEASURED** | [Redstone Dust](https://minecraft.wiki/w/Redstone_Dust), [Conductivity](https://minecraft.wiki/w/Redstone_mechanics#Conductivity) | Tile isolation rings must account for diagonal connections. A glass block over a wire doesn't isolate it, but a solid block does. |
| **Vertical asymmetry.** Dust on non-conductive blocks (glass, upside-down slabs, glowstone) passes power **upward only** in Java ("the signal can never go down from slabs"). **MEASURED** for glass | [Redstone Dust](https://minecraft.wiki/w/Redstone_Dust) | Upward vias come with a built-in diode. Downward vias need conductive steps, which interact with their neighbours. This is why up and down need different tiles, and why v1 uses one plane plus crossing tiles. |
| **Dust shapes since 1.16.** A lone dust placed by a player is a "+" that powers in **all four directions**; one placed by `setblock` with no properties is a **dot** (MEASURED). A dot powers only the block beneath it (MEASURED). A wire with one connection points both ways, and its far end powers the block it points into. When dust reshapes **without** a power change, it doesn't update other components. States: `north/south/east/west ∈ {up, side, none}` and `power 0–15` | [Redstone Dust](https://minecraft.wiki/w/Redstone_Dust) | The exporter must compute exact dust shapes, because a paste without updates keeps whatever shape you write. Never emit a lone dust. |
| **Strong vs weak power.** A torch strongly powers only the block above it; adjacent components see the torch directly. A repeater or comparator strongly powers the block in front. Dust **weakly** powers the block it sits on and the block it points into. A lever or button strongly powers the block it's attached to. A weakly powered block activates repeaters, comparators, torches and lamps, **but not dust**. **MEASURED** (torch, dust → block → dust/repeater/torch) | [Redstone mechanics](https://minecraft.wiki/w/Redstone_mechanics#Powered_blocks), [Components](https://minecraft.wiki/w/Redstone_components); MCHPRS `get_strong_power` | Gives the NOR gate (dust → block → torch). It's also the source of most cross-net leaks: a strongly powered block powers *all* adjacent dust. Input tiles have to isolate the lever's mounting block. |
| **Repeater locking.** Locked only by a **powered repeater or comparator** facing its side; dust and blocks can't lock it. While locked it "completely ignores all block updates and scheduled ticks". **MEASURED:** if the lock and the input change on the same tick, the repeater doesn't switch, because the locking repeater ticks first (priority −3 vs −1) | [Repeater § Signal locking](https://minecraft.wiki/w/Redstone_Repeater#Signal_locking), [Redpiler.md](https://github.com/MCHPR/MCHPRS/blob/master/docs/Redpiler.md) | DFFs. Also a leak hazard: a repeater of one net facing sideways into another net's repeater locks it. Tile rules must forbid that. |
| **Standard DFF.** The wiki's design H (rising edge) is a master/slave with four repeaters and a torch. My timing derivation said clk→q = 6 gt. **MEASURED (stage 8):** clock-to-Q 8 gt, setup 1, hold 5, minimum high and low 4, minimum period 12, acting on the falling edge. The master's output change and the slave's lock do land on the same tick, harmlessly: a locked repeater ignores a pending tick, so the lock wins whichever is processed first | [Memory circuits](https://minecraft.wiki/w/Redstone_circuits/Memory#D_latches_and_flip-flops); `mechanics.md`, `cells/dff.tile` | Characterized in the harness (§2); it is the `dff` tile every clocked design uses. |
| **Scheduled-tick priorities.** Repeater facing a diode: −3. Repeater turning off: −2. Other repeaters: −1. Comparator facing a diode: −1. Everything else: 0. Within one priority, ticks run in scheduling order. At most 65,536 scheduled ticks per gt. **MEASURED** (−3, −2, −1 and 0 seen in the saved pending ticks, stages 1 and 4) | [Tick § Scheduled tick](https://minecraft.wiki/w/Tick#Scheduled_tick) | The simulator must implement these priorities exactly. Order fuzzing permutes events only *within* the same priority. |
| **Redstone Experiments** (1.21.2+, still experimental in 26.3): wire updates only the blocks it can power, sets all wire strengths before sending updates, and uses a deterministic update order (nearest to source first, then left first). The default mode's order still depends on location and orientation. **MEASURED** (stage 2): 6% of random circuits behave differently in the two modes; every tile and design is checked in both and must agree | [Redstone Experiments](https://minecraft.wiki/w/Redstone_Experiments), [analysis](https://gist.github.com/SpaceWalkerRS/25052d03ff956b988c50a75a08619545) | Run the harness in both modes. If a design behaves differently between them, it depends on update order. |
| **Legacy wire order and lamps.** In the default mode the wire algorithm updates in an order that depends on world position. When an output lamp's feed drops for exactly 4 gt (its own turn-off delay) and repowers in the tick its turn-off fires, the game at some placements schedules the lamp a second turn-off tick, which fires with no effect; the lamp never changes. **MEASURED** 2026-10-08 on adder4 and divmod at two of three placements; the Experiments mode never does it | `harness/design.py`, `trace.rs` | Block states are compared exactly in both modes. A pending-tick difference made only of lamp ticks is counted and reported, not failed, in the default mode (`check-traces` prints the count); any other pending difference, and any state difference, still fails. The simulator models the Experiments order, as planned |
| **`/tick`** (since 1.20.3): `freeze` stops scheduled ticks, but lever flips and dust power changes still happen. `step N` runs N gt and freezes again. **MEASURED:** it runs asynchronously at the tick rate (`tick rate 10000` makes it near-instant), the clock advances by exactly N, and `tick sprint N` runs N+1 | [Commands/tick](https://minecraft.wiki/w/Commands/tick) | The harness drives inputs while frozen, then steps. Dust and lamp turn-on settle during the freeze; torches and repeaters wait for `step`. |
| **`/place template … [strict]`** loads `generated/<ns>/structure/<id>.nbt`. By default it applies updates. `strict` (1.21.5+) places blocks as-is, without block or shape updates. The `.nbt` format has no pending ticks. **MEASURED:** a 70×2×70 structure loads; a consistent state placed strictly stays unchanged, and placed normally settles to the same state | [Commands/place](https://minecraft.wiki/w/Commands/place), [Structure file](https://minecraft.wiki/w/Structure_file) | Use it as the harness's loader. If templates over 48³ fail, split the design into chunks, or fall back to `fill`/`setblock … strict`, which are limited by `max_block_modifications`, default 32768. |
| **`/setblock … strict`** (1.21.5+) skips updates; the default `replace` mode updates. **MEASURED:** `setblock lever[powered=true]` does **not** update the neighbours of the lever's mounting block | [Commands/setblock](https://minecraft.wiki/w/Commands/setblock) | Give each input tile a **harness probe position**. The harness places or removes a `redstone_block` there instead of editing the lever. |
| **Reading state:** `execute if block X Y Z minecraft:redstone_lamp[lit=true]` fails if the chunk isn't loaded. RCON: `enable-rcon=true` plus a password; port 25575 | [execute](https://minecraft.wiki/w/Commands/execute), [RCON](https://minecraft.wiki/w/RCON) | The harness reads outputs through `execute if block`. |
| **Litematica paste** (sakura-ryoko source): single-player paste and Servux paste place blocks with `notifyNeighbors=false` and an update suppressor. Command paste adds `strict` (`commandUseStrict` defaults to true). Pending ticks stored in a `.litematic` are restored | [sakura-ryoko/litematica](https://github.com/sakura-ryoko/litematica) | **Bake a quiescent state.** Never write pending ticks. |
| **Chunks:** redstone runs at load level ≤ 32. A circuit that reaches into a border chunk (level 33) stalls. `simulation-distance` defaults to 10. Spawn chunks were removed in 1.21.9. `pause-when-empty-seconds` (1.21.2+) pauses an empty server | [Chunk](https://minecraft.wiki/w/Chunk), [server.properties](https://minecraft.wiki/w/Server.properties) | The harness must `/forceload` and disable the pause. Report the design's footprint in chunks. |
| **GameTest:** `/test` and block-based test instances work in normal play since 1.21.5 | [GameTest](https://minecraft.wiki/w/GameTest) | An alternative harness, used by RTL2MC. RCON plus `/tick` is simpler to start with. |
| **Copper bulb:** toggles on each rising edge in 0 gt and isn't conductive. A comparator reading it outputs 15 when it's lit. MCHPRS doesn't support it | [Copper Bulb](https://minecraft.wiki/w/Copper_Bulb) | See §10, item 3. |

## Appendix B: Prior art

You are not the first to try this. There have been about 20 attempts since 2013, and **most of them died in routing**. Repo dates below come from the GitHub API. I read READMEs and skimmed code; I haven't run any of these tools.

| Project | Year | Approach | Lesson for redfab |
|---|---|---|---|
| [MinecraftHDL](https://github.com/itsfrank/MinecraftHDL) (McGill) | 2016–20 | Yosys JSON. Levelized layers, with a repeater inserted on every skipped layer. **Left-edge channel routing**, with VCG cycles broken by "out-nets". 2D. Forge 1.10 | **This is almost exactly your plan.** It never supported sequential logic, because the layering loop deadlocks on feedback, and it had no verification. Read its paper before writing your router. |
| [PERSHING](https://sigtbd.csail.mit.edu/pubs/2016/paper4.pdf) / [dewey](https://github.com/qmn/dewey) (MIT) | 2016 / 2018 | Liberty from YAML. TimberWolf SA placement, SILK/Lee rip-up and re-route in 3D. Torch towers to go up, sticky pistons to go down | A 4-bit counter took 1084 s to route. A 63-cell decoder **never finished routing**. dewey "may run forever". Unconstrained 3D maze routing on a fine grid is a trap. |
| [HDL2Redstone](https://github.com/tech4me/HDL2Redstone) (U Toronto, supervised by Vaughn Betz) | 2019–20 | C++. .schem cells with JSON pin metadata, including a DFF. SA placement on a 3D grid, re-routing for congestion and repeaters, timing analysis | The closest prior attempt at an "ASIC flow". It's dead, but its cell metadata format is worth a look. |
| [verilog2minecraft](https://github.com/PietPtr/verilog2minecraft) | 2021 | NOT/OR/DFF. A* router that keeps "distance since last repeater" in the search state | That's a good trick if you ever route at block level. |
| [Redstone-Compiler](https://github.com/Redstone-Compiler/redstone-compiler) | 2023–26 | Rust. Clustering, then hierarchical P&R, with an exhaustive local placer and Pareto composition. Latches recognized as a cross-coupled-NOR core. WASM NBT simulator | Active, written in Rust, and its `docs/` design notes are good. |
| [RedstoneBuilder](https://github.com/AlexMelanFromRingo/RedstoneBuilder) | 2026 | Rust. SA placement, PathFinder routing (negotiated congestion, multi-source A* Steiner trees). DFFs. Writes .litematic | In-game checking is manual. |
| [**REDA**](https://github.com/Pr0gCat/REDA) | 2026-08 → | Yosys/ABC to a **NOR1–3-only** netlist. Deterministic placement plus A*, falling back to a row/channel emitter. **Four hard invariants** (spacing, connectivity, torch merge, strength). Own simulator plus a truth-table check **plus a conformance harness in a real Minecraft 26.2 server**. Writes .litematic | Independently converged on NOR-only cells, invariant checking and real-server conformance, the same position as §0–§3. **Study it before you start.** |
| [**RTL2MC**](https://github.com/NTP17/rtl2mc) | 2026-09 | Yosys, DC or Genus synthesis to NOR2/INV/DFF. Writes post-route `mapped.v` plus **SDF/SDC including router repeaters and clock balancing**, and re-simulates with timing. Then a **vanilla 1.21.1 server, plus GameTest with a negative control** (a deliberately broken copy must fail) | The most complete verification story I found. Copy the negative control. A harness that never fails proves nothing. |
| [dustc](https://github.com/0x1bd/dustc) | 2026-08 | Its own HDL. Text-format cells with setup/hold timing in ticks. Registers, generated clocks. Writes .schem | Shows that cell-level timing metadata in text files works. |
| [MineRTL](https://github.com/saucefishman/MineRTL), [RedSynth](https://github.com/zebulontaylor/RedSynth), [LogicLoom](https://github.com/MightyFilipns/LogicLoom), [ThunderFruta](https://github.com/ThunderFruta/RedstoneCompiler), [veristone](https://github.com/benjaminwp18/veristone) | 2025–26 | Various: force-directed or SA placement plus A*. ThunderFruta checks in MCHPRS and a Fabric server, combinational only | ThunderFruta [issue #2](https://github.com/ThunderFruta/RedstoneCompiler/issues/2) compares several of these tools. |
| [minetest_pnr](https://github.com/google/minetest_pnr) (Google CTF) | 2019 | Rust. Yosys `abc -g AND,OR,XOR…`. A 2D placer and router with crossings | Evidence that 2D plus crossings scales: an 8k×8k canvas. |
| [verilog2factorio](https://github.com/Redcrafter/verilog2factorio) | 2021–25 | Yosys JSON to combinator blueprints. SA placement, heuristic routing. Runs a RISC-V core | Not redstone, but proof that the full "Yosys to game" path can reach a CPU. |
| [Redstone2Verilog](https://github.com/raytzliu/Redstone2Verilog), [RedHDL](https://github.com/ebby-s/RedHDL) | 2023 | The reverse direction: world to Verilog | Reference implementations for your extractor and LVS. |
| [fiction](https://github.com/cda-tum/fiction) (TUM, v0.8.0, 2026-09-02, MIT) | active | FCN physical design. C++20 header-only, with Python bindings `mnt.pyfiction`. **ortho** (Walter et al., ASP-DAC 2019) is orthogonal graph drawing on a tile grid where signals only flow E/S, with explicit fan-out tiles and crossing tiles. O(\|N\|+\|L\|), never fails. Also exact SMT-based P&R and A*-based layout (NANO 2024) | Same tile and crossing model as §4.2. **Limits:** fan-in ≤ 2, combinational DAGs only (cut at the DFFs yourself), and area grows like (\|N\|/2)². Use it as a baseline, not as the main engine. |
| [mockturtle AQFP buffer insertion](https://github.com/lsils/mockturtle) | — | Path balancing for clocked-gate technologies | Only relevant if you ever wave-pipeline. With DFFs plus logic that settles within the period, tick-level STA is enough. |

**No one has used nextpnr, VPR or OpenROAD for Minecraft.** [porglezomp/minecraft-eda](https://github.com/porglezomp/minecraft-eda) set out to use nextpnr in 2019 and stopped at a Liberty generator. The tools that work cite PathFinder and reimplement it.

**MCHPRS redpiler as an extractor.** Its passes (`IdentifyNodes`, then `InputSearch` with BFS over dust where the edge weight is dust distance and link types are `Default`/`Side`, then optional optimizations) are exactly the extractor described in §4.1. `/redpiler compile --export` writes a bincode `Vec<Node>` in the format defined in `crates/redpiler_graph`, and `--export-dot` writes Graphviz ([docs](https://github.com/MCHPR/MCHPRS/blob/master/docs/Redpiler.md)). The headless `rilc` CLI in the same repo runs those passes on a Sponge schematic without starting a server. That gives you a **second, independent extractor for free**. Run it with optimizations off, and diff its graph against yours as a test of the extractor itself. Caveat: redpiler doesn't model pistons or observers, so keep them out of your tiles. That's no loss, since your tile set doesn't need them.

**What this means for you.** Decide early whether redfab is (a) a learning project or (b) a tool you want to exist.

- For (a), build it anyway, but read REDA and RTL2MC first so you don't relearn their lessons.
- For (b), contributing to REDA or RTL2MC may get you there faster.

What would actually be new in redfab is **correct-by-construction tiles with a machine-checked adjacency theorem, LVS through two independent extractors, and order-fuzzed simulation**. None of the projects above combine all three.
