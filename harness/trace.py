"""Fixture and trace files, as described in docs/trace-format.md."""

from __future__ import annotations

import re
from dataclasses import dataclass, field

from .anvil import PendingTick
from .blockstate import AIR, BlockState, Pos, format_pos, parse_pos

FIXTURE_FORMAT = "redfab-fixture/1"
TRACE_FORMAT = "redfab-trace/1"

States = dict[Pos, BlockState]


def _states_to_json(states: States) -> dict[str, str]:
    return {format_pos(pos): str(state) for pos, state in sorted(states.items())}


def _states_from_json(data: dict[str, str]) -> States:
    return {parse_pos(k): BlockState.parse(v) for k, v in data.items()}


def diff_states(before: States, after: States) -> States:
    """Blocks whose state differs; blocks that vanished are reported as air."""
    changes: States = {}
    for pos, state in after.items():
        if before.get(pos) != state:
            changes[pos] = state
    for pos in before:
        if pos not in after:
            changes[pos] = AIR
    return changes


def is_powered(state: BlockState) -> bool:
    """What `{"powered": true}` means for each kind of block."""
    name = state.name
    if name == "minecraft:redstone_wire":
        return int(state.get("power", "0")) > 0
    if name in ("minecraft:redstone_lamp", "minecraft:redstone_torch", "minecraft:redstone_wall_torch"):
        return state.get("lit") == "true"
    if name in ("minecraft:repeater", "minecraft:lever"):
        return state.get("powered") == "true"
    return name == "minecraft:redstone_block"


@dataclass(frozen=True)
class Expectation:
    """A block state at a tick, whether a block is powered at a tick, or
    'stable from tick t to the end'."""

    tick: int
    pos: Pos | None = None
    state: BlockState | None = None
    powered: bool | None = None
    stable: bool = False

    @classmethod
    def from_json(cls, data: dict) -> Expectation:
        if data.get("stable"):
            return cls(tick=int(data["from"]), stable=True)
        if "powered" in data:
            return cls(tick=int(data["tick"]), pos=parse_pos(data["pos"]), powered=bool(data["powered"]))
        return cls(tick=int(data["tick"]), pos=parse_pos(data["pos"]), state=BlockState.parse(data["state"]))

    def to_json(self) -> dict:
        if self.stable:
            return {"from": self.tick, "stable": True}
        if self.powered is not None:
            return {"tick": self.tick, "pos": format_pos(self.pos), "powered": self.powered}
        return {"tick": self.tick, "pos": format_pos(self.pos), "state": str(self.state)}

    def check(self, trace: Trace) -> str | None:
        """None if satisfied, else a one-line description of the failure."""
        if self.stable:
            baseline = trace.state_at(self.tick)
            for t in range(self.tick, trace.ticks + 1):
                if trace.pending_at(t):
                    return f"expected stable from tick {self.tick}: tick {t} has pending ticks {[p.to_json() for p in trace.pending_at(t)]}"
                if t > self.tick and trace.changes_at(t):
                    return f"expected stable from tick {self.tick}: tick {t} changed {_states_to_json(trace.changes_at(t))}"
            del baseline
            return None
        actual = trace.state_at(self.tick).get(self.pos, AIR)
        if self.powered is not None:
            if is_powered(actual) != self.powered:
                want = "powered" if self.powered else "unpowered"
                return f"tick {self.tick} at {format_pos(self.pos)}: expected {want}, got {actual}"
            return None
        if not actual.matches(self.state):
            return f"tick {self.tick} at {format_pos(self.pos)}: expected {self.state}, got {actual}"
        return None


@dataclass
class Fixture:
    name: str
    doc: str
    size: tuple[int, int, int]
    placement: str  # "update" | "strict"
    blocks: States
    events: list[tuple[int, States]]
    ticks: int
    expect: list[Expectation] = field(default_factory=list)

    @classmethod
    def from_json(cls, data: dict) -> Fixture:
        if data.get("format") != FIXTURE_FORMAT:
            raise ValueError(f"unexpected fixture format {data.get('format')!r}")
        if data["placement"] not in ("update", "strict"):
            raise ValueError(f"bad placement {data['placement']!r}")
        return cls(
            name=data["name"],
            doc=data.get("doc", ""),
            size=tuple(data["size"]),
            placement=data["placement"],
            blocks=_states_from_json(data["blocks"]),
            events=[(int(e["tick"]), _states_from_json(e["set"])) for e in data.get("events", [])],
            ticks=int(data["ticks"]),
            expect=[Expectation.from_json(e) for e in data.get("expect", [])],
        )

    def to_json(self) -> dict:
        return {
            "format": FIXTURE_FORMAT,
            "name": self.name,
            "doc": self.doc,
            "size": list(self.size),
            "placement": self.placement,
            "blocks": _states_to_json(self.blocks),
            "events": [{"tick": t, "set": _states_to_json(s)} for t, s in sorted(self.events, key=lambda e: e[0])],
            "ticks": self.ticks,
            "expect": [e.to_json() for e in self.expect],
        }

    @property
    def box(self) -> tuple[Pos, Pos]:
        sx, sy, sz = self.size
        return ((0, 0, 0), (sx - 1, sy - 1, sz - 1))


@dataclass
class TickEntry:
    changes: States
    pending: list[PendingTick] | None  # None: the source cannot report pending ticks


@dataclass
class Trace:
    fixture: str
    source: dict
    size: tuple[int, int, int]
    initial: States
    pending0: list[PendingTick] | None
    entries: dict[int, TickEntry]
    ticks: int

    @classmethod
    def from_json(cls, data: dict, ticks: int) -> Trace:
        if data.get("format") != TRACE_FORMAT:
            raise ValueError(f"unexpected trace format {data.get('format')!r}")
        entries = {}
        for e in data.get("ticks", []):
            pending = [PendingTick.from_json(p) for p in e["pending"]] if "pending" in e else None
            entries[int(e["tick"])] = TickEntry(_states_from_json(e.get("changes", {})), pending)
        pending0 = [PendingTick.from_json(p) for p in data["pending0"]] if "pending0" in data else None
        return cls(data["fixture"], data["source"], tuple(data["size"]), _states_from_json(data["initial"]),
                   pending0, entries, ticks)

    def to_json(self) -> dict:
        out = {
            "format": TRACE_FORMAT,
            "fixture": self.fixture,
            "source": self.source,
            "size": list(self.size),
            "initial": _states_to_json(self.initial),
        }
        if self.pending0 is not None:
            out["pending0"] = [p.to_json() for p in self.pending0]
        ticks = []
        for t in sorted(self.entries):
            entry = self.entries[t]
            if not entry.changes and not entry.pending:
                continue
            item: dict = {"tick": t, "changes": _states_to_json(entry.changes)}
            if entry.pending is not None:
                item["pending"] = [p.to_json() for p in entry.pending]
            ticks.append(item)
        out["ticks"] = ticks
        return out

    # --- recording

    @classmethod
    def start(cls, fixture: str, source: dict, size: tuple[int, int, int], initial: States,
              pending0: list[PendingTick] | None, ticks: int) -> Trace:
        return cls(fixture, source, size, dict(initial), pending0, {}, ticks)

    def record(self, tick: int, states: States, pending: list[PendingTick] | None) -> None:
        previous = self.state_at(tick - 1)
        self.entries[tick] = TickEntry(diff_states(previous, states), pending)

    # --- queries

    def changes_at(self, tick: int) -> States:
        entry = self.entries.get(tick)
        return entry.changes if entry else {}

    def pending_at(self, tick: int) -> list[PendingTick]:
        if tick == 0:
            return self.pending0 or []
        entry = self.entries.get(tick)
        return (entry.pending or []) if entry else []

    def state_at(self, tick: int) -> States:
        states = dict(self.initial)
        for t in range(1, tick + 1):
            for pos, state in self.changes_at(t).items():
                if state.is_air:
                    states.pop(pos, None)
                else:
                    states[pos] = state
        return states

    def has_pending_info(self) -> bool:
        return self.pending0 is not None


def first_trace_difference(a: Trace, b: Trace, compare_pending: bool = True) -> str | None:
    if a.initial != b.initial:
        return "initial state differs: " + _describe_diff(a.initial, b.initial)
    ticks = max(a.ticks, b.ticks)
    sa, sb = dict(a.initial), dict(b.initial)
    for t in range(1, ticks + 1):
        sa, sb = a.state_at(t), b.state_at(t)
        if sa != sb:
            return f"tick {t}: " + _describe_diff(sa, sb)
        if compare_pending and a.has_pending_info() and b.has_pending_info():
            if sorted(p.to_json().items() for p in a.pending_at(t)) != sorted(p.to_json().items() for p in b.pending_at(t)):
                return f"tick {t}: pending ticks differ: {[p.to_json() for p in a.pending_at(t)]} vs {[p.to_json() for p in b.pending_at(t)]}"
    return None


def _describe_diff(a: States, b: States) -> str:
    diffs = []
    for pos in sorted(set(a) | set(b)):
        if a.get(pos, AIR) != b.get(pos, AIR):
            diffs.append(f"{format_pos(pos)}: {a.get(pos, AIR)} vs {b.get(pos, AIR)}")
        if len(diffs) >= 5:
            break
    return "; ".join(diffs)


def check_expectations(trace: Trace, expectations: list[Expectation]) -> list[str]:
    return [failure for e in expectations if (failure := e.check(trace)) is not None]


@dataclass(frozen=True)
class ReplaySummary:
    """The counts `redfab check-traces` prints on its last line."""
    matched: int
    racy: int  # matched only with another order of same-tick events: a race in the circuit
    mismatched: int
    skipped: int


def parse_replay_summary(stdout: str) -> ReplaySummary:
    """Reads the summary line of `redfab check-traces`. Raises if there is none, so a
    crash or a changed format cannot pass for a clean replay."""
    found = re.search(r"(\d+) matched \((\d+) of them only with a different order of same-tick events\), "
                      r"(\d+) mismatched, (\d+) skipped", stdout)
    if found is None:
        raise ValueError("check-traces printed no summary line:\n" + stdout.strip())
    return ReplaySummary(*(int(v) for v in found.groups()))
