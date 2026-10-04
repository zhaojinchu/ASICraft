# Redstone mechanics, as measured

What the real game does, measured with the harness on the pinned server (Minecraft 26.3, jar `33680f5f…`) on 2026-10-04, in both the default redstone mode and the Redstone Experiments mode. Every row has a fixture in `fixtures/mechanics/` whose recording is committed and re-checked by `./check --vanilla`. **All 25 fixtures behave identically in both modes.**

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

**Design rule:** a torch may turn off at most **7 times in any 60-gt window**. redfab's burnout budget check uses 7.

## Repeater locking

| Fact | Fixture | Measured | Matches the wiki? |
|---|---|---|---|
| Lock and input changing on the same tick | `lock_same_tick` | Both repeaters schedule for +2. The lock repeater (priority −3, because it faces into another repeater) runs first; the data repeater (priority −1) is locked when its own tick comes and does not switch. Output stays 0 for the whole run. | yes |
| Scheduled-tick priorities | `lock_same_tick`, saved `block_ticks` | repeater facing a diode: −3; other repeater: −1; torch: 0 | yes |

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

## Still unmeasured

- `/place template` with a structure larger than 48³ (needs the `.nbt` writer, stage 2).
- Setup, hold and clock-to-output of the flip-flop designs (stage 8).
- Any circuit where the two redstone modes disagree. None of these fixtures did; update-order differences need multi-path dust networks, which stage 2's random fixtures will cover.
