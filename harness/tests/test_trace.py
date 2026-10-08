"""Trace and fixture files: round trips, diffs, expectation checks."""

from harness.blockstate import BlockState
import pytest

from harness.trace import (Expectation, Fixture, ReplaySummary, Trace, check_expectations, diff_states,
                           first_trace_difference, parse_replay_summary)


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


def test_trace_difference_ignores_pending_when_asked_and_reports_a_changed_block():
    a = {"format": "redfab-trace/1", "fixture": "t", "source": {"kind": "sim"}, "size": [1, 1, 1],
         "initial": {"0,0,0": "minecraft:stone"}, "pending0": [], "ticks": []}
    b = dict(a, source={"kind": "vanilla"})
    b["ticks"] = [{"tick": 1, "changes": {}, "pending": [{"pos": "0,0,0", "block": "minecraft:stone", "delay": 1, "priority": 0}]}]
    assert first_trace_difference(Trace.from_json(a, ticks=2), Trace.from_json(b, ticks=2), compare_pending=False) is None
    assert "pending" in first_trace_difference(Trace.from_json(a, ticks=2), Trace.from_json(b, ticks=2), compare_pending=True)
    # A block that differs is reported with its tick, which is what the game-versus-recording comparison rests on.
    c = dict(a, ticks=[{"tick": 1, "changes": {"0,0,0": "minecraft:glass"}}])
    assert first_trace_difference(Trace.from_json(a, ticks=2), Trace.from_json(c, ticks=2)).startswith("tick 1:")


def test_a_lamp_tick_may_differ_only_when_allowed_and_only_for_lamps():
    a = {"format": "redfab-trace/1", "fixture": "t", "source": {"kind": "vanilla"}, "size": [1, 1, 1],
         "initial": {"0,0,0": "minecraft:redstone_lamp"}, "pending0": [], "ticks": []}
    lamp = dict(a, ticks=[{"tick": 1, "changes": {}, "pending": [{"pos": "0,0,0", "block": "minecraft:redstone_lamp", "delay": 4, "priority": 0}]}])
    repeater = dict(a, ticks=[{"tick": 1, "changes": {}, "pending": [{"pos": "0,0,0", "block": "minecraft:repeater", "delay": 2, "priority": -1}]}])
    t = lambda d: Trace.from_json(d, ticks=2)  # noqa: E731
    assert "pending" in first_trace_difference(t(a), t(lamp))
    seen: list[str] = []
    assert first_trace_difference(t(a), t(lamp), lamp_ticks_may_differ=True, artifacts=seen) is None
    assert seen == ["tick 1: a scheduled lamp tick differs with no effect on the blocks"]
    assert "pending" in first_trace_difference(t(a), t(repeater), lamp_ticks_may_differ=True)


def test_replay_summary_is_parsed_and_its_absence_is_an_error():
    line = "build/tiles: 180 matched (2 of them only with a different order of same-tick events), 0 mismatched, 1 skipped (unsupported blocks)\n"
    assert parse_replay_summary("noise\n" + line) == ReplaySummary(matched=180, racy=2, mismatched=0, skipped=1)
    with pytest.raises(ValueError):
        parse_replay_summary("thread 'main' panicked")


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
