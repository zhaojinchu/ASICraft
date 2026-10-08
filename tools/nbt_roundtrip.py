"""Structure files written by redfab-core, read back by the harness's own NBT reader.

The two implementations share no code, so agreement means the bytes are right.
"""

import gzip
import json
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from harness.blockstate import BlockState, format_pos  # noqa: E402
from harness.nbt import read_nbt  # noqa: E402
from harness.server import read_versions, redfab  # noqa: E402

TRACES = [
    "fixtures/mechanics/torch_delay.vanilla.trace.json",
    "fixtures/mechanics/weak_torch.vanilla.trace.json",
    "fixtures/mechanics/diag_up_open.vanilla.trace.json",
    "fixtures/mechanics/plus_cross.vanilla.trace.json",
]


def read_structure(path: Path) -> tuple[list[int], dict[str, str], int]:
    root = read_nbt(gzip.decompress(path.read_bytes()))
    palette = [str(BlockState.from_palette(entry)) for entry in root["palette"]]
    blocks = {format_pos(tuple(b["pos"])): palette[b["state"]] for b in root["blocks"]}
    assert root["entities"] == [], "entities should be an empty list"
    return list(root["size"]), blocks, root["DataVersion"]


def main() -> int:
    data_version = int(read_versions()["MC_DATA_VERSION"])
    with tempfile.TemporaryDirectory() as tmp:
        for rel in TRACES:
            trace = json.loads((ROOT / rel).read_text())
            out = Path(tmp) / "t.nbt"
            redfab("structure", rel, str(out))
            size, blocks, version = read_structure(out)
            assert version == data_version, f"{rel}: data version {version}"
            assert size == trace["size"], f"{rel}: size {size} vs {trace['size']}"
            assert blocks == trace["initial"], f"{rel}: blocks differ"
            print(f"ok {rel}: {len(blocks)} blocks")
        out = Path(tmp) / "p.nbt"
        redfab("structure-pattern", "5", "2", "70", str(out))
        size, blocks, _ = read_structure(out)
        assert size == [5, 2, 70] and len(blocks) == 5 * 2 * 70
        assert blocks["0,0,0"] == "minecraft:stone" and blocks["1,0,0"] == "minecraft:glass" and blocks["4,1,69"] == "minecraft:stone"
        print("ok pattern 5x2x70")
    return 0


if __name__ == "__main__":
    sys.exit(main())
