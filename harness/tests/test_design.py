from pathlib import Path

import pytest

from harness.design import SIM_SAMPLE, all_vectors, every_pair_walk, sampled_vectors, sim_vectors

MODULE = {"ports": {"a": {"direction": "input", "bits": [2, 3, 4]}, "b": {"direction": "input", "bits": [5, 6, 7]},
                    "y": {"direction": "output", "bits": [8]}}}


def test_all_vectors_covers_every_combination():
    vectors = all_vectors(MODULE)
    assert len(vectors) == 64
    assert len({tuple(sorted(v.items())) for v in vectors}) == 64


def test_sampling_keeps_small_designs_whole():
    assert sampled_vectors(MODULE, 64) == all_vectors(MODULE)


def test_sampling_is_fixed_and_ends_with_everything_on():
    sample = sampled_vectors(MODULE, 32)
    assert len(sample) == 32
    assert sample == sampled_vectors(MODULE, 32)
    assert all(value == 1 for value in sample[-1].values())
    assert all(value == 0 for value in sample[0].values())
    assert len({tuple(sorted(v.items())) for v in sample}) == 32


def test_plots_of_large_fixtures_do_not_overlap():
    from harness.runner import layout
    from harness.trace import Fixture

    def fixture(name, sx, sz):
        return Fixture(name=name, doc="", size=(sx, 4, sz), placement="strict", blocks={}, events=[], ticks=1, expect=[])

    plots = layout([fixture("small", 10, 10), fixture("wide", 365, 250), fixture("tall", 25, 70)])
    boxes = [p.placement.world_box() for p in plots]
    for i, a in enumerate(boxes):
        for b in boxes[i + 1:]:
            apart = a[1][0] < b[0][0] or b[1][0] < a[0][0] or a[1][2] < b[0][2] or b[1][2] < a[0][2]
            assert apart, (a, b)
    for plot, box in zip(plots, boxes):
        if plot.variant == "straddle":
            (x0, _, z0), (x1, _, z1) = box
            assert x0 // 16 != x1 // 16 and z0 // 16 != z1 // 16, box
        if plot.variant == "aligned":
            assert box[0][0] % 16 == 0 and box[0][2] % 16 == 0


def test_lamp_changes_are_measured_from_the_last_input_change():
    from harness.design import lamp_changes
    from harness.trace import Fixture, Trace

    design = {"outputs": [{"name": "y", "lamp": "5,1,2"}]}
    fixture = Fixture(name="t", doc="", size=(8, 4, 8), placement="strict", blocks={}, ticks=40,
                      events=[(2, {}), (20, {})], expect=[])
    trace = Trace.from_json({
        "format": "redfab-trace/1", "fixture": "t", "source": {}, "size": [8, 4, 8], "initial": {},
        "ticks": [{"tick": 8, "changes": {"5,1,2": "minecraft:redstone_lamp[lit=true]"}},
                  {"tick": 12, "changes": {"0,1,0": "minecraft:stone"}},
                  {"tick": 30, "changes": {"5,1,2": "minecraft:redstone_lamp[lit=false]"}}],
    }, 40)
    assert lamp_changes(fixture, trace, design) == {"y": [6, 10]}


def test_a_design_is_found_by_name_or_by_path(tmp_path):
    from harness.design import DESIGNS, design_dir, design_name, load

    assert design_dir("nor2") == DESIGNS / "nor2" and design_name("nor2") == "nor2"
    built = tmp_path / "My Adder"
    built.mkdir()
    (built / "design.json").write_text('{"name": "top"}')
    (built / "mapped.json").write_text('{"modules": {"top": {"ports": {}}}}')
    assert design_dir(str(built)) == built.resolve()
    assert design_name(str(built)) == "my_adder"
    assert load(str(built)) == ({"name": "top"}, {"ports": {}})


def test_port_bits_are_named_by_the_declared_index():
    from harness.design import bit_names

    module = {"ports": {
        "a": {"direction": "input", "bits": [2, 3, 4, 5]},
        "b": {"direction": "input", "bits": [6, 7, 8, 9], "offset": 4},
        "y": {"direction": "output", "bits": [10, 11, 12, 13], "upto": 1},
        "c": {"direction": "output", "bits": [14]},
    }}
    assert bit_names(module, "a") == ["a[0]", "a[1]", "a[2]", "a[3]"]
    assert bit_names(module, "b") == ["b[4]", "b[5]", "b[6]", "b[7]"]
    assert bit_names(module, "y") == ["y[3]", "y[2]", "y[1]", "y[0]"]
    assert bit_names(module, "c") == ["c"]


def test_the_cycle_testbench_makes_the_active_edge_first():
    from harness.design import cycles_testbench

    module = {"ports": {"clk": {"direction": "input", "bits": [2]}, "en": {"direction": "input", "bits": [3]},
                        "q": {"direction": "output", "bits": [4, 5]}}}
    falling = cycles_testbench(module, "clk", "falling", [{"en": 1}])
    assert "logic clk = 1;" in falling
    assert falling.index("clk = 0; #1;") < falling.index("en = 1'd1;")
    assert "clk = 1; #5;" in falling
    rising = cycles_testbench(module, "clk", "rising", [{"en": 1}])
    assert "logic clk = 0;" in rising
    assert "clk = 1; #1;" in rising


def test_verilator_runs_in_a_build_directory_with_a_space_in_its_path(tmp_path: Path):
    # Verilator's generated make does not quote --Mdir, so the harness runs it inside
    # the directory with relative paths; before, `harness.show --self-test` on such a
    # path died with a bare CalledProcessError.
    from harness.design import verilator_outputs

    built = tmp_path / "My Inverter"
    built.mkdir()
    (built / "top.sv").write_text("module top(input logic a, output logic y);\n  assign y = ~a;\nendmodule\n")
    module = {"ports": {"a": {"direction": "input", "bits": [2]}, "y": {"direction": "output", "bits": [3]}}}
    assert verilator_outputs(str(built), module, [{"a": 0}, {"a": 1}]) == [{"y": 1}, {"y": 0}]
    (built / "top.sv").write_text("module top(input logic a, output logic y);\n  assign y = ~a\nendmodule\n")
    with pytest.raises(RuntimeError, match="Verilator failed on .*My Inverter"):
        verilator_outputs(str(built), module, [{"a": 0}])


def test_wide_designs_are_simulated_on_a_fixed_sample():
    narrow = {"ports": {"a": {"direction": "input", "bits": list(range(2, 12))}, "y": {"direction": "output", "bits": [12]}}}
    vectors, sampled = sim_vectors(narrow)
    assert not sampled and len(vectors) == 1024

    wide = {"ports": {"a": {"direction": "input", "bits": list(range(2, 19))},
                      "y": {"direction": "output", "bits": [19]}}}
    vectors, sampled = sim_vectors(wide)
    assert sampled and len(vectors) == SIM_SAMPLE
    names = [f"a[{i}]" for i in range(17)]
    assert all(sorted(v) == sorted(names) for v in vectors)
    assert vectors[0] == dict.fromkeys(names, 0)
    assert vectors[1] == dict.fromkeys(names, 1)
    for i, name in enumerate(names):
        assert vectors[2 + i] == {n: int(n == name) for n in names}, "each bit alone"
    assert len({tuple(v.values()) for v in vectors}) == SIM_SAMPLE, "no vector twice"
    assert sim_vectors(wide)[0] == vectors, "the same sample every run"


def test_the_worst_case_walk_steps_between_every_ordered_pair_once():
    vectors = [{"a": a, "b": b, "c": c} for a in (0, 1) for b in (0, 1) for c in (0, 1)]
    walk = every_pair_walk(vectors)
    assert len(walk) == 8 * 7 + 1 and walk[0] == walk[-1] == vectors[0]
    steps = [(vectors.index(x), vectors.index(y)) for x, y in zip(walk, walk[1:])]
    assert sorted(steps) == sorted((i, j) for i in range(8) for j in range(8) if i != j)
    assert every_pair_walk([]) == [] and every_pair_walk(vectors[:1]) == vectors[:1]
