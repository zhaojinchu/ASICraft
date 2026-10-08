"""The fuzz run's bookkeeping: fixed seeds by default, and dropped arrangements are named."""

from harness.fuzz import VERIFY_SEED, consistent_traces, parse_args
from harness.tiles import DEFAULT_SEED
from harness.tiles import parse_args as tiles_parse_args
from harness.trace import Fixture, Trace


def _trace(name: str, lit: str) -> Trace:
    return Trace.from_json({
        "format": "redfab-trace/1", "fixture": name, "source": {}, "size": [2, 2, 1],
        "initial": {"0,1,0": "minecraft:stone"},
        "ticks": [{"tick": 1, "changes": {"1,1,0": f"minecraft:redstone_lamp[lit={lit}]"}}],
    }, 4)


def test_placement_dependent_arrangements_are_dropped_and_named():
    fxs = [Fixture(name=n, doc="", size=(2, 2, 1), placement="update", blocks={}, events=[], ticks=4, expect=[])
           for n in ("fz_1_000", "fz_1_001")]
    traces = {
        ("fz_1_000", "aligned"): _trace("fz_1_000", "true"), ("fz_1_000", "rotated"): _trace("fz_1_000", "true"),
        ("fz_1_001", "aligned"): _trace("fz_1_001", "true"), ("fz_1_001", "rotated"): _trace("fz_1_001", "false"),
    }
    logged: list[str] = []
    keep, stats = consistent_traces("vanilla", fxs, traces, log=logged.append)
    assert list(keep) == ["fz_1_000"] and stats == {"placement_dependent": 1}
    assert len(logged) == 1 and logged[0].startswith("[vanilla] fz_1_001: placement-dependent, dropped (")


def test_the_seeds_are_fixed_unless_asked_for():
    assert parse_args(["verify"]).seed == VERIFY_SEED
    assert parse_args(["record"]).seed == 1
    assert parse_args(["show"]).seed == 1
    assert parse_args(["show", "--seed", "0"]).seed == 0
    assert parse_args(["verify", "--seed", "11"]).seed == 11
    assert isinstance(parse_args(["verify", "--seed", "random"]).seed, int)
    assert tiles_parse_args(["verify"]).seed == DEFAULT_SEED == 1
    assert tiles_parse_args(["verify", "--serial"]).serial and not tiles_parse_args(["verify"]).serial
