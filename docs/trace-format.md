# Trace format

One JSON format for "what a redstone circuit did, tick by tick". It is produced by the vanilla harness (recorded from the real game), by the simulator in `redfab-core`, and later by the server mod's live stream, so any two of them can be compared directly.

## Conventions

- **Units are game ticks** (20 per second). Redstone ticks are never used in files.
- **Positions** are integer block coordinates in Minecraft's axes (+X east, +Y up, +Z south), written as the string `"x,y,z"`. In a fixture they are relative to the plot origin; `origin` is added when placing in the world and subtracted when recording.
- **Block states** are written in command syntax with properties sorted by name: `minecraft:redstone_wire[east=side,north=none,power=15,south=none,west=side]`. Blocks without properties have no brackets: `minecraft:stone`. Air is written `minecraft:air`.
- **Tick numbering.** Tick 0 is the settled state after placement, before anything has been stepped. "Tick t" means the state after the t-th step. Stimuli listed for tick t are applied *after* the state of tick t−1 was recorded and *before* step t runs, so their synchronous effects (dust power, lamp turn-on) show up at tick t together with that step's scheduled ticks.

## Fixture file (`*.fixture.json`)

What to build and what to do to it.

```json
{
  "format": "redfab-fixture/1",
  "name": "repeater_delay_2",
  "doc": "A 2-tick repeater between two dust lines. Expect the output 4 game ticks after the input.",
  "size": [9, 3, 3],
  "placement": "update",
  "blocks": { "0,0,0": "minecraft:stone", "0,1,0": "minecraft:redstone_wire", "...": "..." },
  "events": [ { "tick": 5, "set": { "0,1,1": "minecraft:redstone_block" } } ],
  "ticks": 40,
  "expect": [
    { "tick": 9, "pos": "6,1,0", "state": "minecraft:redstone_wire[power=15]" },
    { "from": 12, "stable": true }
  ]
}
```

- `size`: the plot's bounding box; everything in it is recorded.
- `placement`: `"update"` places blocks normally and lets the game settle them (shapes, power, pending ticks run out) before tick 0; `"strict"` places them exactly as written with no updates. Blocks in `blocks` may omit properties under `"update"`.
- `events`: stimuli, applied with normal (update-triggering) `setblock` at the start of the given tick.
- `expect`: checks evaluated against a recorded trace. `state` matches if every property it names has the given value (so `redstone_wire[power=15]` ignores the shape). `{"from": t, "stable": true}` means nothing changes and nothing is pending from tick t to the end.

## Trace file (`*.trace.json`)

What happened.

```json
{
  "format": "redfab-trace/1",
  "fixture": "repeater_delay_2",
  "source": { "kind": "vanilla", "mc_version": "26.3", "jar_sha1": "33680f5f…", "redstone_experiments": false },
  "size": [9, 3, 3],
  "initial": { "0,0,0": "minecraft:stone", "0,1,0": "minecraft:redstone_wire[east=side,north=none,power=0,south=none,west=none]" },
  "pending0": [],
  "ticks": [
    { "tick": 5, "changes": { "0,1,1": "minecraft:redstone_block", "0,1,0": "minecraft:redstone_wire[…power=15…]" },
      "pending": [ { "pos": "2,1,0", "block": "minecraft:repeater", "delay": 4, "priority": -1 } ] }
  ]
}
```

- `initial`: every non-air block in the box at tick 0.
- `ticks`: one entry per tick from 1 to the fixture's `ticks`, **only when something changed or something is pending**; a missing tick means "same as the previous tick, nothing pending". `changes` holds the full new state of every block that differs from the previous tick (`minecraft:air` when a block disappeared).
- `pending`: scheduled block ticks inside the box after this tick, with `delay` in ticks remaining and the game's `priority`. Sources that cannot report pending ticks omit the field.
- `source.kind` is `"vanilla"`, `"sim"` or `"live"`.

Two traces of the same fixture are **equal** when `initial` and the per-tick block states are equal for every tick; `pending` is compared only when both sides provide it.
