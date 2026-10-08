"""Writes harness/data/block_defaults.json from the server's own block registry.

Chunk files omit block-state properties that have their default value, so the
harness needs the defaults to reconstruct full states. The server jar can print
its registry: this script runs that data generator and keeps only what we need.

usage: uv run python tools/gen_block_defaults.py
"""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from harness.server import read_versions  # noqa: E402

OUT = ROOT / "harness" / "data" / "block_defaults.json"


def main() -> int:
    pins = read_versions()
    mc = pins["MC_VERSION"]
    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run(
            [str(ROOT / ".tools/java"), "-DbundlerMainClass=net.minecraft.data.Main",
             f"-DbundlerRepoDir={ROOT / '.tools/mc/bundle'}", "-jar", str(ROOT / f".tools/mc/server-{mc}.jar"), "--reports"],
            cwd=tmp, check=True, capture_output=True,
        )
        blocks = json.loads((Path(tmp) / "generated/reports/blocks.json").read_text())
    compact = {}
    propertyless = []
    for name, block in blocks.items():
        props = block.get("properties")
        if not props:
            propertyless.append(name)
            continue
        default = next(s["properties"] for s in block["states"] if s.get("default"))
        compact[name] = {"values": props, "default": default}
    OUT.write_text(json.dumps({"mc_version": mc, "blocks": compact, "propertyless": sorted(propertyless)},
                              indent=0, sort_keys=True) + "\n")
    print(f"{len(blocks)} blocks, {len(compact)} with properties -> {OUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
