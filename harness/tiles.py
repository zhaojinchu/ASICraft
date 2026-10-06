"""The tile library in the game: every tile variant at three placements in both
redstone modes must meet the expectations in its fixture, and the recorded traces
must equal the simulator's. Random tile groups are recorded and replayed too.

    python -m harness.tiles verify [--groups N] [--seed S]
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import time

from .mechanics import MODES, source_for
from .runner import Runner, layout
from .server import ROOT, VanillaServer, cargo
from .session import Session
from .trace import Fixture, check_expectations, first_trace_difference

OUT = ROOT / "build" / "tiles"


def redfab(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run([cargo(), "run", "-q", "-p", "redfab-cli", "--", *args], cwd=ROOT, capture_output=True, text=True)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("action", choices=["verify"])
    ap.add_argument("--groups", type=int, default=24)
    ap.add_argument("--seed", type=int, default=int(time.time()))
    args = ap.parse_args(argv)

    shutil.rmtree(OUT, ignore_errors=True)
    generated = redfab("tile-fixtures", str(OUT), "--groups", str(args.groups), "--seed", str(args.seed))
    if generated.returncode != 0:
        print(generated.stderr.strip())
        return 1
    fixtures = [Fixture.from_json(json.loads(p.read_text())) for p in sorted(OUT.glob("*.fixture.json"))]
    print(f"{len(fixtures)} fixtures (seed {args.seed})")

    failures: list[str] = []
    for mode in MODES:
        with VanillaServer(f"tiles-{mode}", redstone_experiments=MODES[mode]) as server:
            session = Session(server)
            session.prepare()
            traces = Runner(session, source_for(server)).run(layout(fixtures), log=lambda m: print(f"[{mode}] {m}"))
        for fx in fixtures:
            base = traces[(fx.name, "aligned")]
            for variant in ("straddle", "rotated"):
                if (diff := first_trace_difference(base, traces[(fx.name, variant)])) is not None:
                    failures.append(f"[{mode}] {fx.name}: '{variant}' placement differs: {diff}")
            failures += [f"[{mode}] {fx.name}: {f}" for f in check_expectations(base, fx.expect)]
            (OUT / f"{fx.name}.{mode}.trace.json").write_text(json.dumps(base.to_json()) + "\n")

    replay = redfab("check-traces", "--no-skips", str(OUT))
    print(replay.stdout.strip())
    if replay.returncode != 0:
        failures.append("the simulator does not reproduce the game's traces of the tiles")

    for failure in failures:
        print("FAIL: " + failure)
    if not failures:
        print(f"all {len(fixtures)} tile fixtures hold in the game at three placements in both modes, and match the simulator")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
