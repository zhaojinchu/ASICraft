"""Random small redstone arrangements, recorded from the game and replayed in the simulator.

    python -m harness.fuzz record --seed 1 --count 150   # write the committed corpus (fixtures/fuzz/)
    python -m harness.fuzz verify --count 40             # fresh random circuits, game vs simulator

Each random fixture is run in both redstone modes at two placements. A trace is
kept only if the game gave the same result at both placements, because the game
itself is order-dependent on some arrangements (by position in default mode, by
a random choice in experiments mode) and no fixed model can match those.
The simulator follows the experiments-mode rules, so those traces must match
exactly. Default-mode traces are stored only when they equal the experiments
ones; how often they differ is reported.
"""

from __future__ import annotations

import argparse
import json
import random
import shutil
import subprocess
import sys
from pathlib import Path

from .mechanics import AIR, D, G, L, MODES, RB, S, T, Build, source_for
from .runner import Runner, layout
from .server import ROOT, VanillaServer, cargo, seed_argument
from .session import Session
from .trace import Fixture, Trace, first_trace_difference

CORPUS_DIR = ROOT / "fixtures" / "fuzz"
# The gate's `verify` seed is fixed so a run tests the same circuits as the last one;
# 1 and 2 made the committed corpora, so this draws fresh ones.
VERIFY_SEED = 3
FUZZ_TICKS = 60
LONG_TICKS = 230   # long enough to see a burned-out torch relight (160 ticks later)
FULL = (S, G, L)
HORIZONTAL = {"east": (1, 0), "west": (-1, 0), "south": (0, 1), "north": (0, -1)}


def random_fixture(rng: random.Random, name: str, ticks: int = FUZZ_TICKS) -> Fixture:
    """A floor with dust, blocks, lamps and torches on one or two levels, usually a
    chain of inverters (sometimes fed back into itself), a few redstone blocks,
    and redstone blocks that appear and disappear over time."""
    b = Build(name, "random arrangement", ticks)
    sx, sz = rng.randint(5, 11), rng.randint(3, 6)
    cells: dict[tuple[int, int, int], str | None] = {}  # None reserves a cell as air
    forced_slots: list[tuple[int, int, int]] = []

    def pick(table: list[tuple[float, str | None]]) -> str | None:
        r = rng.random()
        for p, value in table:
            if r < p:
                return value
            r -= p
        return None

    # -- an inverter chain along one row: slot, dust, then (block, torch, dust) one to three times
    inverters = rng.randint(1, (sx - 2) // 3)
    if rng.random() < 0.8:
        z0 = rng.randrange(sz)
        cells[(0, 0, z0)] = S
        cells[(0, 1, z0)] = None
        cells[(0, 2, z0)] = None
        forced_slots.append((0, 1, z0))
        cells[(1, 0, z0)] = S
        cells[(1, 1, z0)] = D
        x = 2
        for _ in range(inverters):
            for dx in range(3):
                cells[(x + dx, 0, z0)] = S
            cells[(x, 1, z0)] = S
            if rng.random() < 0.5:   # torch on the far side of the block
                cells[(x + 1, 1, z0)] = "minecraft:redstone_wall_torch[facing=east]"
                cells[(x + 2, 1, z0)] = D
            else:                    # torch on top, output dust one level up, stepping back down
                cells[(x, 2, z0)] = T
                cells[(x + 1, 1, z0)] = S
                cells[(x + 1, 2, z0)] = D
                cells[(x + 2, 1, z0)] = D
                cells[(x + 2, 2, z0)] = None
            x += 3
        if rng.random() < 0.4 and sz >= 2:  # feed the output back to the input along the next row
            z1 = z0 + 1 if z0 + 1 < sz else z0 - 1
            for fx in range(1, x):
                cells.setdefault((fx, 0, z1), S)
                cells.setdefault((fx, 1, z1), D)

    def repeater(facing: str | None = None, delay: int | None = None) -> str:
        facing = facing or rng.choice(list(HORIZONTAL))
        return f"minecraft:repeater[facing={facing},delay={delay or rng.randint(1, 4)}]"

    # -- a latch: a data repeater in a wire, with a second repeater pointing into its side
    #    slot, dust, data repeater, dust        (row z)
    #                lock repeater              (row z+1, fed straight from a slot in row z+2)
    if rng.random() < 0.5 and sz >= 3:
        z = rng.randrange(sz - 2)
        x = rng.randrange(sx - 3)
        spots = [(x + dx, y, z + dz) for dx in range(4) for dz in range(3) for y in (0, 1, 2)]
        if not any(spot in cells for spot in spots):
            for dx in range(4):
                cells[(x + dx, 0, z)] = S
                cells[(x + dx, 2, z)] = None
            cells[(x, 1, z)] = None
            cells[(x + 1, 1, z)] = D
            cells[(x + 2, 1, z)] = repeater("west")
            cells[(x + 3, 1, z)] = D
            cells[(x + 2, 0, z + 1)] = S
            cells[(x + 2, 1, z + 1)] = repeater("south")
            cells[(x + 2, 2, z + 1)] = None
            cells[(x + 2, 1, z + 2)] = None
            cells[(x + 2, 2, z + 2)] = None
            forced_slots += [(x, 1, z), (x + 2, 1, z + 2)]

    # -- random filling around it
    density = rng.choice([0.3, 0.6, 1.0])
    torchy = rng.choice([0.5, 1.0, 2.0, 3.0])
    repeaters = rng.choice([0.0, 0.5, 1.0, 2.0])
    for x in range(sx):
        for z in range(sz):
            if (x, 0, z) not in cells:
                floor = pick([(0.75, S), (0.15, G)])
                if floor:
                    cells[(x, 0, z)] = floor
    for x in range(sx):
        for z in range(sz):
            if (x, 1, z) not in cells and cells.get((x, 0, z)) in FULL:
                block = pick([(0.42 * density, D), (0.16 * density, S), (0.04 * density, G), (0.07 * density, L),
                              (0.03 * torchy * density, T), (0.05 * repeaters * density, repeater())])
                if block:
                    cells[(x, 1, z)] = block
    for x in range(sx):
        for z in range(sz):
            if (x, 2, z) in cells:
                continue
            below = cells.get((x, 1, z))
            if below in FULL:
                block = pick([(0.40 * density, D), (min(0.45, 0.15 * torchy) * density, T), (0.04 * density, L),
                              (0.04 * density, S), (0.05 * repeaters * density, repeater())])
                if block:
                    cells[(x, 2, z)] = block
            elif below in (D, None) and rng.random() < 0.06 * density:
                cells[(x, 2, z)] = rng.choice([S, G])  # a block hanging over a wire or a gap
    for y in (1, 2):  # torches on the sides of full blocks
        for x in range(sx):
            for z in range(sz):
                if (x, y, z) in cells or rng.random() >= min(0.6, 0.12 * torchy) * density:
                    continue
                walls = [f for f, (dx, dz) in HORIZONTAL.items() if cells.get((x - dx, y, z - dz)) in FULL]
                if walls:
                    cells[(x, y, z)] = f"minecraft:redstone_wall_torch[facing={rng.choice(walls)}]"

    # -- places a redstone block can come and go: air, with air above (nothing rests on it)
    def is_air(pos: tuple[int, int, int]) -> bool:
        return cells.get(pos) is None

    free = [
        (x, y, z)
        for x in range(sx) for z in range(sz) for y in (1, 2)
        if is_air((x, y, z)) and is_air((x, y + 1, z)) and (x, y, z) not in forced_slots
    ]
    rng.shuffle(free)

    def beside_dust(pos: tuple[int, int, int]) -> bool:
        return any(cells.get((pos[0] + dx, pos[1], pos[2] + dz)) == D for dx, dz in HORIZONTAL.values())

    free.sort(key=lambda pos: not beside_dust(pos))  # prefer places that drive something
    used = {(p[0], p[2]) for p in forced_slots}
    slots = []
    for pos in free:
        if (pos[0], pos[2]) not in used:  # one slot per column, so slots never stack
            used.add((pos[0], pos[2]))
            slots.append(pos)
    always_on = slots[: rng.randint(0, 2)]
    switched = forced_slots + slots[len(always_on): len(always_on) + rng.randint(0, 2)]
    for pos in always_on:
        cells[pos] = RB
    for pos, block in cells.items():
        if block is not None:
            b.put(pos, block)
    # -- stimuli: switch the redstone blocks, and sometimes edit the circuit itself
    state = {pos: False for pos in switched}
    occupied = {pos for pos, block in cells.items() if block is not None} | set(switched)
    edits: list[tuple[int, int, int]] = []  # blocks added by edits, removable again
    edit_rate = rng.choice([0.0, 0.2, 0.5])
    last = min(ticks - 10, 46)
    for tick in sorted(rng.sample(range(1, last), rng.randint(4, 12))):
        if switched and rng.random() >= edit_rate:
            pos = rng.choice(switched)
            state[pos] = not state[pos]
            b.at(tick, pos, RB if state[pos] else AIR)
            continue
        removable = [p for p in edits if (p[0], p[1] + 1, p[2]) not in occupied]
        if removable and rng.random() < 0.4:
            pos = rng.choice(removable)
            edits.remove(pos)
            occupied.discard(pos)
            cells[pos] = None  # air again, so nothing is later placed "on" it
            b.at(tick, pos, AIR)
            continue
        # add a block in an empty cell; dust and torches need a full block underneath
        spots = [
            (x, y, z)
            for x in range(sx) for z in range(sz) for y in (1, 2)
            if (x, y, z) not in occupied and (x, y + 1, z) not in occupied
        ]
        if not spots:
            continue
        pos = rng.choice(spots)
        below_pos = (pos[0], pos[1] - 1, pos[2])
        # Supported only by a block that is there for the whole run.
        supported = cells.get(below_pos) in FULL and below_pos not in edits
        block = rng.choice([D, D, T, repeater(), S, G, L, RB] if supported else [S, G, L, RB])
        occupied.add(pos)
        edits.append(pos)
        cells[pos] = block
        b.at(tick, pos, block)
    return b.build()


def consistent_traces(mode: str, fxs: list[Fixture], traces: dict[tuple[str, str], Trace],
                      log=print) -> tuple[dict[str, Trace], dict[str, int]]:
    """The aligned trace of every fixture whose rotated placement gave the same trace.
    A fixture the game ran differently at the two placements is placement-dependent:
    it is dropped, counted, and named in the log so the arrangement can be looked at."""
    stats = {"placement_dependent": 0}
    keep: dict[str, Trace] = {}
    for fx in fxs:
        a, r = traces[(fx.name, "aligned")], traces[(fx.name, "rotated")]
        if (diff := first_trace_difference(a, r)) is not None:
            stats["placement_dependent"] += 1
            log(f"[{mode}] {fx.name}: placement-dependent, dropped ({diff})")
            continue
        keep[fx.name] = a
    return keep, stats


def run_mode(mode: str, fxs: list[Fixture], log=print) -> tuple[dict[str, Trace], dict[str, int]]:
    """Record every fixture at two placements. Returns the traces the game was
    consistent about, and counts of what was dropped."""
    with VanillaServer(f"fuzz-{mode}", redstone_experiments=MODES[mode]) as server:
        session = Session(server)
        session.prepare()
        runner = Runner(session, source_for(server))
        # No settling: tick 0 is the moment after placement, before any torch has
        # ticked, so the recorded state plus its pending ticks is the whole story
        # (no burnout history to guess).
        traces = runner.run(layout(fxs, variants=("aligned", "rotated")), log=lambda m: log(f"[{mode}] {m}"),
                            settle_ticks=0, allow_pending_at_start=True)
    return consistent_traces(mode, fxs, traces, log)


def record(fxs: list[Fixture], out: Path, log=print) -> dict:
    """Run both modes and write fixtures and traces into `out`. Returns statistics."""
    out.mkdir(parents=True, exist_ok=True)
    rx, rx_stats = run_mode("vanilla-rx", fxs, log)
    default, default_stats = run_mode("vanilla", fxs, log)
    same_in_both = 0
    for fx in fxs:
        if fx.name not in rx:
            continue
        compact = {"separators": (",", ":")}  # hundreds of machine-read files; keep them small
        (out / f"{fx.name}.fixture.json").write_text(json.dumps(fx.to_json(), **compact) + "\n")
        (out / f"{fx.name}.vanilla-rx.trace.json").write_text(json.dumps(rx[fx.name].to_json(), **compact) + "\n")
        if fx.name in default and first_trace_difference(rx[fx.name], default[fx.name]) is None:
            same_in_both += 1
            (out / f"{fx.name}.vanilla.trace.json").write_text(json.dumps(default[fx.name].to_json(), **compact) + "\n")
    return {
        "generated": len(fxs),
        "experiments_mode": {"kept": len(rx), **rx_stats},
        "default_mode": {"kept": len(default), **default_stats},
        "kept_in_both_and_identical": same_in_both,
        "kept_in_both_but_different": sum(1 for n in rx if n in default) - same_in_both,
    }


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    """The command line, with `seed` resolved: a fixed one unless `--seed` says otherwise,
    so a gate run tests what the last one did."""
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("action", choices=["record", "verify", "show"])
    ap.add_argument("--seed", type=seed_argument,
                    help=f"an integer, or `random` for one from the clock (default: {VERIFY_SEED} for verify, 1 otherwise)")
    ap.add_argument("--count", type=int, default=40)
    ap.add_argument("--long", type=int, default=0, help="how many of the fixtures run for 230 ticks instead of 60")
    ap.add_argument("--out", type=Path, help="where `record` writes the corpus (default: fixtures/fuzz)")
    args = ap.parse_args(argv)
    if args.seed is None:
        args.seed = VERIFY_SEED if args.action == "verify" else 1
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    seed = args.seed

    if args.action == "show":
        print(json.dumps(random_fixture(random.Random(seed), "fz_show").to_json(), indent=1))
        return 0

    rng = random.Random(seed)
    fxs = [random_fixture(rng, f"fz_{seed}_{i:03d}", LONG_TICKS if i < args.long else FUZZ_TICKS) for i in range(args.count)]
    out = (args.out or CORPUS_DIR) if args.action == "record" else ROOT / "build" / "fuzz" / str(seed)
    if out.exists() and args.action == "verify":
        shutil.rmtree(out)
    stats = record(fxs, out)
    print(f"seed {seed}: {json.dumps(stats)}")
    if args.action == "record":
        (out / "STATS.json").write_text(json.dumps({"seed": seed, **stats}, indent=1) + "\n")
        return 0

    result = subprocess.run([cargo(), "run", "-q", "-p", "redfab-cli", "--", "check-traces", str(out)],
                            cwd=ROOT, capture_output=True, text=True)
    print(result.stdout.strip())
    if result.returncode != 0:
        print(result.stderr.strip())
        print(f"simulator disagrees with the game; reproduce with: python -m harness.fuzz verify --seed {seed} --count {args.count}")
        return 1
    if stats["experiments_mode"]["kept"] < args.count // 2:
        print("fewer than half of the random circuits were usable; the generator needs attention")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
