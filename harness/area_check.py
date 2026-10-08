"""Compaction check: no shipped design gets bigger or slower than its baseline row.

    uv run python -m harness.area_check [NAME...]           compare build/designs/NAME (every row by default)
    uv run python -m harness.area_check --update [NAME...]  re-record the named rows (byte-identical when nothing moved)
    uv run python -m harness.area_check --control NAME OUT  a copy of the baseline claiming one repeater fewer for NAME
    uv run python -m harness.area_check --baseline FILE ... another baseline file (the negative control reads the copy)

The baseline, tests/baselines/area.json, holds one row per design under designs/, read
from the counts `redfab build` writes into design.json (tiles, grid, repeaters, hold fix
rounds, placement) and its timing report. Compared, per design: tiles, footprint x by z,
chunks, routing columns, repeaters, settle ticks, the hold fix rounds, tiles per cell and
repeaters per torch on the worst path; combinational designs also their input spacing,
clocked ones the period, the clock skew (latest minus earliest arrival), the input window
(wider is better) and the least hold slack (may fall, must stay at or above zero). The
cell count and the placement style must be the same, or the row describes another
layout. Every number may only stay or improve; a deliberate regression is an edit of the
baseline in the same commit, named in its message. The sequential analysis reports no
path, so clocked rows have no repeaters-per-torch figure.

`measure` turns a design file and its mapped netlist into a row, `compare` a row and its
baseline into the problems, `format_baseline` writes the file in its one canonical form;
`control` makes the copy the gate uses to prove the check can fail.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

from .server import ROOT

BASELINE = ROOT / "tests" / "baselines" / "area.json"
DESIGNS = ROOT / "build" / "designs"
FORMAT = "redfab-area-baseline/1"
NOTE = ("one row per design under designs/, recorded by `harness.area_check --update`; "
        "every number may only stay or improve, and a deliberate regression is an edit here named in the commit message")

# What may only fall, what may only rise, and what must stay the same.
LOWER_IS_BETTER = ("tiles", "area", "chunks", "routing_cols", "repeaters", "settle", "input_spacing", "period", "skew",
                   "hold_fix_rounds", "tiles_per_cell", "repeaters_per_torch")
HIGHER_IS_BETTER = ("input_window",)
MUST_MATCH = ("cells", "placement")


class Stale(Exception):
    """The design file predates the layout counts."""


def chunks(design: dict) -> int:
    """How many chunks the design covers when placed at the origin (as report.txt says)."""
    xs = [int(pos.split(",")[0]) for pos in design["blocks"]]
    zs = [int(pos.split(",")[2]) for pos in design["blocks"]]
    return (max(xs, default=0) // 16 + 1) * (max(zs, default=0) // 16 + 1)


def repeaters_per_torch(timing: dict) -> float | None:
    """Repeaters per torch along the path of the output that settles last; none when no
    output reports a path (a clocked design, or one whose outputs never change)."""
    paths = [o["path"] for o in timing["outputs"].values() if o.get("path")]
    if not paths:
        return None
    worst = max(timing["outputs"].values(), key=lambda o: (o["max"], len(o.get("path", []))))
    repeaters = sum(1 for step in worst["path"] if step.startswith("repeater at"))
    torches = sum(1 for step in worst["path"] if step.startswith("torch at"))
    return round(repeaters / torches, 1) if torches else None


def measure(design: dict, module: dict) -> dict:
    """The baseline row of a built design (its design.json and the top module of its
    mapped netlist). Raises `Stale` for a file written before the counts existed."""
    if "grid" not in design or "tiles" not in design:
        raise Stale("the design file has no layout counts (built before the area baseline); rebuild it")
    cells = len(module["cells"])
    grid = design["grid"]
    size = design["size"]
    timing = design["timing"]
    row = {
        "cells": cells,
        "placement": design.get("placement", ""),
        "tiles": design["tiles"],
        "cols": grid["cols"],
        "rows": grid["rows"],
        "routing": list(grid["routing"]),
        "routing_cols": sum(grid["routing"]),
        "repeaters": design["repeaters"],
        "blocks": len(design["blocks"]),
        "footprint": [size[0], size[2]],
        "area": size[0] * size[2],
        "chunks": chunks(design),
        "settle": design["settle_ticks"],
    }
    seq = timing.get("sequential")
    if seq:
        arrivals = [f["clock_arrival"] for f in seq["flip_flops"]]
        slacks = [f["hold_slack"] for f in seq["flip_flops"] if f.get("hold_slack") is not None]
        row["period"] = seq["period"]
        row["skew"] = max(arrivals, default=0) - min(arrivals, default=0)
        row["input_window"] = seq["input_change_by"] - seq["input_change_from"]
        row["min_hold_slack"] = min(slacks) if slacks else None
    else:
        row["input_spacing"] = timing.get("input_spacing", 0)
    row["hold_fix_rounds"] = design.get("hold_fix_rounds", 0)
    row["tiles_per_cell"] = round(design["tiles"] / cells, 1) if cells else 0.0
    row["repeaters_per_torch"] = repeaters_per_torch(timing)
    return row


def compare(name: str, got: dict, base: dict) -> list[str]:
    """What is worse in `got` than its baseline row, each naming the design."""
    out = []
    for key in MUST_MATCH:
        if got.get(key) != base.get(key):
            out.append(f"{name}: {key} {base.get(key)!r} -> {got.get(key)!r}, another layout; re-record the row if that is meant")
    for key in LOWER_IS_BETTER + HIGHER_IS_BETTER:
        want, have = base.get(key), got.get(key)
        if want is None and have is None:
            continue
        if want is None or have is None:
            out.append(f"{name}: {key} is {want} in the baseline and {have} in the build")
            continue
        worse = have > want if key in LOWER_IS_BETTER else have < want
        if worse:
            out.append(f"{name}: {key} {want} -> {have}, worse than the baseline")
    slack = got.get("min_hold_slack")
    if slack is not None and slack < 0:
        out.append(f"{name}: min_hold_slack {slack} is below zero")
    return out


def summary(name: str, row: dict) -> str:
    """One line of what a row says."""
    text = (f"{name}: {row['tiles']} tiles, {row['cols']} x {row['rows']}, {row['routing_cols']} routing columns, "
            f"{row['repeaters']} repeaters, {row['footprint'][0]} x {row['footprint'][1]} blocks, {row['chunks']} chunks")
    if "period" in row:
        text += f", period {row['period']}, skew {row['skew']}, window {row['input_window']}, hold slack {row['min_hold_slack']}"
    else:
        text += f", settle {row['settle']}, spacing {row['input_spacing']}"
    if row.get("hold_fix_rounds"):
        text += f", {row['hold_fix_rounds']} hold fix rounds"
    return text


def format_baseline(rows: dict[str, dict]) -> str:
    """The baseline file: designs by name, one line per figure, lists on one line."""
    lines = ["{", f'  "format": {json.dumps(FORMAT)},', f'  "note": {json.dumps(NOTE)},', '  "designs": {']
    names = sorted(rows)
    for i, name in enumerate(names):
        lines.append(f"    {json.dumps(name)}: {{")
        items = list(rows[name].items())
        for j, (key, value) in enumerate(items):
            lines.append(f"      {json.dumps(key)}: {json.dumps(value)}{',' if j + 1 < len(items) else ''}")
        lines.append(f"    }}{',' if i + 1 < len(names) else ''}")
    lines += ["  }", "}"]
    return "\n".join(lines) + "\n"


def load_baseline(path: Path) -> dict[str, dict]:
    data = json.loads(path.read_text())
    if data.get("format") != FORMAT:
        raise SystemExit(f"{path}: unexpected format {data.get('format')!r}")
    return data["designs"]


def control(rows: dict[str, dict], name: str) -> dict[str, dict]:
    """A copy of the baseline claiming one repeater fewer for `name`: a build that
    matches the real baseline must fail against it, naming the design."""
    if name not in rows:
        raise SystemExit(f"no baseline row for {name}")
    copy = {n: dict(r) for n, r in rows.items()}
    copy[name]["repeaters"] -= 1
    return copy


def read_row(name: str, designs: Path) -> dict:
    """The row of a built design, or `Stale`/`FileNotFoundError` saying what is missing."""
    directory = designs / name
    if not (directory / "design.json").exists():
        raise FileNotFoundError(f"{directory / 'design.json'} is missing; run `redfab build` first")
    design = json.loads((directory / "design.json").read_text())
    module = json.loads((directory / "mapped.json").read_text())["modules"]["top"]
    return measure(design, module)


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    baseline = BASELINE
    update = False
    control_name = None
    control_out = None
    names: list[str] = []
    while args:
        arg = args.pop(0)
        if arg == "--baseline":
            baseline = Path(args.pop(0))
        elif arg == "--update":
            update = True
        elif arg == "--control":
            control_name, control_out = args.pop(0), Path(args.pop(0))
        elif arg.startswith("-"):
            raise SystemExit(f"unknown option {arg}\n{__doc__}")
        else:
            names.append(arg)
    rows = load_baseline(baseline)
    if control_name is not None:
        assert control_out is not None
        control_out.write_text(format_baseline(control(rows, control_name)))
        print(f"{control_out}: {control_name} claims {rows[control_name]['repeaters'] - 1} repeaters")
        return 0
    names = names or sorted(rows)
    problems = []
    measured = {}
    for name in names:
        try:
            measured[name] = read_row(name, DESIGNS)
        except (Stale, FileNotFoundError) as e:
            problems.append(f"{name}: {e}")
    if update:
        if problems:
            print("\n".join(problems))
            return 1
        rows.update(measured)
        text = format_baseline(rows)
        changed = text != baseline.read_text()
        baseline.write_text(text)
        for name in names:
            print(f"  {summary(name, measured[name])}")
        print(f"{baseline} {'rewritten' if changed else 'unchanged'} with {len(names)} rows re-recorded")
        return 0
    for name in names:
        if name not in measured:
            continue
        if name not in rows:
            problems.append(f"{name}: no baseline row; record one with --update {name}")
            continue
        found = compare(name, measured[name], rows[name])
        if found:
            print(f"  WORSE {summary(name, measured[name])}")
            problems.extend(found)
        else:
            print(f"  ok    {summary(name, measured[name])}")
    for problem in problems:
        print(problem)
    if problems:
        designs = sorted({p.split(":")[0] for p in problems})
        print(f"{len(problems)} problems in {len(designs)} designs ({', '.join(designs)}); the baseline is {baseline}")
        return 1
    print(f"{len(names)} designs are within their baseline rows ({baseline})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
