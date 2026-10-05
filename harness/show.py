"""Starts a local server with a built design placed in it, so you can look at it.

    uv run python -m harness.show nor2

Join `localhost` from Minecraft Java 26.3 (multiplayer, direct connection). You
are in creative mode on a flat world; the design is a few blocks from spawn.
Flip the levers and watch the lamp. Stop the server with Ctrl-C.

    uv run python -m harness.show nor2 --self-test

places the design, drives its inputs through the server console with time
running normally (no freezing, no stepping), checks the lamp, and exits.
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
import time

from .design import DESIGNS, all_vectors, load, verilator_outputs
from .server import VanillaServer

GROUND_Y = -60   # the surface of the default flat world
ORIGIN = (4, GROUND_Y, 4)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("name")
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--port", type=int, default=25565)
    args = ap.parse_args(argv)
    design, module = load(args.name)

    server = VanillaServer("show", {"gamemode": "creative", "force-gamemode": "true", "spawn-monsters": "false"},
                           empty_world=False)
    if not args.self_test:
        server.game_port = args.port
    with server:
        structures = server.run_dir / "world" / "generated" / "redfab" / "structure"
        structures.mkdir(parents=True, exist_ok=True)
        shutil.copy(DESIGNS / args.name / "world.nbt", structures / f"{args.name}.nbt")
        for rule in ("advance_time false", "advance_weather false", "spawn_mobs false"):
            server.command(f"gamerule {rule}")
        x, y, z = ORIGIN
        sx, _, sz = design["size"]
        server.command(f"forceload add {x - 16} {z - 16} {x + sx + 16} {z + sz + 16}")
        server.command(f"setworldspawn {x - 3} {y} {z + sz // 2}")
        reply = server.command(f"place template redfab:{args.name} {x} {y} {z} none none 1.0 0 strict")
        if not reply.startswith("Loaded template"):
            print(f"could not place the design: {reply}")
            return 1

        def world(pos: str) -> str:
            px, py, pz = (int(v) for v in pos.split(","))
            return f"{x + px} {y + py} {z + pz}"

        if args.self_test:
            vectors = all_vectors(module)
            expected = verilator_outputs(args.name, module, vectors)
            seconds = design["settle_ticks"] / 20 + 0.5
            ok = True
            for vector, outputs in zip(vectors, expected):
                for pin in design["inputs"]:
                    server.command(f"setblock {world(pin['probe'])} {pin['on'] if vector[pin['name']] else pin['off']}")
                time.sleep(seconds)  # real time: the game is running at its normal 20 ticks per second
                for pin in design["outputs"]:
                    lit = server.command(f"execute if block {world(pin['lamp'])} minecraft:redstone_lamp[lit=true]") == "Test passed"
                    good = lit == bool(outputs[pin["name"]])
                    ok &= good
                    print(f"inputs {vector} -> {pin['name']}={int(lit)} {'ok' if good else 'WRONG, expected ' + str(outputs[pin['name']])}")
            print("PASS" if ok else "FAIL")
            return 0 if ok else 1

        inputs = ", ".join(f"{pin['name']} at {world(pin['probe'])}" for pin in design["inputs"])
        outputs = ", ".join(f"{pin['name']} at {world(pin['lamp'])}" for pin in design["outputs"])
        print(json.dumps({"design": args.name, "address": f"localhost:{server.game_port}"}))
        print(f"Minecraft {server.mc_version} server running. Join localhost:{server.game_port}.")
        print(f"Levers (on top of the stone blocks): {inputs}")
        print(f"Lamps: {outputs}")
        print("Ctrl-C to stop.")
        try:
            while server.is_running():
                time.sleep(1)
        except KeyboardInterrupt:
            pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
