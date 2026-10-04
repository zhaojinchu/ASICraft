"""Tick-exact control of a running vanilla server: freeze, place, step, snapshot.

The server is frozen for the whole session. Block placement and other commands
run between ticks, `step(n)` runs exactly n game ticks, and `snapshot()` saves
the world and reads every block state (and pending block tick) back from disk.
"""

from __future__ import annotations

import time

from .anvil import Box, PendingTick, SnapshotReader
from .blockstate import BlockState, Pos
from .server import VanillaServer

# Values that keep the world inert: nothing spawns, nothing grows, time and
# weather stand still, broken blocks drop nothing, and admin commands stay out of the log.
GAMERULES = {
    "random_tick_speed": "0",
    "advance_time": "false",
    "advance_weather": "false",
    "spawn_mobs": "false",
    "spawn_monsters": "false",
    "spawn_phantoms": "false",
    "spawn_patrols": "false",
    "spawn_wandering_traders": "false",
    "spawn_wardens": "false",
    "mob_griefing": "false",
    "block_drops": "false",
    "entity_drops": "false",
    "log_admin_commands": "false",
    "fire_spread_radius_around_player": "0",
}

# Fastest rate the game allows; stepping then takes ~0.1 ms per tick instead of 50 ms.
TICK_RATE = 10000


class SessionError(Exception):
    pass


class Session:
    def __init__(self, server: VanillaServer):
        self.server = server
        self.region_dir = server.run_dir / "world" / "dimensions" / "minecraft" / "overworld" / "region"
        self.reader = SnapshotReader(self.region_dir)
        self.commands_sent = 0

    # --- basics

    def command(self, text: str) -> str:
        self.commands_sent += 1
        return self.server.command(text)

    def prepare(self) -> None:
        for rule, value in GAMERULES.items():
            reply = self.command(f"gamerule {rule} {value}")
            if "is now set to" not in reply:
                raise SessionError(f"gamerule {rule}: {reply!r}")
        self.command("tick freeze")
        self.command(f"tick rate {TICK_RATE}")
        if "frozen" not in self.command("tick query"):
            raise SessionError("game did not freeze")

    def gametime(self) -> int:
        reply = self.command("time query gametime")  # "The game time is N tick(s)"
        return int(reply.split()[4])

    def step(self, ticks: int, timeout: float = 60.0) -> None:
        """Run exactly `ticks` game ticks. The server steps asynchronously at the
        tick rate, so poll the game clock until it has advanced by exactly `ticks`."""
        if ticks <= 0:
            return
        start = self.gametime()
        reply = self.command(f"tick step {ticks}")
        if not reply.startswith("Stepping"):
            raise SessionError(f"tick step: {reply!r}")
        deadline = time.monotonic() + timeout
        while True:
            now = self.gametime()
            if now == start + ticks:
                return
            if now > start + ticks:
                raise SessionError(f"stepped {now - start} ticks, asked for {ticks}")
            if time.monotonic() > deadline:
                raise SessionError(f"step of {ticks} ticks did not finish in {timeout}s (at {now - start})")
            time.sleep(0.002)

    # --- world

    def forceload(self, box: Box) -> None:
        (x0, _, z0), (x1, _, z1) = box
        reply = self.command(f"forceload add {x0} {z0} {x1} {z1}")
        if "force loaded" not in reply:
            raise SessionError(f"forceload: {reply!r}")

    def setblock(self, pos: Pos, state: BlockState, strict: bool) -> bool:
        """Place one block. Returns False if the block was already in that state."""
        x, y, z = pos
        reply = self.command(f"setblock {x} {y} {z} {state}{' strict' if strict else ''}")
        if reply.startswith("Changed the block"):
            return True
        if reply.startswith("Could not set the block"):
            return False
        raise SessionError(f"setblock {pos} {state}: {reply!r}")

    def fill(self, box: Box, state: BlockState, strict: bool) -> None:
        (x0, y0, z0), (x1, y1, z1) = box
        reply = self.command(f"fill {x0} {y0} {z0} {x1} {y1} {z1} {state}{' strict' if strict else ''}")
        if "block" not in reply:
            raise SessionError(f"fill: {reply!r}")

    def probe(self, pos: Pos, state: BlockState) -> bool:
        """True if the block at pos matches the state (properties not named are ignored)."""
        x, y, z = pos
        reply = self.command(f"execute if block {x} {y} {z} {state}")
        if reply == "Test passed":
            return True
        if reply == "Test failed":
            return False
        raise SessionError(f"probe {pos} {state}: {reply!r}")

    def snapshot(self, boxes: list[Box]) -> tuple[dict[Pos, BlockState], list[PendingTick]]:
        """Save the world and read back every block and pending tick inside the boxes."""
        reply = self.command("save-all flush")
        if "Saved the game" not in reply:
            raise SessionError(f"save-all: {reply!r}")
        return self.reader.read(boxes)
