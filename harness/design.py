"""Checks built designs (build/designs/<name>/) against Verilator and against the game.

    python -m harness.design sim NAME...              simulator vs Verilator, every input combination
    python -m harness.design lvs-negative NAME        broken layouts must fail the netlist check
    python -m harness.design vanilla NAME...          the game: function, traces vs simulator, placement
    python -m harness.design vanilla-negative NAME    broken layouts must fail in the game
    python -m harness.design exhaustive NAME          the game: every input combination, outputs only
    python -m harness.design timing-traces NAME...    recorded game traces vs the design's timing report
    python -m harness.design cycles NAME...           clocked designs: simulator vs Verilator for 64 cycles
    python -m harness.design cycles-vanilla NAME...   the same in the game, at three placements, both modes
    python -m harness.design hold-negative NAME       a hold-broken copy is flagged and misbehaves
    python -m harness.design burnout-negative NAME    driving a design too fast is flagged and burns out

Build the designs first with `redfab build designs/<name>/top.sv -o build/designs/<name>`.
"""

from __future__ import annotations

import argparse
import itertools
import json
import random
import re
import shutil
import subprocess
import sys
from pathlib import Path

from .blockstate import BlockState, format_pos, parse_pos
from .mechanics import MODES, source_for
from .place_template import place, read_box
from .runner import Runner, layout
from .server import ROOT, VanillaServer, cargo
from .session import Session
from .trace import Expectation, Fixture, Trace, first_trace_difference

DESIGNS = ROOT / "build" / "designs"
MAX_INPUT_BITS = 10  # all combinations are tried up to this many input bits
FIRST_VECTOR_TICK = 2
# A full trace records every block every tick, which is slow: the `vanilla` check
# runs at most this many vectors of a design (a fixed sample when there are more),
# and fewer, down to two, when a design's settle time would make the trace longer
# than MAX_TRACED_TICKS. The `exhaustive` check covers every vector without traces.
MAX_TRACED_VECTORS = 32
MAX_TRACED_TICKS = 300
TEMPLATE_Z = 8192  # where the template checks place their copies, clear of the traced plots

DUST_NS = "minecraft:redstone_wire[east=none,north=side,power=0,south=side,west=none]"


def redfab(*args: str, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run([cargo(), "run", "-q", "-p", "redfab-cli", "--", *args], cwd=ROOT, check=check,
                          capture_output=True, text=True)


def load(name: str) -> tuple[dict, dict]:
    """The built design file and the top module of its mapped netlist."""
    directory = DESIGNS / name
    if not (directory / "design.json").exists():
        raise SystemExit(f"{directory} has no design.json; run `redfab build` first")
    design = json.loads((directory / "design.json").read_text())
    module = json.loads((directory / "mapped.json").read_text())["modules"]["top"]
    return design, module


# ---------------------------------------------------------------- expected values

def ports(module: dict) -> tuple[list[tuple[str, int]], list[tuple[str, int]]]:
    ins, outs = [], []
    for name, port in module["ports"].items():
        (ins if port["direction"] == "input" else outs).append((name, len(port["bits"])))
    return ins, outs


def bit_names(name: str, width: int) -> list[str]:
    return [name] if width == 1 else [f"{name}[{i}]" for i in range(width)]


def all_vectors(module: dict) -> list[dict[str, int]]:
    ins, _ = ports(module)
    names = [bit for name, width in ins for bit in bit_names(name, width)]
    if len(names) > MAX_INPUT_BITS:
        raise SystemExit(f"{len(names)} input bits: too many to try every combination")
    return [dict(zip(names, values)) for values in itertools.product((0, 1), repeat=len(names))]


def sampled_vectors(module: dict, limit: int) -> list[dict[str, int]]:
    """All vectors when there are at most `limit`, otherwise `limit` of them spread
    evenly from the first (all off) to the last (all on), the same every run."""
    vectors = all_vectors(module)
    if len(vectors) <= limit:
        return vectors
    return [vectors[round(i * (len(vectors) - 1) / (limit - 1))] for i in range(limit)]


def verilator_outputs(name: str, module: dict, vectors: list[dict[str, int]]) -> list[dict[str, int]]:
    """What the source design computes for each vector, according to Verilator."""
    ins, outs = ports(module)
    directory = DESIGNS / name / "verilator"
    shutil.rmtree(directory, ignore_errors=True)
    directory.mkdir(parents=True)
    lines = ["module tb;"]
    lines += [f"  logic [{width - 1}:0] {port};" for port, width in ins + outs]
    lines.append("  top dut (.*);")
    lines.append("  initial begin")
    for vector in vectors:
        for port, width in ins:
            value = sum(vector[bit] << i for i, bit in enumerate(bit_names(port, width)))
            lines.append(f"    {port} = {width}'d{value};")
        shown = " ".join(f"{port}=%b" for port, _ in outs)
        lines.append(f'    #1; $display("OUT {shown}", {", ".join(port for port, _ in outs)});')
    lines += ["    $finish;", "  end", "endmodule"]
    (directory / "tb.sv").write_text("\n".join(lines) + "\n")
    subprocess.run(
        ["verilator", "--binary", "--timing", "-Wno-fatal", "--Mdir", str(directory / "obj"), "--top-module", "tb",
         str(DESIGNS / name / "top.sv"), str(directory / "tb.sv")],
        check=True, capture_output=True, text=True,
    )
    output = subprocess.run([str(directory / "obj" / "Vtb")], check=True, capture_output=True, text=True).stdout
    results = []
    for line in output.splitlines():
        if not line.startswith("OUT "):
            continue
        row = {}
        for port, bits in re.findall(r"(\w+)=([01xz]+)", line):
            width = dict(outs)[port]
            for i, bit in enumerate(bit_names(port, width)):
                row[bit] = int(bits[len(bits) - 1 - i])
        results.append(row)
    if len(results) != len(vectors):
        raise SystemExit(f"{name}: Verilator printed {len(results)} results for {len(vectors)} vectors")
    return results


# ---------------------------------------------------------------- simulator

def cmd_sim(names: list[str]) -> int:
    failures = []
    for name in names:
        design, module = load(name)
        vectors = all_vectors(module)
        expected = verilator_outputs(name, module, vectors)
        vectors_file = DESIGNS / name / "vectors.json"
        vectors_file.write_text(json.dumps(vectors))
        got = json.loads(redfab("simulate", str(DESIGNS / name / "design.json"), str(vectors_file)).stdout)
        wrong = [(v, e, g) for v, e, g in zip(vectors, expected, got) if e != g]
        for vector, want, have in wrong:
            failures.append(f"{name}: inputs {vector}: Verilator says {want}, the block simulator says {have}")
        print(f"{name}: {len(vectors) - len(wrong)} of {len(vectors)} input combinations agree with Verilator")
    for failure in failures:
        print("FAIL: " + failure)
    return 1 if failures else 0


# ---------------------------------------------------------------- broken copies

def find_block(design: dict, prefix: str) -> str:
    """Position of the first block whose state starts with `prefix`."""
    for pos, state in sorted(design["blocks"].items(), key=lambda kv: parse_pos(kv[0])):
        if state.startswith(prefix):
            return pos
    raise SystemExit(f"no {prefix} in the design")


def broken_copies(design: dict) -> dict[str, dict]:
    """Deliberately wrong versions of a two-input layout, by what is wrong with them."""
    def copy() -> dict:
        return json.loads(json.dumps(design))

    probes = [parse_pos(i["probe"]) for i in design["inputs"]]

    cut = copy()  # a gap in the first input's wire
    x, y, z = probes[0]
    del cut["blocks"][format_pos((x + 3, y, z))]

    no_torch = copy()  # the gate's torch is missing
    del no_torch["blocks"][find_block(design, "minecraft:redstone_wall_torch")]

    stuck = copy()  # a redstone block where the torch should be: output always on
    stuck["blocks"][find_block(design, "minecraft:redstone_wall_torch")] = "minecraft:redstone_block"

    short = copy()  # dust joining the two input wires
    (x0, y0, z0), (_, _, z1) = probes[0], probes[1]
    for z in range(min(z0, z1) + 1, max(z0, z1)):
        short["blocks"][format_pos((x0 + 3, y0, z))] = DUST_NS
        short["blocks"][format_pos((x0 + 3, y0 - 1, z))] = "minecraft:glass"

    return {"cut wire": cut, "missing torch": no_torch, "stuck output": stuck, "shorted inputs": short}


def cmd_lvs_negative(name: str) -> int:
    design, _ = load(name)
    directory = DESIGNS / name / "broken"
    shutil.rmtree(directory, ignore_errors=True)
    directory.mkdir(parents=True)
    mapped = str(DESIGNS / name / "mapped.json")
    ok = True
    good = redfab("lvs", str(DESIGNS / name / "design.json"), mapped, check=False)
    print(f"unbroken layout: {'accepted' if good.returncode == 0 else 'REJECTED: ' + good.stdout.strip()}")
    ok &= good.returncode == 0
    for kind, broken in broken_copies(design).items():
        path = directory / f"{kind.replace(' ', '_')}.json"
        path.write_text(json.dumps(broken, indent=1))
        result = redfab("lvs", str(path), mapped, check=False)
        rejected = result.returncode == 1
        first = result.stdout.strip().splitlines()[0] if result.stdout.strip() else result.stderr.strip()
        print(f"{kind}: {'rejected' if rejected else 'NOT REJECTED'} ({first})")
        ok &= rejected
    return 0 if ok else 1


# ---------------------------------------------------------------- the game

def function_fixture(name: str, design: dict, vectors: list[dict[str, int]], expected: list[dict[str, int]]) -> Fixture:
    """The design placed strictly, with every vector applied in turn and the lamps
    expected to show Verilator's outputs once each has settled."""
    period = design["settle_ticks"] + 2
    blocks = {parse_pos(p): BlockState.parse(s) for p, s in design["blocks"].items()}
    events, expect = [], []
    for k, (vector, outputs) in enumerate(zip(vectors, expected)):
        tick = FIRST_VECTOR_TICK + k * period
        events.append((tick, {
            parse_pos(pin["probe"]): BlockState.parse(pin["on"] if vector[pin["name"]] else pin["off"])
            for pin in design["inputs"]
        }))
        for pin in design["outputs"]:
            lit = "true" if outputs[pin["name"]] else "false"
            expect.append(Expectation(tick + design["settle_ticks"], parse_pos(pin["lamp"]),
                                      BlockState.parse(f"minecraft:redstone_lamp[lit={lit}]")))
    return Fixture(name=name, doc=f"{name}: every input combination", size=tuple(design["size"]), placement="strict",
                   blocks=blocks, events=events, ticks=FIRST_VECTOR_TICK + len(vectors) * period, expect=expect)


def run_function(fixtures: list[Fixture], mode: str, out: Path | None) -> tuple[dict[str, list[str]], VanillaServer]:
    """Runs fixtures at three placements. Returns problems per fixture."""
    problems: dict[str, list[str]] = {fx.name: [] for fx in fixtures}
    server = VanillaServer(f"design-{mode}", redstone_experiments=MODES[mode])
    server.start()
    session = Session(server)
    session.prepare()
    traces = Runner(session, source_for(server)).run(layout(fixtures), log=lambda m: print(f"[{mode}] {m}"))
    for fx in fixtures:
        base = traces[(fx.name, "aligned")]
        for variant in ("straddle", "rotated"):
            if (diff := first_trace_difference(base, traces[(fx.name, variant)])) is not None:
                problems[fx.name].append(f"[{mode}] '{variant}' placement differs: {diff}")
        problems[fx.name] += [f"[{mode}] {failure}" for e in fx.expect if (failure := e.check(base)) is not None]
        if out is not None:
            out.mkdir(parents=True, exist_ok=True)
            (out / f"{fx.name}.fixture.json").write_text(json.dumps(fx.to_json()) + "\n")
            (out / f"{fx.name}.{mode}.trace.json").write_text(json.dumps(base.to_json()) + "\n")
    return problems, server


def cmd_vanilla(names: list[str]) -> int:
    fixtures, designs = [], {}
    for name in names:
        design, module = load(name)
        limit = max(2, min(MAX_TRACED_VECTORS, MAX_TRACED_TICKS // (design["settle_ticks"] + 2)))
        vectors = sampled_vectors(module, limit)
        designs[name] = design
        fixtures.append(function_fixture(name, design, vectors, verilator_outputs(name, module, vectors)))
    game_dir = DESIGNS / "_game"
    shutil.rmtree(game_dir, ignore_errors=True)

    failures: list[str] = []
    for mode in MODES:
        problems, server = run_function(fixtures, mode, game_dir)
        try:
            for name, items in problems.items():
                failures += [f"{name}: {item}" for item in items]
            if mode == "vanilla":
                failures += template_checks(server, designs)
        finally:
            server.stop()

    # The simulator must reproduce each recorded trace.
    replay = redfab("check-traces", str(game_dir), check=False)
    print(replay.stdout.strip())
    if replay.returncode != 0 or " 0 skipped" not in replay.stdout:
        failures.append("the simulator does not reproduce the game's traces of the designs")

    for failure in failures:
        print("FAIL: " + failure)
    if not failures:
        print(f"{', '.join(names)}: correct in the game at three placements in both modes, and traces match the simulator")
    return 1 if failures else 0


def template_checks(server: VanillaServer, designs: dict[str, dict]) -> list[str]:
    """`world.nbt` placed strictly must not move; placed with updates it must settle to the same state."""
    failures = []
    session = Session(server)
    structures = server.run_dir / "world" / "generated" / "redfab" / "structure"
    structures.mkdir(parents=True, exist_ok=True)
    # Well south of the plots the traced run left in this world.
    x, z = 0, TEMPLATE_Z
    for name, design in designs.items():
        shutil.copy(DESIGNS / name / "world.nbt", structures / f"{name}.nbt")
        size = tuple(design["size"])
        strict_origin, normal_origin = (x, 64, z), (x, 64, z + size[2] + 16)
        x += size[0] + 16
        for origin in (strict_origin, normal_origin):
            session.forceload(((origin[0] - 16, 0, origin[2] - 16), (origin[0] + size[0] + 16, 0, origin[2] + size[2] + 16)))
        place(session, name, strict_origin, strict=True)
        if read_box(session, strict_origin, size) != design["blocks"]:
            failures.append(f"{name}: world.nbt placed strictly differs from the design")
        session.step(20)
        if read_box(session, strict_origin, size) != design["blocks"]:
            failures.append(f"{name}: the strictly placed build changed within 20 ticks")
        place(session, name, normal_origin, strict=False)
        session.step(20)
        if read_box(session, normal_origin, size) != design["blocks"]:
            failures.append(f"{name}: placed with updates, the build settles to a different state")
    if not failures:
        print(f"world.nbt: {len(designs)} builds stay put when placed strictly and settle the same when placed with updates")
    return failures


def cmd_vanilla_negative(name: str) -> int:
    design, module = load(name)
    vectors = all_vectors(module)
    expected = verilator_outputs(name, module, vectors)
    copies = {"unbroken": design, **{k: v for k, v in broken_copies(design).items() if k in ("cut wire", "stuck output")}}
    fixtures = [function_fixture(kind.replace(" ", "_"), d, vectors, expected) for kind, d in copies.items()]
    problems, server = run_function(fixtures, "vanilla", None)
    server.stop()
    ok = True
    for kind in copies:
        wrong = [p for p in problems[kind.replace(" ", "_")] if "expected" in p]
        if kind == "unbroken":
            print(f"unbroken: {'passes' if not wrong else 'FAILS: ' + wrong[0]}")
            ok &= not wrong
        else:
            print(f"{kind}: {'fails as it should (' + wrong[0] + ')' if wrong else 'PASSES, which is wrong'}")
            ok &= bool(wrong)
    return 0 if ok else 1


def cmd_exhaustive(name: str) -> int:
    """Every input combination in the game, in both modes, reading only the lamps:
    the inputs are set, the game steps `settle_ticks`, and each lamp is tested."""
    design, module = load(name)
    vectors = all_vectors(module)
    expected = verilator_outputs(name, module, vectors)
    size = tuple(design["size"])
    origin = (0, 64, 600)
    failures: list[str] = []
    for mode in MODES:
        server = VanillaServer(f"exhaustive-{mode}", redstone_experiments=MODES[mode])
        server.start()
        try:
            session = Session(server)
            session.prepare()
            structures = server.run_dir / "world" / "generated" / "redfab" / "structure"
            structures.mkdir(parents=True, exist_ok=True)
            shutil.copy(DESIGNS / name / "world.nbt", structures / f"{name}.nbt")
            session.forceload((origin, (origin[0] + size[0], 0, origin[2] + size[2])))
            place(session, name, origin, strict=True)
            if read_box(session, origin, size) != design["blocks"]:
                failures.append(f"[{mode}] world.nbt placed strictly differs from the design")
                continue
            at = lambda p: tuple(a + b for a, b in zip(origin, parse_pos(p)))
            wrong = 0
            for k, (vector, outputs) in enumerate(zip(vectors, expected)):
                for pin in design["inputs"]:
                    session.setblock(at(pin["probe"]), BlockState.parse(pin["on"] if vector[pin["name"]] else pin["off"]), False)
                session.step(design["settle_ticks"])
                got = {pin["name"]: int(session.probe(at(pin["lamp"]), BlockState.parse("minecraft:redstone_lamp[lit=true]")))
                       for pin in design["outputs"]}
                if got != outputs:
                    wrong += 1
                    if wrong <= 5:
                        failures.append(f"[{mode}] inputs {vector}: Verilator says {outputs}, the game shows {got}")
                if (k + 1) % 64 == 0:
                    print(f"[{mode}] {k + 1} of {len(vectors)} combinations, {wrong} wrong")
            print(f"[{mode}] {name}: {len(vectors) - wrong} of {len(vectors)} input combinations correct in the game")
            if wrong:
                failures.append(f"[{mode}] {wrong} of {len(vectors)} input combinations are wrong")
        finally:
            server.stop()
    for failure in failures:
        print("FAIL: " + failure)
    return 1 if failures else 0


def lamp_changes(fixture: Fixture, trace: Trace, design: dict) -> dict[str, list[int]]:
    """For each output, how many ticks after the preceding input change every change
    of its lamp happened in the recorded trace."""
    lamps = {parse_pos(pin["lamp"]): pin["name"] for pin in design["outputs"]}
    stimuli = sorted(tick for tick, _ in fixture.events)
    changes: dict[str, list[int]] = {name: [] for name in lamps.values()}
    for tick in sorted(trace.entries):
        before = [t for t in stimuli if t <= tick]
        if not before:
            continue
        for pos in trace.entries[tick].changes:
            if pos in lamps:
                changes[lamps[pos]].append(tick - before[-1])
    return changes


def cmd_timing_traces(names: list[str]) -> int:
    """The traces design-vanilla recorded, against each design's timing report: no lamp
    changes later than its worst case or earlier than its best case, and the full
    adder reaches its worst case in the game."""
    game_dir = DESIGNS / "_game"
    failures: list[str] = []
    reached = False
    for name in names:
        design, _ = load(name)
        timing = design.get("timing")
        if timing is None:
            failures.append(f"{name}: design.json has no timing report")
            continue
        fixture = Fixture.from_json(json.loads((game_dir / f"{name}.fixture.json").read_text()))
        for mode in MODES:
            path = game_dir / f"{name}.{mode}.trace.json"
            if not path.exists():
                failures.append(f"{name}: no recorded trace {path.name}; run `design vanilla` first")
                continue
            trace = Trace.from_json(json.loads(path.read_text()), fixture.ticks)
            for output, delays in lamp_changes(fixture, trace, design).items():
                worst, best = timing["outputs"][output]["max"], timing["outputs"][output]["min"]
                late = [d for d in delays if d > worst]
                early = [d for d in delays if d < best]
                if late:
                    failures.append(f"{name} [{mode}]: {output} changed {max(late)} ticks after an input change, "
                                    f"later than its reported worst case of {worst}")
                if early:
                    failures.append(f"{name} [{mode}]: {output} changed {min(early)} ticks after an input change, "
                                    f"earlier than its reported best case of {best}")
                if name == "fa1" and worst in delays:
                    reached = True
            print(f"{name} [{mode}]: lamp changes stay within the reported timing")
    if "fa1" in names and not reached:
        failures.append("fa1: no recorded lamp change reaches the reported worst case")
    for failure in failures:
        print("FAIL: " + failure)
    return 1 if failures else 0


# ---------------------------------------------------------------- clocked designs

CYCLES = 64
BURST_TOGGLES = 16


def clocked_vectors(design: dict, cycles: int, seed: int) -> list[dict[str, int]]:
    """One vector of the data inputs per cycle: random, after a cycle with every
    input on (which resets any design with a synchronous reset) and one with all off."""
    rng = random.Random(seed)
    names = [p["name"] for p in design["inputs"] if p["name"] != design["timing"]["sequential"]["clock"]]
    vectors = [{n: 1 for n in names}, {n: 0 for n in names}]
    vectors += [{n: rng.randint(0, 1) for n in names} for _ in range(cycles - 2)]
    return vectors


def verilator_cycles(name: str, module: dict, clock: str, vectors: list[dict[str, int]]) -> list[dict[str, int]]:
    """What the source design's outputs are at the end of each clock cycle according
    to Verilator: the clock rises, the inputs change, and the outputs are read before
    the next rise."""
    ins, outs = ports(module)
    data = [(port, width) for port, width in ins if port != clock]
    directory = DESIGNS / name / "verilator"
    shutil.rmtree(directory, ignore_errors=True)
    directory.mkdir(parents=True)
    lines = ["module tb;", f"  logic {clock} = 0;"]
    lines += [f"  logic [{width - 1}:0] {port};" for port, width in data + outs]
    lines.append("  top dut (.*);")
    lines.append("  initial begin")
    for vector in vectors:
        lines.append(f"    {clock} = 1; #1;")
        for port, width in data:
            value = sum(vector[bit] << i for i, bit in enumerate(bit_names(port, width)))
            lines.append(f"    {port} = {width}'d{value};")
        shown = " ".join(f"{port}=%b" for port, _ in outs)
        lines.append(f'    #4; $display("OUT {shown}", {", ".join(port for port, _ in outs)});')
        lines.append(f"    {clock} = 0; #5;")
    lines += ["    $finish;", "  end", "endmodule"]
    (directory / "tb.sv").write_text("\n".join(lines) + "\n")
    subprocess.run(
        ["verilator", "--binary", "--timing", "-Wno-fatal", "--Mdir", str(directory / "obj"), "--top-module", "tb",
         str(DESIGNS / name / "top.sv"), str(directory / "tb.sv")],
        check=True, capture_output=True, text=True,
    )
    output = subprocess.run([str(directory / "obj" / "Vtb")], check=True, capture_output=True, text=True).stdout
    results = []
    for line in output.splitlines():
        if not line.startswith("OUT "):
            continue
        row = {}
        for port, bits in re.findall(r"(\w+)=([01xz]+)", line):
            width = dict(outs)[port]
            for i, bit in enumerate(bit_names(port, width)):
                row[bit] = int(bits[len(bits) - 1 - i]) if bits[len(bits) - 1 - i] in "01" else 0
        results.append(row)
    if len(results) != len(vectors):
        raise SystemExit(f"{name}: Verilator printed {len(results)} results for {len(vectors)} cycles")
    return results


def simulate_cycles(name: str, vectors: list[dict[str, int]], shuffle: int | None = None) -> list[dict[str, int]]:
    vectors_file = DESIGNS / name / "cycles.json"
    vectors_file.write_text(json.dumps(vectors))
    args = ["simulate", str(DESIGNS / name / "design.json"), str(vectors_file)]
    if shuffle is not None:
        args += ["--shuffle", str(shuffle)]
    return json.loads(redfab(*args).stdout)


def cmd_cycles(names: list[str]) -> int:
    failures = []
    for name in names:
        design, module = load(name)
        seq = design["timing"].get("sequential")
        if seq is None:
            failures.append(f"{name}: not a clocked design")
            continue
        vectors = clocked_vectors(design, CYCLES, 1)
        expected = verilator_cycles(name, module, seq["clock"], vectors)
        got = simulate_cycles(name, vectors)
        wrong = [k for k, (e, g) in enumerate(zip(expected, got)) if e != g]
        for k in wrong[:3]:
            failures.append(f"{name}: cycle {k}: Verilator says {expected[k]}, the block simulator says {got[k]}")
        for seed in (1, 2, 3):
            shuffled = simulate_cycles(name, vectors, shuffle=seed)
            if shuffled != got:
                k = next(i for i, (a, b) in enumerate(zip(got, shuffled)) if a != b)
                failures.append(f"{name}: with same-tick order shuffled (seed {seed}), cycle {k} differs: {got[k]} vs {shuffled[k]}")
        print(f"{name}: {CYCLES - len(wrong)} of {CYCLES} cycles agree with Verilator at a period of {seq['period']} ticks; "
              f"shuffled same-tick orders agree")
    for failure in failures:
        print("FAIL: " + failure)
    return 1 if failures else 0


def drive_cycles_in_game(session: Session, placement: "Placement", design: dict, vectors: list[dict[str, int]]) -> list[dict[str, int]]:
    """Drives a placed clocked design through the vectors as the timing report says
    and reads its lamps at the end of each cycle."""
    seq = design["timing"]["sequential"]
    pins = {p["name"]: p for p in design["inputs"]}
    clock = pins[seq["clock"]]
    on_level = BlockState.parse(clock["on"]) if seq["active"] == "rising" else BlockState.parse(clock["off"])
    off_level = BlockState.parse(clock["off"]) if seq["active"] == "rising" else BlockState.parse(clock["on"])
    at = lambda p: placement.to_world(parse_pos(p))
    session.setblock(at(clock["probe"]), off_level, False)
    session.step(10)
    period, high, change_at, sample_at = seq["period"], seq["high"], seq["input_change_at"], seq["sample_at"]
    results = []
    for vector in vectors:
        tick = 0
        events = sorted({0, high, change_at, sample_at, period})
        for t in events:
            session.step(t - tick)
            tick = t
            if t == period:
                break
            if t == 0:
                session.setblock(at(clock["probe"]), on_level, False)
            if t == high:
                session.setblock(at(clock["probe"]), off_level, False)
            if t == change_at:
                for name, value in vector.items():
                    session.setblock(at(pins[name]["probe"]), BlockState.parse(pins[name]["on"] if value else pins[name]["off"]), False)
            if t == sample_at:
                results.append({p["name"]: int(session.probe(at(p["lamp"]), BlockState.parse("minecraft:redstone_lamp[lit=true]")))
                                for p in design["outputs"]})
    return results


def place_by_blocks(session: Session, placement: "Placement", design: dict) -> None:
    """Places a design block by block, as the trace runner does, strictly."""
    blocks = {parse_pos(p): BlockState.parse(s) for p, s in design["blocks"].items()}
    for pos, state in sorted(blocks.items(), key=lambda kv: (kv[0][1], kv[0])):
        session.setblock(placement.to_world(pos), placement.state_to_world(state), True)


def placements_for(design: dict, origin_z: int) -> list[tuple[str, "Placement"]]:
    from .runner import Placement, Rotation, cell_size
    size = tuple(design["size"])
    fx = Fixture(name="d", doc="", size=size, placement="strict", blocks={}, events=[], ticks=1, expect=[])
    cell = cell_size(fx)
    sx, _, sz = size
    shift = lambda s: (16 - max(1, s // 2)) % 16
    return [
        ("aligned", Placement(Rotation(0), (0, 64, origin_z), size)),
        ("straddle", Placement(Rotation(0), (cell + shift(sx), 64, origin_z + shift(sz)), size)),
        ("rotated", Placement(Rotation(1), (2 * cell, 64, origin_z), size)),
    ]


def run_cycles_in_game(names: list[str], vectors_of, expected_of, expect_match: bool) -> list[str]:
    """Runs each design at three placements in both modes. Returns failures: a cycle
    differing from Verilator when `expect_match`, or none differing when not."""
    failures = []
    designs = {name: load(name)[0] for name in names}
    for mode in MODES:
        server = VanillaServer(f"cycles-{mode}", redstone_experiments=MODES[mode])
        server.start()
        try:
            session = Session(server)
            session.prepare()
            z = 0
            for name, design in designs.items():
                vectors = vectors_of(name, design)
                expected = expected_of(name, design, vectors)
                for variant, placement in placements_for(design, z):
                    (x0, _, z0), (x1, _, z1) = placement.world_box()
                    session.forceload(((x0 - 16, 0, z0 - 16), (x1 + 16, 0, z1 + 16)))
                    place_by_blocks(session, placement, design)
                    got = drive_cycles_in_game(session, placement, design, vectors)
                    wrong = [k for k, (e, g) in enumerate(zip(expected, got)) if e != g]
                    tag = f"{name} [{mode}] {variant}"
                    if expect_match and wrong:
                        k = wrong[0]
                        failures.append(f"{tag}: cycle {k}: Verilator says {expected[k]}, the game shows {got[k]} ({len(wrong)} cycles differ)")
                    elif not expect_match and not wrong:
                        failures.append(f"{tag}: every cycle matches Verilator, but this copy should fail")
                    print(f"{tag}: {len(vectors) - len(wrong)} of {len(vectors)} cycles match Verilator")
                z += max(placement.world_box()[1][2] for _, placement in placements_for(design, z)) - z + 32
        finally:
            server.stop()
    return failures


def cmd_cycles_vanilla(names: list[str]) -> int:
    failures = run_cycles_in_game(
        names,
        lambda name, design: clocked_vectors(design, CYCLES, 1),
        lambda name, design, vectors: verilator_cycles(name, load(name)[1], design["timing"]["sequential"]["clock"], vectors),
        expect_match=True,
    )
    for failure in failures:
        print("FAIL: " + failure)
    if not failures:
        print(f"{', '.join(names)}: {CYCLES} cycles correct in the game at three placements in both modes")
    return 1 if failures else 0


def build_broken(name: str, fault: str) -> str:
    """Builds a copy of a design with a fault built in; the build must fail and say so."""
    broken = f"_{name}_{fault}"
    result = redfab("build", str(ROOT / "designs" / name / "top.sv"), "--top", "top", "--break", fault,
                    "-o", str(DESIGNS / broken), check=False)
    if result.returncode == 0:
        raise SystemExit(f"{broken}: the build with --break {fault} did not fail")
    if "hold violated" not in result.stdout and "hold violated" not in result.stderr:
        raise SystemExit(f"{broken}: the build did not report a hold violation:\n{result.stdout}{result.stderr}")
    shutil.copy(DESIGNS / name / "top.sv", DESIGNS / broken / "top.sv")
    print(f"{broken}: the build is flagged for a hold violation, as it should be")
    return broken


def cmd_hold_negative(name: str) -> int:
    broken = build_broken(name, "hold")
    design, module = load(broken)
    seq = design["timing"]["sequential"]
    vectors = clocked_vectors(design, CYCLES, 1)
    expected = verilator_cycles(broken, module, seq["clock"], vectors)
    got = simulate_cycles(broken, vectors)
    wrong = [k for k, (e, g) in enumerate(zip(expected, got)) if e != g]
    if not wrong:
        print(f"FAIL: {broken}: the simulator agrees with Verilator on every cycle, but the hold violation should show")
        return 1
    print(f"{broken}: in the simulator {len(wrong)} of {CYCLES} cycles differ from Verilator, as a hold violation should")
    return 0


def cmd_hold_negative_vanilla(name: str) -> int:
    broken = f"_{name}_hold"
    if not (DESIGNS / broken / "design.json").exists():
        build_broken(name, "hold")
    failures = run_cycles_in_game(
        [broken],
        lambda n, design: clocked_vectors(design, CYCLES, 1),
        lambda n, design, vectors: verilator_cycles(n, load(n)[1], design["timing"]["sequential"]["clock"], vectors),
        expect_match=False,
    )
    for failure in failures:
        print("FAIL: " + failure)
    return 1 if failures else 0


def burst_fixture(name: str, design: dict, spacing: int, expected_lit: dict[str, bool]) -> Fixture:
    """The design with its first input toggled BURST_TOGGLES times, `spacing` ticks
    apart, then left off; the lamps must show `expected_lit` once it has settled."""
    blocks = {parse_pos(p): BlockState.parse(s) for p, s in design["blocks"].items()}
    pin = design["inputs"][0]
    events = []
    for k in range(BURST_TOGGLES):
        tick = FIRST_VECTOR_TICK + k * spacing
        events.append((tick, {parse_pos(pin["probe"]): BlockState.parse(pin["on"] if k % 2 == 0 else pin["off"])}))
    end = FIRST_VECTOR_TICK + BURST_TOGGLES * spacing + design["settle_ticks"] + 4
    expect = [Expectation(end, parse_pos(p["lamp"]), BlockState.parse(f"minecraft:redstone_lamp[lit={'true' if expected_lit[p['name']] else 'false'}]"))
              for p in design["outputs"]]
    return Fixture(name=name, doc=f"{design['name']}: {pin['name']} toggled every {spacing} ticks", size=tuple(design["size"]),
                   placement="strict", blocks=blocks, events=events, ticks=end + 2, expect=expect)


def burnout_fixtures(name: str) -> tuple[list[Fixture], int]:
    """Two burst fixtures of the design: at its reported input spacing, and at half
    of it. Returns them and the spacing."""
    design, module = load(name)
    spacing = design["timing"]["input_spacing"]
    pin = design["inputs"][0]["name"]
    rest = {p["name"]: 0 for p in design["inputs"]}
    expected = verilator_outputs(name, module, [rest])[0]
    lit = {k: bool(v) for k, v in expected.items()}
    return [burst_fixture("within_budget", design, spacing, lit), burst_fixture("too_fast", design, spacing // 2, lit)], spacing


def cmd_burnout_negative(name: str) -> int:
    fixtures, spacing = burnout_fixtures(name)
    ok = True
    for fx, flagged in zip(fixtures, (False, True)):
        rate = spacing if fx.name == "within_budget" else spacing // 2
        stress = redfab("stress", str(DESIGNS / name / "design.json"), "--spacing", str(rate), check=False)
        print(stress.stdout.strip())
        if (stress.returncode != 0) != flagged:
            print(f"FAIL: {name}: changes every {rate} ticks should {'' if flagged else 'not '}be flagged")
            ok = False
        directory = DESIGNS / name / "burst"
        directory.mkdir(exist_ok=True)
        (directory / f"{fx.name}.fixture.json").write_text(json.dumps(fx.to_json()))
        run = redfab("check-fixtures", str(directory), check=False)
        failed = f"FAIL  {fx.name}" in run.stdout
        if failed != flagged:
            print(f"FAIL: {name}: in the simulator the burst at every {rate} ticks {'passes' if not failed else 'fails'}, but should {'fail' if flagged else 'pass'}")
            ok = False
        else:
            print(f"{name}: in the simulator, toggling every {rate} ticks {'burns a torch out, as it should' if failed else 'is fine'}")
        for stale in directory.glob("*.fixture.json"):
            stale.unlink()
    return 0 if ok else 1


def cmd_burnout_negative_vanilla(name: str) -> int:
    fixtures, spacing = burnout_fixtures(name)
    problems, server = run_function(fixtures, "vanilla", None)
    server.stop()
    ok = True
    for fx, flagged in zip(fixtures, (False, True)):
        wrong = [p for p in problems[fx.name] if "expected" in p]
        rate = spacing if fx.name == "within_budget" else spacing // 2
        if bool(wrong) != flagged:
            print(f"FAIL: {name}: in the game, toggling every {rate} ticks {'fails' if wrong else 'passes'}, but should {'fail' if flagged else 'pass'}")
            ok = False
        else:
            print(f"{name}: in the game, toggling every {rate} ticks {'burns a torch out, as it should (' + wrong[0] + ')' if wrong else 'is fine'}")
    return 0 if ok else 1


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("action", choices=["sim", "lvs-negative", "vanilla", "vanilla-negative", "exhaustive", "timing-traces",
                                       "cycles", "cycles-vanilla", "hold-negative", "hold-negative-vanilla",
                                       "burnout-negative", "burnout-negative-vanilla"])
    ap.add_argument("names", nargs="+")
    args = ap.parse_args(argv)
    if args.action == "sim":
        return cmd_sim(args.names)
    if args.action == "lvs-negative":
        return cmd_lvs_negative(args.names[0])
    if args.action == "vanilla":
        return cmd_vanilla(args.names)
    if args.action == "exhaustive":
        return cmd_exhaustive(args.names[0])
    if args.action == "timing-traces":
        return cmd_timing_traces(args.names)
    if args.action == "cycles":
        return cmd_cycles(args.names)
    if args.action == "cycles-vanilla":
        return cmd_cycles_vanilla(args.names)
    if args.action == "hold-negative":
        return cmd_hold_negative(args.names[0])
    if args.action == "hold-negative-vanilla":
        return cmd_hold_negative_vanilla(args.names[0])
    if args.action == "burnout-negative":
        return cmd_burnout_negative(args.names[0])
    if args.action == "burnout-negative-vanilla":
        return cmd_burnout_negative_vanilla(args.names[0])
    return cmd_vanilla_negative(args.names[0])


if __name__ == "__main__":
    sys.exit(main())
