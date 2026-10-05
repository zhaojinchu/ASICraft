"""Stage 2 check: structure files written by redfab load correctly in the real game.

For several recorded (settled) states, the structure is placed with
`/place template` twice: strictly, which must reproduce the state exactly, and
normally (with block updates), which must settle to the same state. A third
structure larger than 48 blocks checks that size is not limited.

Run with: uv run python -m harness.place_template
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

from .blockstate import BlockState, format_pos
from .server import ROOT, VanillaServer
from .session import Session

TRACES = [
    "fixtures/mechanics/torch_delay.vanilla.trace.json",
    "fixtures/mechanics/weak_torch.vanilla.trace.json",
    "fixtures/mechanics/diag_up_open.vanilla.trace.json",
    "fixtures/mechanics/torch_strong_up.vanilla.trace.json",
    "fixtures/mechanics/dust_decay.vanilla.trace.json",
]
BIG = (70, 2, 70)
Y = 64


def redfab(*args: str) -> None:
    subprocess.run(["cargo", "run", "-q", "-p", "redfab-cli", "--", *args], cwd=ROOT, check=True)


def read_box(session: Session, origin: tuple[int, int, int], size: tuple[int, int, int]) -> dict[str, str]:
    box = (origin, (origin[0] + size[0] - 1, origin[1] + size[1] - 1, origin[2] + size[2] - 1))
    blocks, _ = session.snapshot([box])
    return {format_pos((x - origin[0], y - origin[1], z - origin[2])): str(state) for (x, y, z), state in blocks.items()}


def place(session: Session, name: str, origin: tuple[int, int, int], strict: bool) -> None:
    x, y, z = origin
    reply = session.command(f"place template redfab:{name} {x} {y} {z}" + (" none none 1.0 0 strict" if strict else ""))
    if not reply.startswith("Loaded template"):
        raise RuntimeError(f"place template {name}: {reply!r}")


def main() -> int:
    failures: list[str] = []
    with VanillaServer("place-template") as server:
        session = Session(server)
        session.prepare()
        structures = server.run_dir / "world" / "generated" / "redfab" / "structure"
        structures.mkdir(parents=True)
        session.forceload(((-16, 0, -16), (160, 0, 160)))

        for i, rel in enumerate(TRACES):
            name = f"t{i}"
            trace = json.loads((ROOT / rel).read_text())
            size = tuple(trace["size"])
            redfab("structure", rel, str(structures / f"{name}.nbt"))

            strict_origin = (i * 24, Y, 0)
            place(session, name, strict_origin, strict=True)
            got = read_box(session, strict_origin, size)
            if got != trace["initial"]:
                failures.append(f"{rel}: strict placement differs: {_diff(trace['initial'], got)}")
            session.step(20)  # a consistent state must not move when time runs
            if read_box(session, strict_origin, size) != trace["initial"]:
                failures.append(f"{rel}: strictly placed state changed after 20 ticks")

            normal_origin = (i * 24, Y, 32)
            place(session, name, normal_origin, strict=False)
            session.step(20)
            got = read_box(session, normal_origin, size)
            if got != trace["initial"]:
                failures.append(f"{rel}: normal placement settles differently: {_diff(trace['initial'], got)}")
            print(f"ok {rel}" if not any(rel in f for f in failures) else f"FAILED {rel}")

        redfab("structure-pattern", *map(str, BIG), str(structures / "big.nbt"))
        origin = (0, Y, 80)
        place(session, "big", origin, strict=True)
        got = read_box(session, origin, BIG)
        want = {
            format_pos((x, y, z)): str(BlockState.parse("stone" if (x + y + z) % 2 == 0 else "glass"))
            for x in range(BIG[0]) for y in range(BIG[1]) for z in range(BIG[2])
        }
        if got != want:
            failures.append(f"{BIG} pattern: {len(got)} blocks read, {len(want)} expected; {_diff(want, got)}")
        else:
            print(f"ok {BIG[0]}x{BIG[1]}x{BIG[2]} structure ({len(got)} blocks)")

    for failure in failures:
        print("FAIL: " + failure)
    return 1 if failures else 0


def _diff(want: dict[str, str], got: dict[str, str]) -> str:
    diffs = [f"{p}: want {want.get(p, 'air')}, got {got.get(p, 'air')}" for p in sorted(set(want) | set(got)) if want.get(p) != got.get(p)]
    return "; ".join(diffs[:4]) + (f" (+{len(diffs) - 4} more)" if len(diffs) > 4 else "")


if __name__ == "__main__":
    sys.exit(main())
