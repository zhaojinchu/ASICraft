"""Tick-exact control of a running vanilla server: freeze, place, step, snapshot.

The server is frozen for the whole session. Block placement and other commands
run between ticks, `step(n)` runs exactly n game ticks, and `snapshot()` saves
the world and reads every block state (and pending block tick) back from disk.
"""

from __future__ import annotations

import time

from .anvil import Box, PendingTick, SnapshotReader, StaleSnapshot
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
SNAPSHOT_TIMEOUT = 30.0  # seconds to wait for a save to reach the disk


class SessionError(Exception):
    pass


class Session:
    def __init__(self, server: VanillaServer):
        self.server = server
        self.region_dir = server.run_dir / "world" / "dimensions" / "minecraft" / "overworld" / "region"
        self.reader = SnapshotReader(self.region_dir)
        self.commands_sent = 0
        self.stale_snapshots = 0  # reads that caught the save still in progress

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
        """Keep the chunks under the box loaded. One command covers at most 256
        chunks, so a large box is loaded in strips."""
        (x0, _, z0), (x1, _, z1) = box
        chunks_x = x1 // 16 - x0 // 16 + 1
        rows_per_strip = max(1, 256 // chunks_x)
        z = z0
        while z <= z1:
            z_end = min(z1, (z // 16 + rows_per_strip) * 16 - 1)
            reply = self.command(f"forceload add {x0} {z} {x1} {z_end}")
            if "force loaded" not in reply and not reply.startswith("No chunks were marked"):  # all loaded already
                raise SessionError(f"forceload: {reply!r}")
            z = z_end + 1

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

    def snapshot(self, boxes: list[Box], fresh: bool = False) -> tuple[dict[Pos, BlockState], list[PendingTick]]:
        """Save the world and read back every block and pending tick inside the boxes.

        The save goes on in the background after the command replies, so a read
        can catch a chunk from before it, or half written. With `fresh`, every
        chunk inside the boxes must have been saved at the current game time
        (the caller has made sure each of them changed this tick), and the read
        is repeated until that holds."""
        reply = self.command("save-all flush")
        if "Saved the game" not in reply:
            raise SessionError(f"save-all: {reply!r}")
        if not fresh:
            # Chunks that did not change are not written again, so there is no stamp
            # to wait for; wait for the region files to stop changing instead.
            deadline = time.monotonic() + SNAPSHOT_TIMEOUT
            stamps = None
            while True:
                time.sleep(0.1)
                now = {f: f.stat().st_mtime_ns for f in self.region_dir.glob("r.*.mca")}
                if now == stamps:
                    try:
                        return self.reader.read(boxes)
                    except StaleSnapshot:
                        self.stale_snapshots += 1
                if time.monotonic() > deadline:
                    raise SessionError(f"the save did not complete within {SNAPSHOT_TIMEOUT}s")
                stamps = now
        saved_at = self.gametime()
        deadline = time.monotonic() + SNAPSHOT_TIMEOUT
        while True:
            try:
                return self.reader.read(boxes, saved_at=saved_at)
            except StaleSnapshot as e:
                self.stale_snapshots += 1
                if time.monotonic() > deadline:
                    raise SessionError(f"the save did not complete within {SNAPSHOT_TIMEOUT}s: {e}") from e
                time.sleep(0.02)
