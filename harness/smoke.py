"""Stage 0 check: the vanilla server starts, answers RCON, and stops cleanly.

Run with: uv run python -m harness.smoke
"""

import re
import subprocess
import sys

from .server import VanillaServer, _port_open


def main() -> int:
    server = VanillaServer("smoke")
    with server:
        print(f"server {server.mc_version} ready in {server.startup_seconds:.1f}s")

        listing = server.command("list")
        print(f"list -> {listing!r}")
        if not re.search(r"There are \d+ of a max", listing):
            print("FAIL: unexpected reply to 'list'")
            return 1

        # A reply longer than one RCON packet (4096 bytes) must come back whole.
        long_reply = server.command("help")
        print(f"help -> {len(long_reply)} chars")
        if "/tick" not in long_reply or "/setblock" not in long_reply:
            print("FAIL: 'help' reply is missing expected commands")
            return 1

        # Control: an unknown command must be reported as an error, not silently accepted.
        bogus = server.command("redfab_no_such_command")
        print(f"bogus -> {bogus!r}")
        if "Unknown or incomplete command" not in bogus:
            print("FAIL: unknown command was not rejected")
            return 1

    if server.is_running():
        print("FAIL: server process still running after stop")
        return 1
    if _port_open(server.rcon_port) or _port_open(server.game_port):
        print("FAIL: server ports still open after stop")
        return 1
    stray = subprocess.run(
        ["pgrep", "-f", f"redfab.run={server.run_dir}"], capture_output=True, text=True
    ).stdout.split()
    if stray:
        print(f"FAIL: stray server processes: {stray}")
        return 1

    print("PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
