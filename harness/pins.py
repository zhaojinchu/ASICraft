"""Stage 0 check: every committed game recording was made with the pinned server jar.

    uv run python -m harness.pins

The correctness argument rests on the simulator matching recordings of the exact
game build in versions.env; after a jar bump, stale recordings would only be
noticed if the game's behaviour had changed. This fails until they are re-recorded.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

from .server import ROOT, read_versions


def stale_recordings(fixtures: Path, mc_version: str, jar_sha1: str) -> list[str]:
    """The trace files under `fixtures` whose source is not the pinned jar."""
    stale = []
    for path in sorted(fixtures.glob("**/*.trace.json")):
        source = json.loads(path.read_text()).get("source", {})
        if source.get("mc_version") != mc_version or source.get("jar_sha1") != jar_sha1:
            stale.append(f"{path.relative_to(fixtures)}: recorded with {source.get('mc_version')} {source.get('jar_sha1')}")
    return stale


def main() -> int:
    pins = read_versions()
    fixtures = ROOT / "fixtures"
    traces = sum(1 for _ in fixtures.glob("**/*.trace.json"))
    stale = stale_recordings(fixtures, pins["MC_VERSION"], pins["MC_SERVER_SHA1"])
    for line in stale:
        print("STALE " + line)
    if stale:
        print(f"{len(stale)} of {traces} recordings were not made with Minecraft {pins['MC_VERSION']} "
              f"jar {pins['MC_SERVER_SHA1'][:8]}; re-record them")
        return 1
    if traces == 0:
        print("no recordings found under fixtures/")
        return 1
    print(f"all {traces} recordings were made with Minecraft {pins['MC_VERSION']} jar {pins['MC_SERVER_SHA1'][:8]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
