"""The area-period baseline: a row is read from design.json, a worse number names the
design, a better one passes, the file has one canonical form, and the committed baseline
lists exactly the shipped designs."""

import json
from pathlib import Path

import pytest

from harness.area_check import (BASELINE, FORMAT, STYLES, Stale, compare, control, format_baseline, load_baseline,
                                measure, read_row)
from harness.server import ROOT

MODULE = {"cells": {"g1": {}, "g2": {}, "g3": {}, "g4": {}}}


def combinational() -> dict:
    blocks = {f"{x},1,{z}": "minecraft:stone" for x in (0, 33) for z in (0, 17)}
    return {
        "format": "redfab-design/1", "name": "x", "size": [35, 4, 20], "inputs": [], "outputs": [], "constants": [],
        "settle_ticks": 30, "placement": "pitch4", "tiles": 101, "grid": {"cols": 16, "rows": 14, "routing": [1, 1, 2, 2, 2, 1]},
        "repeaters": 28, "hold_fix_rounds": 0,
        "timing": {"outputs": {
            "y": {"min": 14, "max": 30, "path": ["input at 1,1,7", "repeater at 2,1,7", "torch at 3,1,7", "repeater at 4,1,7",
                                                  "repeater at 5,1,7", "torch at 6,1,7", "lamp at 7,1,7 lighting"]},
            "c": {"min": 14, "max": 20, "path": ["input at 1,1,2", "torch at 3,1,2", "lamp at 7,1,2 lighting"]}},
            "exhaustive": True, "sequential": None, "input_spacing": 8},
        "blocks": blocks,
    }


def clocked() -> dict:
    design = combinational()
    design["timing"] = {
        "outputs": {"q": {"min": 0, "max": 82, "path": []}}, "exhaustive": False, "input_spacing": 0,
        "sequential": {"clock": "clk", "active": "rising", "period": 92, "high": 46, "input_change_from": 0, "input_change_by": 17,
                       "input_change_at": 0, "sample_at": 91, "hold_violations": [],
                       "flip_flops": [{"cell": "q0", "clock_arrival": 24, "hold_slack": 51},
                                      {"cell": "q1", "clock_arrival": 40, "hold_slack": 63},
                                      {"cell": "q2", "clock_arrival": 30, "hold_slack": None}]},
    }
    design["hold_fix_rounds"] = 2
    return design


def test_a_combinational_row_is_read_from_the_design_file():
    row = measure(combinational(), MODULE)
    assert row == {
        "cells": 4, "placement": "pitch4", "tiles": 101, "cols": 16, "rows": 14, "routing": [1, 1, 2, 2, 2, 1], "routing_cols": 9,
        "repeaters": 28, "blocks": 4, "footprint": [35, 20], "area": 700, "chunks": 3 * 2, "settle": 30, "input_spacing": 8,
        "hold_fix_rounds": 0, "tiles_per_cell": 25.2, "repeaters_per_torch": 1.5,
    }


def test_a_clocked_row_has_the_period_skew_window_and_least_hold_slack_and_no_path_ratio():
    row = measure(clocked(), MODULE)
    assert "input_spacing" not in row
    assert (row["period"], row["skew"], row["input_window"], row["min_hold_slack"]) == (92, 16, 17, 51)
    assert row["hold_fix_rounds"] == 2
    assert row["repeaters_per_torch"] is None


def test_a_design_file_without_the_counts_is_stale():
    design = combinational()
    del design["grid"]
    del design["tiles"]
    with pytest.raises(Stale, match="rebuild it"):
        measure(design, MODULE)


def test_a_worse_number_names_the_design_and_a_better_one_passes():
    base = measure(combinational(), MODULE)
    assert compare("x", dict(base), base) == []
    for key in ("tiles", "area", "chunks", "routing_cols", "repeaters", "settle", "input_spacing", "hold_fix_rounds",
                "tiles_per_cell", "repeaters_per_torch"):
        worse = dict(base, **{key: base[key] + 1})
        assert compare("x", worse, base) == [f"x: {key} {base[key]} -> {base[key] + 1}, worse than the baseline"], key
        better = dict(base, **{key: base[key] - 1})
        assert compare("x", better, base) == [], key
    # The uncompared figures may move either way: cols and rows trade against each other within the area.
    assert compare("x", dict(base, cols=20, rows=10, blocks=99, routing=[9]), base) == []


def test_a_clocked_row_wants_a_wider_window_and_a_hold_slack_at_or_above_zero():
    base = measure(clocked(), MODULE)
    assert compare("c", dict(base, input_window=16), base) == ["c: input_window 17 -> 16, worse than the baseline"]
    assert compare("c", dict(base, input_window=18), base) == []
    assert compare("c", dict(base, min_hold_slack=0), base) == []
    assert compare("c", dict(base, min_hold_slack=-1), base) == ["c: min_hold_slack -1 is below zero"]
    assert compare("c", dict(base, period=90, skew=8), base) == []
    assert compare("c", dict(base, period=94), base) == ["c: period 92 -> 94, worse than the baseline"]


def test_another_netlist_or_placement_style_is_not_comparable():
    base = measure(combinational(), MODULE)
    assert compare("x", dict(base, cells=3), base) == ["x: cells 4 -> 3, another layout; re-record the row if that is meant"]
    assert compare("x", dict(base, placement="packed"), base) == [
        "x: placement 'pitch4' -> 'packed', another layout; re-record the row if that is meant"]
    clocked_row = measure(clocked(), MODULE)
    found = compare("x", clocked_row, base)
    assert "x: input_spacing is 8 in the baseline and None in the build" in found
    assert "x: period is None in the baseline and 92 in the build" in found


def test_the_file_has_one_canonical_form(tmp_path: Path):
    rows = {"b": measure(clocked(), MODULE), "a": measure(combinational(), MODULE)}
    text = format_baseline(rows)
    assert text.startswith('{\n  "format": "redfab-area-baseline/1",\n')
    assert '"routing": [1, 1, 2, 2, 2, 1]' in text, "lists stay on one line"
    assert text.index('"a": {') < text.index('"b": {'), "designs are sorted by name"
    assert json.loads(text)["designs"] == rows
    path = tmp_path / "area.json"
    path.write_text(text)
    assert format_baseline(load_baseline(path)) == text


def test_the_control_claims_one_repeater_fewer_and_the_real_row_fails_against_it():
    base = measure(combinational(), MODULE)
    rows = {"x": base, "y": measure(clocked(), MODULE)}
    tampered = control(rows, "x")
    assert tampered["x"]["repeaters"] == base["repeaters"] - 1
    assert tampered["y"] == rows["y"] and rows["x"]["repeaters"] == base["repeaters"], "the original is untouched"
    assert compare("x", base, tampered["x"]) == [f"x: repeaters {base['repeaters'] - 1} -> {base['repeaters']}, worse than the baseline"]
    with pytest.raises(SystemExit, match="no baseline row for z"):
        control(rows, "z")


def test_the_committed_baseline_is_canonical_and_lists_exactly_the_shipped_designs():
    text = BASELINE.read_text()
    assert json.loads(text)["format"] == FORMAT
    rows = load_baseline(BASELINE)
    assert format_baseline(rows) == text, "run harness.area_check --update to rewrite it"
    shipped = sorted(p.name for p in (ROOT / "designs").iterdir() if p.is_dir())
    assert sorted(rows) == shipped
    for name, row in rows.items():
        assert row["placement"] in STYLES, name
        if "period" in row:
            assert row["placement"] == "pitch4", f"{name}: clocked designs keep the four-row pitch until step 3"
        assert row["cells"] >= 1 and row["tiles"] >= row["cells"], name
        assert ("period" in row) != ("input_spacing" in row), name
        assert row["area"] == row["footprint"][0] * row["footprint"][1], name


def test_a_missing_or_stale_build_is_named(tmp_path: Path):
    with pytest.raises(FileNotFoundError, match="run `redfab build` first"):
        read_row("x", tmp_path)
    (tmp_path / "x").mkdir()
    design = combinational()
    del design["grid"]
    (tmp_path / "x" / "design.json").write_text(json.dumps(design))
    (tmp_path / "x" / "mapped.json").write_text(json.dumps({"modules": {"top": MODULE}}))
    with pytest.raises(Stale):
        read_row("x", tmp_path)
    (tmp_path / "x" / "design.json").write_text(json.dumps(combinational()))
    assert read_row("x", tmp_path)["tiles"] == 101
