# Redstone mechanics, as measured

What the real game does, measured with the harness on the pinned server (Minecraft 26.3, jar `33680f5f…`): 25 fixtures on 2026-10-04 (stage 1), five more on 2026-10-05 (stage 4, repeaters), the flip-flop sweeps on 2026-10-06 (stage 8), one more on 2026-10-07 (`lever_mount`), in both the default redstone mode and the Redstone Experiments mode. Every row except the flip-flop section's has a fixture in `fixtures/mechanics/` whose recording is committed and re-checked by `./check --vanilla` (`mechanics-verify`: 31 fixtures, 62 recordings; `docs-agree` in the fast gate holds these counts to the directory); the flip-flop rows come from the tile fixtures `tiles-vanilla` records. **All 31 fixtures behave identically in both modes.**

Units are game ticks (gt). "Tick t" follows `trace-format.md`: a stimulus at tick t is the last event of tick t, so a delay of d means the reaction shows at tick t+d.

## Delays and ranges

| Fact | Fixture | Measured | Matches the wiki? |
|---|---|---|---|
| Dust loses 1 power per block; the 16th block is unpowered | `dust_decay` | 15, 14, … 1, 0 | yes |
| Torch reacts 2 gt after its input changes, both directions | `torch_delay` | off at +2, on at +2 | yes |
| Repeater set to d delays 2d gt, both edges | `repeater_delay_1..4` | 2, 4, 6, 8 gt | yes |
| Lamp turns on immediately, off 4 gt later | `lamp_delay` | on at +0, off at +4 | yes |

## Torch burnout

| Fact | Fixture | Measured | Matches the wiki? |
|---|---|---|---|
| Threshold | `burnout_7`, `burnout_8` | 7 turn-offs in 60 gt: fine. **8 turn-offs: burns out on the 8th.** | **No.** The wiki says "more than eight". The game burns out *on* the eighth turn-off within 60 gt. |
| What happens after | `burnout_8..10` | Torch stays dark and ignores further input pulses. A 160-gt block tick is scheduled at burnout; when it fires the torch relights (observed at exactly +160). | The wiki calls the 160-gt retry uncertain; here it relit every time, with no other updates arriving. |

**Design rule:** the game allows a torch **7 turn-offs in any 60-gt window**. redfab's burnout budget check allows at most **6** (`BURNOUT_BUDGET` in `crates/redfab-core/src/timing.rs`), one to spare; `./check` drives the inverter at twice its reported input rate and requires both the tool and the simulator to catch it.

## Repeater locking

| Fact | Fixture | Measured | Matches the wiki? |
|---|---|---|---|
| Lock and input changing on the same tick | `lock_same_tick` | Both repeaters schedule for +2. The lock repeater (priority −3, because it faces into another repeater) runs first; the data repeater (priority −1) is locked when its own tick comes and does not switch. Output stays 0 for the whole run. | yes |
| Scheduled-tick priorities | `lock_same_tick`, saved `block_ticks` | repeater facing a diode: −3; other repeater: −1; torch: 0 | yes |

## Repeaters in more detail (stage 4)

| Fact | Fixture | Measured | Matches the wiki? |
|---|---|---|---|
| A repeater works standing on glass | `repeater_on_glass` | passes its input on after its delay | yes (and the tile library depends on it) |
| A repeater strongly powers the block in front of it | `repeater_strong_front` | dust beyond that block gets 15 | yes |
| Dust beside a repeater does not join it | `repeater_side_dust` | the side dust stays at 0 and does not connect | yes |
| An on-pulse shorter than the delay is stretched to the delay | `repeater_pulse_stretch` | a 2-gt pulse into a 6-gt repeater comes out 6 gt long | yes |
| A locked repeater holds its output whatever its input does, and follows its input again once released | `lock_hold_release` | held through an input rise and an input fall; 2 gt after the lock repeater turned off, the data repeater caught up | yes |
| A repeater placed by command beside a powered locking repeater is locked at once | probe | `locked=true` on placement | yes |

Tick priorities seen in the saved pending ticks, all as documented: a repeater feeding the back or side of another repeater −3, a repeater turning off −2, any other repeater −1, torches and lamps 0.

## Connectivity and powering

| Fact | Fixture | Measured | Matches the wiki? |
|---|---|---|---|
| Dust steps up diagonally when the block above the lower dust is air | `diag_up_open` | upper dust gets 14 | yes |
| A conductive block above the lower dust cuts the diagonal | `diag_up_cut` | 0 | yes |
| Glass above the lower dust does not cut it | `diag_up_glass` | 14 | yes |
| Power steps down from dust on stone | `stone_down` | lower dust 14 | yes |
| Power does not step down from dust on glass | `glass_down` | lower dust 0 | yes |
| A weakly powered block does not power dust | `weak_dust` | 0 | yes |
| A weakly powered block drives a repeater | `weak_repeater` | output 15 | yes |
| A weakly powered block turns off a torch on it (the NOT gate) | `weak_torch` | lit=false | yes |
| A torch strongly powers the block above it and powers adjacent dust directly | `torch_strong_up` | 15 and 15 | yes |
| A torch does not power the solid block beside it | `torch_strong_up` | dust beyond that block: 0 | yes |
| Lone dust in the cross shape powers all four neighbours | `plus_cross` | all four lamps light | yes |
| Lone dust in the dot shape powers none of them | `plus_dot` | no lamp lights | yes |

## Placement and commands

| Fact | Fixture / probe | Measured |
|---|---|---|
| `setblock … strict` places exactly the written state, with no updates | `strict_keeps_shape` | A dust written with a wrong shape and power=0 beside a redstone block stays that way until the first neighbour update, which fixes shape and power at once. |
| `setblock` of a powered lever does **not** update the mounting block's neighbours | `lever_setblock` | Dust beside the block stays at 0. Drive inputs by placing a `redstone_block`, never by editing a lever's state. |
| A lever that is already powered strongly powers the block it stands on; removing it updates that block's neighbours | `lever_mount` | Placed with updates, the dust on both sides of the mounting block reads 15 at tick 0 (what the input tile relies on). `setblock air` on the powered lever drops both to 0 on the same tick, with nothing pending: the lever's removal, unlike its placement, updates the mounting block's neighbours. |
| `setblock minecraft:redstone_wire` with no properties gives a **dot**, not a cross | probe | The default block state is all `none`. (A player placing dust gets a cross; a command does not.) |
| Dust and torches can sit on glass | probe | Both survive a neighbour update. |
| `/tick step N` advances the game clock by exactly N | every run (asserted on every step) | Steps run asynchronously at the tick rate; `tick rate 10000` makes them near-instant. `/tick query` says "frozen" throughout, so completion is detected by polling `time query gametime`. |
| `/tick sprint N` runs N+1 ticks | probe | Not used. |
| The game clock does not advance while frozen | probe | 0 ticks in 1 s. |
| A command issued while frozen runs while the clock still reads the last completed tick | `torch_delay` | A torch scheduled by such a command fires 2 steps later, which is why the trace format defines a stimulus as the last event of its tick. |
| Saved chunk files omit block-state properties at their default value | probe | The harness restores them from the server's own registry (`harness/data/block_defaults.json`). |
| `save-all flush` writes only chunks with block changes; a newly scheduled block tick does not count | `repeater_delay_1` at the chunk border | The harness changes a marker block in every plot chunk before each save, so pending ticks are never stale. |
| The RCON server drops the connection if two requests arrive in one read | stage 0 | The client waits for a reply before sending its end marker. |
| 26.3 gamerule names | probe | snake_case: `advance_time`, `random_tick_speed`, `spawn_mobs`, … (the camelCase names are rejected). |

## Structure files (stage 2)

| Fact | Check | Measured |
|---|---|---|
| Format in 26.3 | built-in `igloo/top.nbt` from the server jar | Palette entries are `{id, properties}` (lowercase), not the older `{Name, Properties}`. `DataVersion` 5023. |
| Where `/place template ns:name` looks | `harness.place_template` | `<world>/generated/<ns>/structure/<name>.nbt`. The reply on success is `Loaded template "ns:name" at x, y, z`. |
| Strict placement | 5 recorded states | Reproduces the state exactly, and it stays unchanged for 20 ticks. |
| Normal placement (with updates) | same 5 states | Settles to the same state. |
| Size | 70×2×70 structure | Loads in full. The 48-block limit belongs to structure blocks, not to `/place template`. |

## Random circuits and races (stage 2)

`python -m harness.fuzz` builds random small circuits (dust, blocks, lamps, torches, repeaters, inverter chains, some fed back into themselves, and latches made of a repeater locked from its side), switches redstone blocks, and sometimes edits the circuit mid-run. Each is recorded in both modes at two placements.

There are two committed corpora of 200 circuits each: `fixtures/fuzz/` (seed 1, recorded before repeaters existed in the generator) and `fixtures/fuzz-repeaters/` (seed 2, with repeaters and latches; the simulator matches all 383 of its traces, 3 of them only with another order of same-moment events). The numbers below are for the first:

| | Experiments mode | Default mode |
|---|---|---|
| Same result at both placements | 197 | 197 |
| Result depends on placement (dropped) | 3 | 3 |

- **12 of 196 circuits (6%) behave differently in the two modes.** For those, only the experiments-mode recording is kept.
- **The game itself is order-dependent on some circuits.** When several scheduled ticks fall due together, the order they run in can change the result; so can the order in which one wire update reaches several neighbours. In default mode that order follows block position; in experiments mode part of it is chosen at random. The saved world does not record it.
- **The simulator matches all 381 corpus traces.** For 5 of them its default order of same-moment events gives a different result, and another order reproduces the game exactly; those circuits have a race. The check fails only when no order reproduces the game.
- Each of nine deliberately wrong rules (dust losing 2 per block, torch delay 1, and so on) makes the corpus or the mechanics fixtures fail.

Two game behaviours the editing stimuli exposed:

- A wire reshaped by a neighbour's removal can point into a block for a moment while it still holds its old power. A lamp that hears an update in that moment lights, then goes out 4 ticks later. Whether it hears one depends on delivery order.
- `setblock` of dust with no block underneath places it anyway, as a dot. redfab never emits unsupported dust or torches, and the simulator refuses them.

## Tiles (stage 5)

Every tile variant was run in the game with drivers and readers on its ports (`redfab tile-fixtures`), at three placements in both modes, and the recordings matched the simulator. Two things the tiles rely on that were measured along the way:

| Fact | Where it matters | Measured |
|---|---|---|
| A signal steps down onto a lower wire only if the block beside the lower wire (the upper wire's support) conducts | the crossing's bridge | Glass steps: no signal; stone steps: signal. Consistent with `glass_down`/`stone_down` in stage 1. |
| A repeater works on glass and is locked at placement if a powered repeater already points into its side | repeater tile, latches | yes |

## Flip-flops (stage 8)

Both candidate tiles were swept in the game and in the simulator (`redfab characterize`): the data edge moved one tick at a time against the clock edge, in both directions, at clock periods 16, 12 and 8, plus clock pulses of 2 to 8 ticks. The sweep traces are among the tile fixtures `tiles-vanilla` records, and the figures below are what the simulator reads off them, which must equal the `timing` line of the tile (`cells/dff.tile`, `cells/dff_alt.tile`). Game ticks after the active clock edge:

| | `dff` (master/slave locked repeaters, falling edge) | `dff_alt` (pulse latch, rising edge) |
|---|---|---|
| Captures data changes up to | +1 | +7 |
| Ignores them from | +3 | +9 |
| Setup / hold (with 2 ticks of margin) | 1 / 5 | −5 / 11 |
| Clock to Q | 8 | 8 |
| Minimum high / low | 4 / 4 | 4 / 2 |
| Minimum period | 12 | 16 |
| Ordering-dependent trials | the capture boundary (+2) only | +8 only |

`dff` is the tile every clocked design uses. The wiki's derivation for this design (DESIGN.md Appendix A) said clock-to-Q 6; measured, it is 8. A clock of period 8 burns the tile's torches out (eight turn-offs in sixty ticks), which is why the sweeps at period 8 end in a long quiet time.

## Still unmeasured

- A lever flipped by a player. Every input in the harness, the page and the live backend is driven by placing a `redstone_block` at the probe position, because `setblock` of a powered lever updates nothing (`lever_setblock`). `lever_mount` shows a powered lever strongly powering its mounting block (dust beside it reads 15) and its removal updating the neighbours; the flip itself, which in the game's code does the same neighbour update as the removal, has no recording.
- Litematica paste of `world.nbt`. No Minecraft client has run in this project; see README.

## Legacy wire order and lamp ticks (2026-10-08)

In the default redstone mode the wire algorithm's update order depends on world position. A lit, powered lamp whose feed dropped for exactly 4 game ticks and came back in the tick its turn-off fired can be handed a second turn-off tick, which fires with no effect. Seen on adder4 and divmod at two of three placements; never in the Experiments mode. The checks compare block states exactly in both modes and count such lamp ticks instead of failing on them (see DESIGN.md Appendix A).
