"""The tile library in the game: every tile variant at three placements in both
redstone modes must meet the expectations in its fixture, and the recorded traces
must equal the simulator's. Random tile groups are recorded and replayed too.

    python -m harness.tiles verify [--groups N] [--seed S] [--serial]

The seed of the random tile groups is fixed (1) so the gate tests the same groups
run after run; `--seed random` draws one from the clock for an exploratory run.
The two redstone modes run at the same time on two servers (each server gets its
own ports and run directory); `--serial` runs them one after the other.
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
from concurrent.futures import ThreadPoolExecutor

from .mechanics import MODES, source_for
from .runner import Runner, layout
from .server import ROOT, VanillaServer, redfab, seed_argument
from .session import Session
from .trace import Fixture, Trace, check_expectations, first_trace_difference, parse_replay_summary

OUT = ROOT / "build" / "tiles"
DEFAULT_SEED = 1


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("action", choices=["verify"])
    ap.add_argument("--groups", type=int, default=24)
    ap.add_argument("--seed", type=seed_argument, default=DEFAULT_SEED, help="an integer, or `random`")
    ap.add_argument("--serial", action="store_true", help="run the two redstone modes one after the other")
    return ap.parse_args(argv)


def run_mode(mode: str, fixtures: list[Fixture]) -> dict[tuple[str, str], Trace]:
    """Every fixture at three placements on one server of the given mode."""
    with VanillaServer(f"tiles-{mode}", redstone_experiments=MODES[mode]) as server:
        session = Session(server)
        session.prepare()
        return Runner(session, source_for(server)).run(layout(fixtures), log=lambda m: print(f"[{mode}] {m}", flush=True))


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)

    shutil.rmtree(OUT, ignore_errors=True)
    generated = redfab("tile-fixtures", str(OUT), "--groups", str(args.groups), "--seed", str(args.seed), check=False)
    if generated.returncode != 0:
        print(generated.stderr.strip())
        return 1
    fixtures = [Fixture.from_json(json.loads(p.read_text())) for p in sorted(OUT.glob("*.fixture.json"))]
    print(f"{len(fixtures)} fixtures (seed {args.seed})")

    failures: list[str] = []
    if args.serial:
        runs = {mode: run_mode(mode, fixtures) for mode in MODES}
    else:
        with ThreadPoolExecutor(max_workers=len(MODES)) as pool:
            runs = {mode: future.result() for mode, future in [(m, pool.submit(run_mode, m, fixtures)) for m in MODES]}
    for mode, traces in runs.items():
        for fx in fixtures:
            base = traces[(fx.name, "aligned")]
            for variant in ("straddle", "rotated"):
                if (diff := first_trace_difference(base, traces[(fx.name, variant)])) is not None:
                    failures.append(f"[{mode}] {fx.name}: '{variant}' placement differs: {diff}")
            failures += [f"[{mode}] {fx.name}: {f}" for f in check_expectations(base, fx.expect)]
            (OUT / f"{fx.name}.{mode}.trace.json").write_text(json.dumps(base.to_json()) + "\n")

    # Every trace must replay in the simulator's default order of same-tick events:
    # a tile that matches only in another order has a race, which the tile contract forbids.
    replay = redfab("check-traces", "--no-skips", str(OUT), check=False)
    print(replay.stdout.strip())
    summary = parse_replay_summary(replay.stdout)
    if replay.returncode != 0:
        failures.append("the simulator does not reproduce the game's traces of the tiles")
    if summary.racy != 0:
        failures.append(f"{summary.racy} tile trace(s) match the simulator only with another order of same-tick events")

    for failure in failures:
        print("FAIL: " + failure)
    if not failures:
        print(f"all {len(fixtures)} tile fixtures hold in the game at three placements in both modes, and match the simulator")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
