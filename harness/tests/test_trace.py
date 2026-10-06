"""Trace and fixture files: round trips, diffs, expectation checks."""

from harness.blockstate import BlockState
from harness.trace import Expectation, Fixture, Trace, check_expectations, diff_states, traces_equal


def _state(s):
    return BlockState.parse(s)


def test_diff_reports_changed_and_removed_blocks():
    before = {(0, 0, 0): _state("minecraft:stone"), (1, 0, 0): _state("minecraft:redstone_wire[power=0]")}
    after = {(0, 0, 0): _state("minecraft:stone"), (1, 0, 0): _state("minecraft:redstone_wire[power=15]")}
    assert diff_states(before, after) == {(1, 0, 0): _state("minecraft:redstone_wire[power=15]")}
    assert diff_states(after, {(0, 0, 0): _state("minecraft:stone")}) == {(1, 0, 0): _state("minecraft:air")}


def test_trace_json_round_trip_and_state_at():
    trace = Trace.from_json(
        {
            "format": "redfab-trace/1",
            "fixture": "t",
            "source": {"kind": "sim"},
            "size": [2, 1, 1],
            "initial": {"0,0,0": "minecraft:stone", "1,0,0": "minecraft:redstone_wire[power=0]"},
            "pending0": [],
            "ticks": [
                {"tick": 3, "changes": {"1,0,0": "minecraft:redstone_wire[power=15]"},
                 "pending": [{"pos": "0,0,0", "block": "minecraft:redstone_torch", "delay": 2, "priority": 0}]},
                {"tick": 5, "changes": {"1,0,0": "minecraft:air"}},
            ],
        },
        ticks=6,
    )
    assert trace.to_json() == Trace.from_json(trace.to_json(), ticks=6).to_json()
    assert str(trace.state_at(2)[(1, 0, 0)]) == "minecraft:redstone_wire[power=0]"
    assert str(trace.state_at(4)[(1, 0, 0)]) == "minecraft:redstone_wire[power=15]"
    assert (1, 0, 0) not in trace.state_at(6)
    assert trace.pending_at(3)[0].delay == 2 and trace.pending_at(4) == []


def test_traces_equal_ignores_pending_when_one_side_lacks_it():
    a = {"format": "redfab-trace/1", "fixture": "t", "source": {"kind": "sim"}, "size": [1, 1, 1],
         "initial": {"0,0,0": "minecraft:stone"}, "pending0": [], "ticks": []}
    b = dict(a, source={"kind": "vanilla"})
    b["ticks"] = [{"tick": 1, "changes": {}, "pending": [{"pos": "0,0,0", "block": "minecraft:stone", "delay": 1, "priority": 0}]}]
    assert traces_equal(Trace.from_json(a, ticks=2), Trace.from_json(b, ticks=2), compare_pending=False)
    assert not traces_equal(Trace.from_json(a, ticks=2), Trace.from_json(b, ticks=2), compare_pending=True)


def test_expectations_pass_and_fail():
    trace = Trace.from_json(
        {"format": "redfab-trace/1", "fixture": "t", "source": {"kind": "sim"}, "size": [1, 1, 1],
         "initial": {"0,0,0": "minecraft:redstone_lamp[lit=false]"}, "pending0": [],
         "ticks": [{"tick": 2, "changes": {"0,0,0": "minecraft:redstone_lamp[lit=true]"}}]},
        ticks=5,
    )
    ok = [Expectation.from_json({"tick": 2, "pos": "0,0,0", "state": "minecraft:redstone_lamp[lit=true]"}),
          Expectation.from_json({"tick": 1, "pos": "0,0,0", "state": "minecraft:redstone_lamp[lit=false]"}),
          Expectation.from_json({"from": 2, "stable": True})]
    assert check_expectations(trace, ok) == []
    bad = [Expectation.from_json({"tick": 1, "pos": "0,0,0", "state": "minecraft:redstone_lamp[lit=true]"}),
           Expectation.from_json({"from": 1, "stable": True})]
    failures = check_expectations(trace, bad)
    assert len(failures) == 2 and "tick 1" in failures[0]


def test_powered_expectations():
    trace = Trace.from_json(
        {"format": "redfab-trace/1", "fixture": "t", "source": {"kind": "sim"}, "size": [1, 1, 1],
         "initial": {"0,0,0": "minecraft:redstone_wire[east=side,north=none,power=0,south=none,west=side]"}, "pending0": [],
         "ticks": [{"tick": 1, "changes": {"0,0,0": "minecraft:redstone_wire[east=side,north=none,power=7,south=none,west=side]"}}]},
        ticks=2,
    )
    assert check_expectations(trace, [Expectation.from_json({"tick": 0, "pos": "0,0,0", "powered": False}),
                                      Expectation.from_json({"tick": 1, "pos": "0,0,0", "powered": True})]) == []
    assert len(check_expectations(trace, [Expectation.from_json({"tick": 1, "pos": "0,0,0", "powered": False})])) == 1


def test_fixture_round_trip():
    fx = Fixture.from_json(
        {"format": "redfab-fixture/1", "name": "x", "doc": "d", "size": [1, 1, 1], "placement": "update",
         "blocks": {"0,0,0": "minecraft:stone"}, "events": [{"tick": 1, "set": {"0,0,0": "minecraft:air"}}],
         "ticks": 3, "expect": []}
    )
    assert Fixture.from_json(fx.to_json()).to_json() == fx.to_json()
