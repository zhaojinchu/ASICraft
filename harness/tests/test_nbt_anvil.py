"""NBT and region-file reading, checked against a region saved by the 26.3 server.

tests/data/r.0.0.mca was saved after placing a known set of blocks in chunk (0,0)
with a pending torch tick; tests/data/r.0.0.expected.json lists them.
"""

import json
import struct
from pathlib import Path

from harness.anvil import read_region_blocks
from harness.blockstate import format_pos
from harness.nbt import read_nbt

DATA = Path(__file__).resolve().parent.parent.parent / "tests" / "data"


def test_region_blocks_and_pending_ticks_match_expected():
    expected = json.loads((DATA / "r.0.0.expected.json").read_text())
    blocks, pending = read_region_blocks(DATA / "r.0.0.mca", expected["box"])
    assert {format_pos(k): str(v) for k, v in blocks.items()} == expected["blocks"]
    assert [p.to_json() for p in pending] == expected["pending"]


def _tag(tag_type: int, name: str, payload: bytes) -> bytes:
    return struct.pack(">bH", tag_type, len(name)) + name.encode() + payload


def test_nbt_reads_every_tag_type():
    inner = _tag(5, "g", struct.pack(">f", 5.0)) + b"\x00"
    payload = (
        struct.pack(">bH", 10, 0)  # root compound, empty name
        + _tag(1, "b", struct.pack(">b", 7))
        + _tag(2, "s", struct.pack(">h", -1))
        + _tag(3, "i", struct.pack(">i", 258))
        + _tag(4, "l", struct.pack(">q", 3))
        + _tag(5, "f", struct.pack(">f", 2.0))
        + _tag(6, "d", struct.pack(">d", 2.0))
        + _tag(7, "ba", struct.pack(">i2b", 2, -2, 1))
        + _tag(8, "st", struct.pack(">H", 2) + b"hi")
        + _tag(9, "t", struct.pack(">bi2b", 1, 2, 5, 6))
        + _tag(10, "c", inner)
        + _tag(11, "ia", struct.pack(">ii", 1, -2))
        + _tag(12, "la", struct.pack(">iq", 1, -1))
        + b"\x00"
    )
    tag = read_nbt(payload)
    assert tag["b"] == 7 and tag["s"] == -1 and tag["i"] == 258 and tag["l"] == 3
    assert tag["f"] == 2.0 and tag["d"] == 2.0
    assert tag["ba"] == [-2, 1] and tag["st"] == "hi"
    assert tag["t"] == [5, 6]
    assert tag["c"] == {"g": 5.0}
    assert tag["ia"] == [-2] and tag["la"] == [-1]
