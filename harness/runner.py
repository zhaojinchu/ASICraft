"""Runs many fixtures side by side on one vanilla server, in lockstep.

Every fixture is placed in its own plot. All plots share the game's tick clock:
each tick, the runner applies every plot's stimuli, steps the game once, saves
the world, and records each plot's new state in that fixture's own coordinate
frame. A plot can be placed rotated or at any position; results are mapped back
so traces from different placements compare directly.
"""

from __future__ import annotations

import time
from dataclasses import dataclass

from .anvil import Box, PendingTick
from .blockstate import AIR, BlockState, Pos, Rotation
from .session import Session, SessionError
from .trace import Fixture, States, Trace

PLOT_Y = 64            # all plots sit at this height, in an otherwise empty world
MARKER_Y = PLOT_Y - 8  # marker blocks that force every plot chunk to be re-saved each tick
CELL = 32              # distance between plot cells (two chunks)
SETTLE_TICKS = 10      # run after update-placement so torches and repeaters reach rest
GLASS = BlockState("minecraft:glass")

# Power sources go in last under update placement, so every component sees its
# final neighbourhood when it is first evaluated.
_SOURCES = {"minecraft:redstone_block", "minecraft:lever", "minecraft:redstone_torch", "minecraft:redstone_wall_torch"}
_COMPONENTS = {
    "minecraft:redstone_lamp", "minecraft:redstone_torch", "minecraft:redstone_wall_torch",
    "minecraft:repeater", "minecraft:comparator", "minecraft:redstone_wire",
}


@dataclass(frozen=True)
class Placement:
    """Where and how a fixture sits in the world."""

    rotation: Rotation
    origin: Pos  # world position of the rotated fixture's minimum corner
    size: tuple[int, int, int]

    def _rotated_min(self) -> Pos:
        sx, sy, sz = self.size
        corners = [self.rotation.apply_pos(p) for p in ((0, 0, 0), (sx - 1, sy - 1, sz - 1))]
        return tuple(min(c[i] for c in corners) for i in range(3))  # type: ignore[return-value]

    def to_world(self, pos: Pos) -> Pos:
        r = self.rotation.apply_pos(pos)
        m = self._rotated_min()
        return (self.origin[0] + r[0] - m[0], self.origin[1] + r[1] - m[1], self.origin[2] + r[2] - m[2])

    def to_fixture(self, world: Pos) -> Pos:
        m = self._rotated_min()
        r = (world[0] - self.origin[0] + m[0], world[1] - self.origin[1] + m[1], world[2] - self.origin[2] + m[2])
        return self.rotation.inverse().apply_pos(r)

    def world_box(self) -> Box:
        sx, sy, sz = self.size
        a = self.to_world((0, 0, 0))
        b = self.to_world((sx - 1, sy - 1, sz - 1))
        return (tuple(min(a[i], b[i]) for i in range(3)), tuple(max(a[i], b[i]) for i in range(3)))  # type: ignore[return-value]

    def state_to_world(self, state: BlockState) -> BlockState:
        return self.rotation.apply_state(state)

    def state_to_fixture(self, state: BlockState) -> BlockState:
        return self.rotation.inverse().apply_state(state)


@dataclass
class Plot:
    fixture: Fixture
    variant: str
    placement: Placement


def layout(fixtures: list[Fixture], variants: tuple[str, ...] = ("aligned", "straddle", "rotated")) -> list[Plot]:
    """One plot per fixture per variant, on a grid of CELL-sized cells.

    aligned:  origin at a chunk corner.
    straddle: origin shifted so the plot crosses a chunk border on both axes.
    rotated:  a quarter turn clockwise, at a chunk corner.
    """
    plots = []
    n = 0
    per_row = 8
    for fx in fixtures:
        sx, _, sz = fx.size
        if sx > CELL - 8 or sz > CELL - 8:
            raise ValueError(f"fixture {fx.name} is too large for a {CELL}-block cell")
        for variant in variants:
            base_x, base_z = (n % per_row) * CELL, (n // per_row) * CELL
            n += 1
            if variant == "aligned":
                plots.append(Plot(fx, variant, Placement(Rotation(0), (base_x, PLOT_Y, base_z), fx.size)))
            elif variant == "straddle":
                origin = (base_x + 16 - max(1, sx // 2), PLOT_Y, base_z + 16 - max(1, sz // 2))
                plots.append(Plot(fx, variant, Placement(Rotation(0), origin, fx.size)))
            elif variant == "rotated":
                plots.append(Plot(fx, variant, Placement(Rotation(1), (base_x, PLOT_Y, base_z), fx.size)))
            else:
                raise ValueError(variant)
    return plots


def _placement_order(name: str, placement: str) -> int:
    if placement == "strict":
        return 0
    return 2 if name in _SOURCES else 1 if name in _COMPONENTS else 0


class Runner:
    def __init__(self, session: Session, source: dict):
        self.session = session
        self.source = source

    def run(self, plots: list[Plot], log=print) -> dict[tuple[str, str], Trace]:
        session = self.session
        if not plots:
            return {}
        boxes = [p.placement.world_box() for p in plots]
        chunks: set[tuple[int, int]] = set()
        for box in boxes:
            (x0, _, z0), (x1, _, z1) = box
            session.forceload(((x0 - 16, 0, z0 - 16), (x1 + 16, 0, z1 + 16)))
            chunks.update((cx, cz) for cx in range(x0 >> 4, (x1 >> 4) + 1) for cz in range(z0 >> 4, (z1 >> 4) + 1))
        # One marker line per chunk row, spanning every chunk column in use.
        rows: dict[int, tuple[int, int]] = {}
        for cx, cz in chunks:
            lo, hi = rows.get(cz, (cx, cx))
            rows[cz] = (min(lo, cx), max(hi, cx))
        self._marker_rows = sorted(rows.items())
        self._marker_phase = 0

        # --- place
        t0 = time.monotonic()
        for plot in plots:
            fx, pl = plot.fixture, plot.placement
            strict = fx.placement == "strict"
            order = sorted(fx.blocks.items(), key=lambda kv: (kv[0][1], _placement_order(kv[1].name, fx.placement), kv[0]))
            for pos, state in order:
                session.setblock(pl.to_world(pos), pl.state_to_world(state), strict)
        if any(p.fixture.placement == "update" for p in plots):
            # Nudge every component once more so blocks whose neighbours were
            # reshaped after they were evaluated get re-evaluated (see runner docs).
            for plot in plots:
                if plot.fixture.placement != "update":
                    continue
                pl = plot.placement
                for pos, state in plot.fixture.blocks.items():
                    if state.name in _COMPONENTS:
                        above = (pos[0], pos[1] + 1, pos[2])
                        if above not in plot.fixture.blocks:
                            wp = pl.to_world(above)
                            session.setblock(wp, GLASS, False)
                            session.setblock(wp, AIR, False)
            session.step(SETTLE_TICKS)
        log(f"placed {len(plots)} plots in {time.monotonic() - t0:.1f}s")

        # --- tick 0 (stimuli listed for tick 0 are applied before it is recorded)
        self._apply_events(plots, 0)
        blocks, pending = self._snapshot(boxes)
        traces: dict[tuple[str, str], Trace] = {}
        for plot in plots:
            states, pend = self._extract(plot, blocks, pending)
            if plot.fixture.placement == "update" and pend:
                raise SessionError(f"{plot.fixture.name}/{plot.variant} did not settle: pending {[p.to_json() for p in pend]}")
            traces[(plot.fixture.name, plot.variant)] = Trace.start(
                plot.fixture.name, self.source, plot.fixture.size, states, pend, plot.fixture.ticks
            )

        # --- lockstep
        total = max(p.fixture.ticks for p in plots)
        t0 = time.monotonic()
        for tick in range(1, total + 1):
            # Step first, then apply this tick's stimuli: a command runs while the game
            # clock still reads the last completed tick, so a stimulus applied here acts
            # as the last event of tick `tick`, and a component with delay d reacts at tick+d.
            session.step(1)
            self._apply_events(plots, tick)
            blocks, pending = self._snapshot(boxes)
            for plot in plots:
                if tick <= plot.fixture.ticks:
                    states, pend = self._extract(plot, blocks, pending)
                    traces[(plot.fixture.name, plot.variant)].record(tick, states, pend)
            if tick % 50 == 0:
                log(f"  tick {tick}/{total} ({time.monotonic() - t0:.1f}s)")
        log(f"ran {total} ticks in {time.monotonic() - t0:.1f}s, {session.commands_sent} commands so far")
        return traces

    def _snapshot(self, boxes: list[Box]):
        """Save and read back. `save-all` only writes chunks with block changes, and a
        newly scheduled block tick alone does not count as one, so a chunk's saved
        pending-tick list can be stale. Changing a marker block far below each plot
        chunk forces every one of them to be written."""
        self._marker_phase ^= 1
        marker = BlockState("minecraft:stone" if self._marker_phase else "minecraft:glass")
        for cz, (cx0, cx1) in self._marker_rows:
            self.session.fill(((cx0 * 16, MARKER_Y, cz * 16), (cx1 * 16 + 15, MARKER_Y, cz * 16)), marker, True)
        return self.session.snapshot(boxes)

    def _apply_events(self, plots: list[Plot], tick: int) -> None:
        for plot in plots:
            for ev_tick, changes in plot.fixture.events:
                if ev_tick == tick:
                    pl = plot.placement
                    for pos, state in sorted(changes.items(), key=lambda kv: kv[0][1]):
                        self.session.setblock(pl.to_world(pos), pl.state_to_world(state), False)

    @staticmethod
    def _extract(plot: Plot, blocks: dict[Pos, BlockState], pending: list[PendingTick]) -> tuple[States, list[PendingTick]]:
        pl = plot.placement
        (x0, y0, z0), (x1, y1, z1) = pl.world_box()
        states: States = {}
        for pos, state in blocks.items():
            if x0 <= pos[0] <= x1 and y0 <= pos[1] <= y1 and z0 <= pos[2] <= z1:
                states[pl.to_fixture(pos)] = pl.state_to_fixture(state)
        pend = [
            PendingTick(pl.to_fixture(p.pos), p.block, p.delay, p.priority)
            for p in pending
            if x0 <= p.pos[0] <= x1 and y0 <= p.pos[1] <= y1 and z0 <= p.pos[2] <= z1
        ]
        pend.sort(key=lambda t: (t.pos, t.block))
        return states, pend
