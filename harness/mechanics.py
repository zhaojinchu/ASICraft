"""The redstone-mechanics experiments: small circuits with documented expectations.

    python -m harness.mechanics record   # run on the real game, write fixtures/mechanics/*
    python -m harness.mechanics verify   # run again and compare with the committed recordings

Each fixture's expectations come from the Minecraft wiki or DESIGN.md Appendix A.
When the game disagrees, that is a finding to report, not something to paper over.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

from .blockstate import BlockState, Pos
from .runner import Runner, layout
from .server import ROOT, VanillaServer, read_versions
from .session import Session
from .trace import Expectation, Fixture, Trace, check_expectations, first_trace_difference

FIXTURE_DIR = ROOT / "fixtures" / "mechanics"
MODES = {"vanilla": False, "vanilla-rx": True}  # file suffix -> redstone_experiments

S = "minecraft:stone"
G = "minecraft:glass"
D = "minecraft:redstone_wire"
RB = "minecraft:redstone_block"
T = "minecraft:redstone_torch"
L = "minecraft:redstone_lamp"
AIR = "minecraft:air"


class Build:
    """Builds a Fixture from blocks at convenient coordinates; the result is shifted
    so its minimum corner is (0,0,0)."""

    def __init__(self, name: str, doc: str, ticks: int, placement: str = "update"):
        self.name, self.doc, self.ticks, self.placement = name, doc, ticks, placement
        self.blocks: dict[Pos, BlockState] = {}
        self.events: list[tuple[int, dict[Pos, BlockState]]] = []
        self.expect: list[Expectation] = []

    def put(self, pos: Pos, state: str) -> Build:
        self.blocks[pos] = BlockState.parse(state)
        return self

    def floor(self, x0: int, z0: int, x1: int, z1: int, y: int = 0, block: str = S) -> Build:
        for x in range(x0, x1 + 1):
            for z in range(z0, z1 + 1):
                self.put((x, y, z), block)
        return self

    def at(self, tick: int, pos: Pos, state: str) -> Build:
        for t, changes in self.events:
            if t == tick:
                changes[pos] = BlockState.parse(state)
                return self
        self.events.append((tick, {pos: BlockState.parse(state)}))
        return self

    def expects(self, tick: int, pos: Pos, state: str) -> Build:
        self.expect.append(Expectation(tick=tick, pos=pos, state=BlockState.parse(state)))
        return self

    def stable(self, from_tick: int) -> Build:
        self.expect.append(Expectation(tick=from_tick, stable=True))
        return self

    def build(self) -> Fixture:
        positions = list(self.blocks) + [p for _, c in self.events for p in c] + [e.pos for e in self.expect if e.pos]
        mins = tuple(min(p[i] for p in positions) for i in range(3))
        maxs = tuple(max(p[i] for p in positions) for i in range(3))
        shift = lambda p: (p[0] - mins[0], p[1] - mins[1], p[2] - mins[2])  # noqa: E731
        return Fixture(
            name=self.name,
            doc=self.doc,
            size=(maxs[0] - mins[0] + 1, maxs[1] - mins[1] + 1, maxs[2] - mins[2] + 1),
            placement=self.placement,
            blocks={shift(p): s for p, s in self.blocks.items()},
            events=[(t, {shift(p): s for p, s in c.items()}) for t, c in self.events],
            ticks=self.ticks,
            expect=[
                Expectation(e.tick, shift(e.pos), e.state) if e.pos else e for e in self.expect
            ],
        )


def inverter(b: Build, x: int = 0) -> Build:
    """Stimulus slot at (x,1,0); dust (x+1..x+2); torch on top of the block at x+3; output dust at (x+4,2,0)."""
    b.floor(x, 0, x + 4, 0)
    b.put((x + 1, 1, 0), D).put((x + 2, 1, 0), D).put((x + 3, 1, 0), S).put((x + 3, 2, 0), T)
    b.put((x + 4, 1, 0), S).put((x + 4, 2, 0), D)
    return b


def fixtures() -> list[Fixture]:
    out: list[Fixture] = []

    # -- calibration against documented delays and ranges
    b = Build("dust_decay", "A redstone block feeding 16 dust. Wiki: power drops by 1 per block; the 16th dust is unpowered.", 2)
    b.floor(0, 0, 16, 0).put((0, 1, 0), RB)
    for k in range(1, 17):
        b.put((k, 1, 0), D)
        b.expects(0, (k, 1, 0), f"{D}[power={max(0, 16 - k)}]")
    out.append(b.stable(0).build())

    b = Build("torch_delay", "Torch on a block powered through dust. Wiki: a torch changes state 2 game ticks after its input.", 16)
    inverter(b)
    b.at(2, (0, 1, 0), RB).at(10, (0, 1, 0), AIR)
    b.expects(0, (3, 2, 0), f"{T}[lit=true]").expects(0, (4, 2, 0), f"{D}[power=15]")
    b.expects(2, (1, 1, 0), f"{D}[power=15]").expects(3, (3, 2, 0), f"{T}[lit=true]")
    b.expects(4, (3, 2, 0), f"{T}[lit=false]").expects(4, (4, 2, 0), f"{D}[power=0]")
    b.expects(11, (3, 2, 0), f"{T}[lit=false]").expects(12, (3, 2, 0), f"{T}[lit=true]").expects(12, (4, 2, 0), f"{D}[power=15]")
    out.append(b.stable(12).build())

    for d in (1, 2, 3, 4):
        b = Build(f"repeater_delay_{d}", f"Repeater set to {d}. Wiki: delay is {2 * d} game ticks, on and off.", 14 + 2 * d + 2)
        b.floor(0, 0, 3, 0).put((1, 1, 0), D).put((2, 1, 0), f"minecraft:repeater[facing=west,delay={d}]").put((3, 1, 0), D)
        b.at(2, (0, 1, 0), RB).at(14, (0, 1, 0), AIR)
        b.expects(2, (1, 1, 0), f"{D}[power=15]")
        b.expects(2 + 2 * d - 1, (3, 1, 0), f"{D}[power=0]").expects(2 + 2 * d, (3, 1, 0), f"{D}[power=15]")
        b.expects(14 + 2 * d - 1, (3, 1, 0), f"{D}[power=15]").expects(14 + 2 * d, (3, 1, 0), f"{D}[power=0]")
        out.append(b.stable(14 + 2 * d).build())

    b = Build("lamp_delay", "Dust pointing into a lamp. Wiki: the lamp lights at once and goes out 4 game ticks after power is lost.", 16)
    b.floor(0, 0, 2, 0).put((1, 1, 0), D).put((2, 1, 0), L)
    b.at(2, (0, 1, 0), RB).at(10, (0, 1, 0), AIR)
    b.expects(1, (2, 1, 0), f"{L}[lit=false]").expects(2, (2, 1, 0), f"{L}[lit=true]")
    b.expects(13, (2, 1, 0), f"{L}[lit=true]").expects(14, (2, 1, 0), f"{L}[lit=false]")
    out.append(b.stable(14).build())

    # -- torch burnout: K turn-offs, one every 6 game ticks (all within 60 ticks)
    for k in (7, 8, 9, 10):
        burned = k >= 8
        b = Build(f"burnout_{k}", f"Inverter pulsed so its torch turns off {k} times in 60 game ticks. "
                  f"The wiki says burnout needs more than eight turn-offs in 60 ticks; the game (measured here) "
                  f"burns out on the eighth. So this torch should {'burn out, stay dark, and relight 160 ticks later' if burned else 'keep working'}.",
                  220)
        inverter(b)
        for i in range(k):
            b.at(2 + 6 * i, (0, 1, 0), RB).at(5 + 6 * i, (0, 1, 0), AIR)
        if burned:
            # 8th turn-off happens at tick 4 + 6*7 = 46; the relight attempt is scheduled 160 ticks later.
            b.expects(46, (3, 2, 0), f"{T}[lit=false]").expects(205, (3, 2, 0), f"{T}[lit=false]")
            b.expects(206, (3, 2, 0), f"{T}[lit=true]").stable(206)
        else:
            b.expects(120, (3, 2, 0), f"{T}[lit=true]").stable(46)
        out.append(b.build())

    # -- repeater locking when the lock and the input change on the same tick
    b = Build("lock_same_tick", "Data repeater R (facing west) and a lock repeater facing into R's side; both inputs rise on the same tick. "
              "Wiki: if the lock and the input change on the same tick, the repeater does not switch.", 20)
    b.floor(0, 0, 3, 3).put((1, 1, 0), D).put((2, 1, 0), "minecraft:repeater[facing=west,delay=1]").put((3, 1, 0), D)
    b.put((2, 1, 1), "minecraft:repeater[facing=south,delay=1]").put((2, 1, 2), D)
    b.at(2, (0, 1, 0), RB).at(2, (2, 1, 3), RB)
    b.expects(4, (2, 1, 0), "minecraft:repeater[locked=true]").expects(4, (3, 1, 0), f"{D}[power=0]")
    b.expects(20, (3, 1, 0), f"{D}[power=0]")
    out.append(b.stable(4).build())

    # -- does /setblock of a powered lever update the neighbours of its mounting block? (unknown; recorded only)
    b = Build("lever_setblock", "Lever on a block, dust beside the block. /setblock replaces the lever with a powered one. "
              "No documented expectation; the recording answers whether the dust lights.", 4)
    b.floor(0, 0, 2, 0).put((1, 1, 0), S).put((1, 2, 0), "minecraft:lever[face=floor,facing=north,powered=false]").put((2, 1, 0), D)
    b.at(2, (1, 2, 0), "minecraft:lever[face=floor,facing=north,powered=true]")
    out.append(b.build())

    # -- dust connectivity across heights
    def diag(name: str, doc: str, above_lower: str | None, expect_power: int) -> Fixture:
        b = Build(name, doc, 2)
        b.floor(0, 0, 1, 0).put((-1, 1, 0), RB).put((0, 1, 0), D).put((1, 1, 0), S).put((1, 2, 0), D)
        if above_lower:
            b.put((0, 2, 0), above_lower)
        return b.expects(0, (1, 2, 0), f"{D}[power={expect_power}]").stable(0).build()

    out.append(diag("diag_up_open", "Dust stepping up one block with air above the lower dust. Wiki: connects.", None, 14))
    out.append(diag("diag_up_cut", "Same, with a stone block above the lower dust. Wiki: a conductive block there cuts the connection.", S, 0))
    out.append(diag("diag_up_glass", "Same, with glass above the lower dust. Wiki: non-conductive blocks never cut.", G, 14))

    def down(name: str, doc: str, support: str, expect_power: int) -> Fixture:
        b = Build(name, doc, 2)
        b.put((0, 0, 0), S).put((0, 1, 0), D).put((1, 1, 0), support).put((1, 2, 0), D).put((2, 1, 0), S).put((2, 2, 0), RB)
        return b.expects(0, (0, 1, 0), f"{D}[power={expect_power}]").stable(0).build()

    out.append(down("stone_down", "Powered dust on stone, dust one block lower beside it. Wiki: power steps down.", S, 14))
    out.append(down("glass_down", "Powered dust on glass, dust one block lower beside it. Wiki (Java): the signal never goes down from a non-conductive block.", G, 0))

    # -- weak power
    b = Build("weak_dust", "Dust pointing into a stone block, dust on the far side. Wiki: a weakly powered block does not power dust.", 2)
    b.floor(0, 0, 3, 0).put((0, 1, 0), RB).put((1, 1, 0), D).put((2, 1, 0), S).put((3, 1, 0), D)
    out.append(b.expects(0, (3, 1, 0), f"{D}[power=0]").stable(0).build())

    b = Build("weak_repeater", "Dust pointing into a stone block, repeater taking input from that block. Wiki: a weakly powered block drives a repeater.", 2)
    b.floor(0, 0, 4, 0).put((0, 1, 0), RB).put((1, 1, 0), D).put((2, 1, 0), S)
    b.put((3, 1, 0), "minecraft:repeater[facing=west,delay=1]").put((4, 1, 0), D)
    out.append(b.expects(0, (4, 1, 0), f"{D}[power=15]").stable(0).build())

    b = Build("weak_torch", "Dust pointing into a stone block with a torch on its far face. Wiki: the torch turns off (this is the NOT gate).", 2)
    b.floor(0, 0, 4, 0).put((0, 1, 0), RB).put((1, 1, 0), D).put((2, 1, 0), S)
    b.put((3, 1, 0), "minecraft:redstone_wall_torch[facing=east]").put((4, 1, 0), D)
    out.append(b.expects(0, (3, 1, 0), "minecraft:redstone_wall_torch[lit=false]").expects(0, (4, 1, 0), f"{D}[power=0]").stable(0).build())

    # -- lone dust: cross vs dot (placed strictly, then the lamps are nudged so they re-read their input)
    def lone(name: str, doc: str, side: str, lit: str) -> Fixture:
        b = Build(name, doc, 4, placement="strict")
        b.put((1, 0, 0), RB).put((1, 1, 0), f"{D}[east={side},north={side},south={side},west={side},power=15]")
        for pos in ((2, 1, 0), (0, 1, 0), (1, 1, 1), (1, 1, -1)):
            b.put(pos, f"{L}[lit=false]")
            b.at(2, (pos[0], 2, pos[2]), G)
            b.expects(2, pos, f"{L}[lit={lit}]")
        return b.stable(2).build()

    out.append(lone("plus_cross", "Lone powered dust in the cross shape with a lamp on each side. Wiki: a cross powers all four directions.", "side", "true"))
    out.append(lone("plus_dot", "Lone powered dust in the dot shape with a lamp on each side. Wiki: a dot powers only the block beneath it.", "none", "false"))

    # -- strict placement keeps whatever shape is written until an update arrives
    b = Build("strict_keeps_shape", "Dust written with a wrong shape and power next to a redstone block, placed strictly. "
              "Wiki/plan: strict placement triggers no updates; the first update fixes shape and power.", 4, placement="strict")
    b.put((0, 0, 0), S).put((1, 0, 0), S).put((0, 1, 0), RB).put((1, 1, 0), f"{D}[east=none,north=up,south=none,west=none,power=0]")
    b.at(2, (1, 2, 0), G)
    b.expects(1, (1, 1, 0), f"{D}[east=none,north=up,power=0,south=none,west=none]")
    b.expects(2, (1, 1, 0), f"{D}[east=side,north=none,power=15,south=none,west=side]")
    out.append(b.stable(2).build())

    # -- which blocks a torch powers
    b = Build("torch_strong_up", "Torch with a stone block above it, dust beside the torch, and a stone block beside the torch with dust beyond. "
              "Wiki: the torch strongly powers the block above and powers adjacent dust; it does not power the block beside it.", 2)
    b.put((1, 0, 0), S).put((1, 1, 0), T).put((1, 2, 0), S).put((2, 1, 0), S).put((2, 2, 0), D)
    b.put((0, 0, 0), S).put((0, 1, 0), D).put((1, 1, 1), S).put((1, 0, 2), S).put((1, 1, 2), D)
    b.expects(0, (2, 2, 0), f"{D}[power=15]").expects(0, (0, 1, 0), f"{D}[power=15]").expects(0, (1, 1, 2), f"{D}[power=0]")
    out.append(b.stable(0).build())

    # -- repeaters (stage 4)
    R = "minecraft:repeater"

    b = Build("repeater_on_glass", "A repeater standing on glass, between two dust lines on glass. The tile library puts everything on glass, so this must work.", 12)
    b.floor(0, 0, 3, 0, block=G).put((1, 1, 0), D).put((2, 1, 0), f"{R}[facing=west,delay=1]").put((3, 1, 0), D)
    b.at(2, (0, 1, 0), RB).at(8, (0, 1, 0), AIR)
    b.expects(3, (3, 1, 0), f"{D}[power=0]").expects(4, (3, 1, 0), f"{D}[power=15]").expects(10, (3, 1, 0), f"{D}[power=0]")
    out.append(b.stable(10).build())

    b = Build("repeater_strong_front", "A repeater facing a stone block, with dust on the far side of the block. Wiki: a repeater strongly powers the block in front, and a strongly powered block powers dust.", 12)
    b.floor(0, 0, 4, 0).put((1, 1, 0), D).put((2, 1, 0), f"{R}[facing=west,delay=1]").put((3, 1, 0), S).put((4, 1, 0), D)
    b.at(2, (0, 1, 0), RB)
    b.expects(3, (4, 1, 0), f"{D}[power=0]").expects(4, (4, 1, 0), f"{D}[power=15]")
    out.append(b.stable(4).build())

    b = Build("repeater_side_dust", "Dust beside a repeater (at its side, not its front or back). Wiki: dust connects to a repeater only at its front and back, and a repeater powers only what is in front.", 12)
    b.floor(0, 0, 3, 1).put((1, 1, 0), D).put((2, 1, 0), f"{R}[facing=west,delay=1]").put((3, 1, 0), D).put((2, 1, 1), D)
    b.at(2, (0, 1, 0), RB)
    b.expects(4, (3, 1, 0), f"{D}[power=15]").expects(4, (2, 1, 1), f"{D}[power=0,north=none]").expects(12, (2, 1, 1), f"{D}[power=0]")
    out.append(b.stable(4).build())

    b = Build("repeater_pulse_stretch", "A 2-tick pulse into a repeater set to 3 (6 game ticks). Wiki: a repeater stretches an on-pulse shorter than its delay to the length of its delay.", 24)
    b.floor(0, 0, 3, 0).put((1, 1, 0), D).put((2, 1, 0), f"{R}[facing=west,delay=3]").put((3, 1, 0), D)
    b.at(2, (0, 1, 0), RB).at(4, (0, 1, 0), AIR)
    b.expects(7, (3, 1, 0), f"{D}[power=0]").expects(8, (3, 1, 0), f"{D}[power=15]")
    b.expects(13, (3, 1, 0), f"{D}[power=15]").expects(14, (3, 1, 0), f"{D}[power=0]")
    out.append(b.stable(14).build())

    b = Build("lock_hold_release", "A data repeater with a lock repeater facing into its side. Wiki: while locked it keeps its output whatever its input does; when released it follows its input again after its delay.", 40)
    b.floor(0, 0, 3, 3).put((1, 1, 0), D).put((2, 1, 0), f"{R}[facing=west,delay=1]").put((3, 1, 0), D)
    b.put((2, 1, 1), f"{R}[facing=south,delay=1]").put((2, 1, 2), D)
    data, lock = (0, 1, 0), (2, 1, 3)
    b.at(2, lock, RB)      # lock engages at 4 with the data repeater off
    b.at(8, data, RB)      # input rises while locked: no change
    b.at(16, lock, AIR)    # lock releases at 18; the repeater then turns on at 20
    b.at(24, lock, RB)     # lock again at 26 with the repeater on
    b.at(28, data, AIR)    # input falls while locked: no change
    b.expects(4, (2, 1, 0), f"{R}[locked=true,powered=false]").expects(14, (3, 1, 0), f"{D}[power=0]")
    b.expects(18, (2, 1, 0), f"{R}[locked=false]").expects(19, (3, 1, 0), f"{D}[power=0]").expects(20, (3, 1, 0), f"{D}[power=15]")
    b.expects(26, (2, 1, 0), f"{R}[locked=true,powered=true]").expects(40, (3, 1, 0), f"{D}[power=15]")
    out.append(b.stable(28).build())

    names = [f.name for f in out]
    assert len(names) == len(set(names)), "duplicate fixture names"
    return out


# ---------------------------------------------------------------- running

def source_for(server: VanillaServer) -> dict:
    pins = read_versions()
    return {"kind": "vanilla", "mc_version": pins["MC_VERSION"], "jar_sha1": pins["MC_SERVER_SHA1"],
            "redstone_experiments": server.redstone_experiments}


def run_mode(mode: str, fxs: list[Fixture], log=print, idle_check: bool = True) -> tuple[dict[tuple[str, str], Trace], list[str]]:
    """Run every fixture in one server of the given mode. Returns traces by (fixture, variant)
    and a list of harness-level problems (not expectation failures)."""
    problems: list[str] = []
    server = VanillaServer(f"mechanics-{mode}", redstone_experiments=MODES[mode])
    with server:
        log(f"[{mode}] server ready in {server.startup_seconds:.1f}s")
        session = Session(server)
        session.prepare()
        started = time.monotonic()
        traces = Runner(session, source_for(server)).run(layout(fxs), log=lambda m: log(f"[{mode}] {m}"))
        # The server must not pause while idle (no players ever join). Make sure the
        # run outlasted the default 60 s pause delay, then confirm ticking still works.
        remaining = 66 - (time.monotonic() - started)
        if idle_check and remaining > 0:
            log(f"[{mode}] waiting {remaining:.0f}s to prove the idle pause is off")
            time.sleep(remaining)
        try:
            session.step(1)
        except Exception as e:  # noqa: BLE001
            problems.append(f"[{mode}] step after idling failed: {e}")
        log_text = (server.run_dir / "server.log").read_text(errors="replace")
        if "pausing" in log_text:
            problems.append(f"[{mode}] server log reports an idle pause")
    return traces, problems


def trace_path(name: str, mode: str) -> Path:
    return FIXTURE_DIR / f"{name}.{mode}.trace.json"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("action", choices=["record", "verify", "write-fixtures"])
    ap.add_argument("--only", help="run just this fixture")
    ap.add_argument("--mode", choices=[*MODES, "both"], default="both")
    ap.add_argument("--break-expectation", action="store_true",
                    help="negative control: corrupt one expectation; verify must then fail")
    args = ap.parse_args(argv)

    fxs = fixtures()
    if args.only:
        fxs = [f for f in fxs if f.name == args.only]
        if not fxs:
            print(f"no fixture named {args.only}", file=sys.stderr)
            return 2
    if args.break_expectation:
        target = next(e for e in fxs[0].expect if e.pos is not None)
        key, value = target.state.props[0]
        wrong = {"true": "false", "false": "true"}.get(value, str((int(value) + 1) % 16) if value.isdigit() else value + "x")
        fxs[0].expect[fxs[0].expect.index(target)] = Expectation(target.tick, target.pos, target.state.with_props(**{key: wrong}))
        print(f"negative control: expecting {key}={wrong} instead of {value} at tick {target.tick} of {fxs[0].name}")

    FIXTURE_DIR.mkdir(parents=True, exist_ok=True)
    if args.action == "write-fixtures":
        for fx in fxs:
            (FIXTURE_DIR / f"{fx.name}.fixture.json").write_text(json.dumps(fx.to_json(), indent=1) + "\n")
        print(f"wrote {len(fxs)} fixtures")
        return 0

    modes = list(MODES) if args.mode == "both" else [args.mode]
    failures: list[str] = []
    for mode in modes:
        traces, problems = run_mode(mode, fxs, idle_check=not args.only)
        failures.extend(problems)
        for fx in fxs:
            variants = {v: t for (n, v), t in traces.items() if n == fx.name}
            base = variants["aligned"]
            for variant, trace in variants.items():
                if variant != "aligned" and (diff := first_trace_difference(base, trace)) is not None:
                    failures.append(f"[{mode}] {fx.name}: '{variant}' placement differs from 'aligned': {diff}")
            for failure in check_expectations(base, fx.expect):
                failures.append(f"[{mode}] {fx.name}: {failure}")
            path = trace_path(fx.name, mode)
            if args.action == "record":
                path.write_text(json.dumps(base.to_json(), indent=1) + "\n")
                (FIXTURE_DIR / f"{fx.name}.fixture.json").write_text(json.dumps(fx.to_json(), indent=1) + "\n")
            elif not path.exists():
                failures.append(f"[{mode}] {fx.name}: no recorded trace at {path}; run `record`")
            else:
                committed = Trace.from_json(json.loads(path.read_text()), fx.ticks)
                if (diff := first_trace_difference(committed, base)) is not None:
                    failures.append(f"[{mode}] {fx.name}: differs from the committed recording: {diff}")
        print(f"[{mode}] {len(fxs)} fixtures, {sum(1 for f in failures if f.startswith(f'[{mode}]'))} problems so far")

    print()
    if failures:
        print(f"{len(failures)} problem(s):")
        for f in failures:
            print("  " + f)
        return 1
    print(f"all {len(fxs)} fixtures agree with the recordings and their expectations in modes {modes}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
