"""Starts a local server with a built design placed in it, so you can look at it.

    uv run python -m harness.show nor2

Join `localhost` from Minecraft Java 26.3 (multiplayer, direct connection). You
are in creative mode on a flat world; the design is a few blocks from spawn.
Flip the levers and watch the lamp. Stop the server with Ctrl-C.

    uv run python -m harness.show tiles

places every tile of the library in a row instead, each with a bit of dust at
its ports, so you can walk along them and drive them with redstone blocks.

    uv run python -m harness.show nor2 --self-test

places the design, drives its inputs through the server console with time
running normally (no freezing, no stepping), checks the lamp, and exits.
"""

from __future__ import annotations

import argparse
import json
import shutil
import signal
import sys
import time

from .design import DESIGNS, all_vectors, load, verilator_outputs
from .server import VanillaServer, cargo

GROUND_Y = -60   # the surface of the default flat world
ORIGIN = (4, GROUND_Y, 4)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("name")
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--port", type=int, default=25565)
    args = ap.parse_args(argv)
    if args.name == "tiles":
        return show_tiles(args.port)
    design, module = load(args.name)

    properties = {
        "gamemode": "creative",
        "force-gamemode": "true",
        # 26.3 turns the whitelist on by default, and ours is empty: nobody could join.
        # The server only listens on 127.0.0.1, so it is safe to turn off.
        "white-list": "false",
        "enforce-whitelist": "false",
    }
    server = VanillaServer("show", properties, empty_world=False)
    # Stop cleanly on Ctrl-C or `kill`, however this script was started.
    signal.signal(signal.SIGINT, signal.default_int_handler)
    signal.signal(signal.SIGTERM, signal.default_int_handler)
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
        print("Ctrl-C to stop.", flush=True)
        try:
            while server.is_running():
                time.sleep(1)
        except KeyboardInterrupt:
            print("stopping the server...")
    return 0


def show_tiles(port: int) -> int:
    """Every tile variant from its test fixture, 20 blocks apart along the x axis."""
    import subprocess
    from pathlib import Path

    out = Path(DESIGNS.parent) / "tiles"
    shutil.rmtree(out, ignore_errors=True)
    subprocess.run([cargo(), "run", "-q", "-p", "redfab-cli", "--", "tile-fixtures", str(out), "--groups", "0"],
                   cwd=DESIGNS.parent.parent, check=True, capture_output=True)
    fixtures = [json.loads(p.read_text()) for p in sorted(out.glob("*.fixture.json"))]
    properties = {"gamemode": "creative", "force-gamemode": "true", "white-list": "false", "enforce-whitelist": "false"}
    server = VanillaServer("show", properties, empty_world=False)
    server.game_port = port
    signal.signal(signal.SIGINT, signal.default_int_handler)
    signal.signal(signal.SIGTERM, signal.default_int_handler)
    with server:
        for rule in ("advance_time false", "advance_weather false", "spawn_mobs false"):
            server.command(f"gamerule {rule}")
        pitch = 20
        x0, y, z0 = 4, GROUND_Y, 4
        server.command(f"forceload add {x0 - 16} {z0 - 16} {x0 + pitch * len(fixtures) + 16} {z0 + 40}")
        server.command(f"setworldspawn {x0 - 3} {y} {z0 + 8}")
        for i, fx in enumerate(fixtures):
            origin_x = x0 + i * pitch
            for pos, state in sorted(fx["blocks"].items(), key=lambda kv: tuple(int(v) for v in kv[0].split(","))[::-1]):
                px, py, pz = (int(v) for v in pos.split(","))
                server.command(f"setblock {origin_x + px} {y + py} {z0 + pz} {state}")
            print(f"  x={origin_x + 6:4d}  {fx['name']}")
        print(json.dumps({"tiles": len(fixtures), "address": f"localhost:{server.game_port}"}))
        print(f"Minecraft {server.mc_version} server running. Join localhost:{server.game_port}.")
        print("Tiles run east from spawn, 20 blocks apart (x above is each tile's west edge). Ctrl-C to stop.", flush=True)
        try:
            while server.is_running():
                time.sleep(1)
        except KeyboardInterrupt:
            print("stopping the server...")
    return 0


if __name__ == "__main__":
    sys.exit(main())
