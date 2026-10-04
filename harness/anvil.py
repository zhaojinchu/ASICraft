"""Reads block states and pending block ticks out of a saved world (Anvil region files).

This is how the harness takes a complete snapshot: `save-all flush`, then parse.
Only the chunks and sections that intersect the requested box are decoded.
"""

from __future__ import annotations

import gzip
import zlib
from dataclasses import dataclass
from pathlib import Path

from .blockstate import AIR, BlockState, Pos, format_pos
from .nbt import read_nbt
from .registry import complete

Box = tuple[Pos, Pos]  # inclusive min and max corners

_SECTOR = 4096


@dataclass(frozen=True)
class PendingTick:
    pos: Pos
    block: str
    delay: int
    priority: int

    def to_json(self) -> dict:
        return {"pos": format_pos(self.pos), "block": self.block, "delay": self.delay, "priority": self.priority}

    @classmethod
    def from_json(cls, data: dict) -> PendingTick:
        from .blockstate import parse_pos

        return cls(parse_pos(data["pos"]), data["block"], int(data["delay"]), int(data["priority"]))

    def translated(self, dx: int, dy: int, dz: int) -> PendingTick:
        x, y, z = self.pos
        return PendingTick((x + dx, y + dy, z + dz), self.block, self.delay, self.priority)


def _in_box(pos: Pos, box: Box) -> bool:
    (x0, y0, z0), (x1, y1, z1) = box
    x, y, z = pos
    return x0 <= x <= x1 and y0 <= y <= y1 and z0 <= z <= z1


def _decode_section(section: dict, cx: int, cz: int, boxes: list[Box], out: dict[Pos, BlockState]) -> None:
    states = section.get("block_states")
    if not states:
        return
    # Saved palettes omit properties at their default value; restore them.
    palette = [complete(BlockState.from_palette(entry)) for entry in states["palette"]]
    if len(palette) == 1 and palette[0].is_air:
        return
    base_x, base_y, base_z = cx * 16, section["Y"] * 16, cz * 16
    if len(palette) > 1:
        bits = max(4, (len(palette) - 1).bit_length())
        per_long = 64 // bits
        mask = (1 << bits) - 1
        data = states["data"]
    for (x0, y0, z0), (x1, y1, z1) in boxes:
        xs = range(max(x0, base_x), min(x1, base_x + 15) + 1)
        ys = range(max(y0, base_y), min(y1, base_y + 15) + 1)
        zs = range(max(z0, base_z), min(z1, base_z + 15) + 1)
        if not xs or not ys or not zs:
            continue
        for y in ys:
            for z in zs:
                for x in xs:
                    if len(palette) == 1:
                        block = palette[0]
                    else:
                        i = ((y - base_y) << 8) | ((z - base_z) << 4) | (x - base_x)
                        word = data[i // per_long] & 0xFFFFFFFFFFFFFFFF
                        block = palette[(word >> ((i % per_long) * bits)) & mask]
                    if not block.is_air:
                        out[(x, y, z)] = block


def _bounding(boxes: list[Box]) -> Box:
    return (
        tuple(min(b[0][i] for b in boxes) for i in range(3)),  # type: ignore[return-value]
        tuple(max(b[1][i] for b in boxes) for i in range(3)),
    )


def read_region_blocks(path: Path, box: Box) -> tuple[dict[Pos, BlockState], list[PendingTick]]:
    """All non-air blocks and pending block ticks inside `box` from one region file."""
    return SnapshotReader(path.parent).read([box])


def read_world_blocks(region_dir: Path, box: Box) -> tuple[dict[Pos, BlockState], list[PendingTick]]:
    """Same as read_region_blocks across every region file the box touches."""
    return SnapshotReader(region_dir).read([box])


class SnapshotReader:
    """Reads the blocks inside a set of boxes out of a world directory. Only the
    chunks and cells the boxes cover are decoded; everything else in a chunk is
    skipped (the NBT reader leaves big arrays undecoded)."""

    def __init__(self, region_dir: Path):
        self.region_dir = region_dir

    def read(self, boxes: list[Box]) -> tuple[dict[Pos, BlockState], list[PendingTick]]:
        blocks: dict[Pos, BlockState] = {}
        pending: list[PendingTick] = []
        if not boxes:
            return blocks, pending
        (x0, y0, z0), (x1, y1, z1) = _bounding(boxes)
        by_chunk: dict[tuple[int, int], list[Box]] = {}
        for box in boxes:
            (bx0, _, bz0), (bx1, _, bz1) = box
            for cx in range(bx0 >> 4, (bx1 >> 4) + 1):
                for cz in range(bz0 >> 4, (bz1 >> 4) + 1):
                    by_chunk.setdefault((cx, cz), []).append(box)
        region_cache: dict[tuple[int, int], bytes | None] = {}
        for (cx, cz), chunk_boxes in sorted(by_chunk.items()):
            key = (cx >> 5, cz >> 5)
            if key not in region_cache:
                path = self.region_dir / f"r.{key[0]}.{key[1]}.mca"
                region_cache[key] = path.read_bytes() if path.exists() else None
            data = region_cache[key]
            if data is None:
                continue
            raw = _chunk_bytes(data, cx, cz)
            if raw is None:
                continue
            chunk = read_nbt(_decompress(raw))
            for section in chunk.get("sections", []):
                sy = section["Y"]
                if sy * 16 + 15 < y0 or sy * 16 > y1:
                    continue
                _decode_section(section, cx, cz, chunk_boxes, blocks)
            for tick in chunk.get("block_ticks", []):
                pos = (tick["x"], tick["y"], tick["z"])
                if any(_in_box(pos, box) for box in chunk_boxes):
                    pending.append(PendingTick(pos, tick["i"], int(tick["t"]), int(tick["p"])))
        pending.sort(key=lambda t: (t.pos, t.block))
        return blocks, pending


def _chunk_bytes(data: bytes, cx: int, cz: int) -> bytes | None:
    """The stored (compressed) bytes of one chunk, including its compression byte."""
    index = (cx & 31) + (cz & 31) * 32
    header = data[4 * index: 4 * index + 4]
    offset = int.from_bytes(header[:3], "big")
    if offset == 0:
        return None
    start = offset * _SECTOR
    length = int.from_bytes(data[start: start + 4], "big")
    return data[start + 4: start + 4 + length]


def _decompress(raw: bytes) -> bytes:
    compression, payload = raw[0], raw[1:]
    if compression == 1:
        return gzip.decompress(payload)
    if compression == 2:
        return zlib.decompress(payload)
    if compression == 3:
        return payload
    raise ValueError(f"unsupported chunk compression {compression}")


__all__ = ["AIR", "Box", "PendingTick", "SnapshotReader", "read_region_blocks", "read_world_blocks"]
