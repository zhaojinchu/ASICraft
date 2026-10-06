"""Checks built designs (build/designs/<name>/) against Verilator and against the game.

    python -m harness.design sim NAME...              simulator vs Verilator, every input combination
    python -m harness.design lvs-negative NAME        broken layouts must fail the netlist check
    python -m harness.design vanilla NAME...          the game: function, traces vs simulator, placement
    python -m harness.design vanilla-negative NAME    broken layouts must fail in the game
    python -m harness.design exhaustive NAME          the game: every input combination, outputs only

Build the designs first with `redfab build designs/<name>/top.sv -o build/designs/<name>`.
"""

from __future__ import annotations

import argparse
import itertools
import json
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
from .trace import Expectation, Fixture, first_trace_difference

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


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("action", choices=["sim", "lvs-negative", "vanilla", "vanilla-negative", "exhaustive"])
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
    return cmd_vanilla_negative(args.names[0])


if __name__ == "__main__":
    sys.exit(main())
