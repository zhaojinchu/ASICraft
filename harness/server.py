"""Starts and stops a vanilla Minecraft server for tests.

Each server gets a fresh run directory under build/mc/, so no world state leaks
between runs. The server only listens on 127.0.0.1.
"""

import atexit
import json
import re
import secrets
import shutil
import socket
import subprocess
import time
from pathlib import Path

from .rcon import Rcon, RconError

ROOT = Path(__file__).resolve().parent.parent
# `tools/setup.sh` links this to the JDK's java binary, whatever the JDK's layout on this platform.
JAVA = ROOT / ".tools" / "java"
# Written by `tools/setup.sh --accept-eula`. No server starts without it.
EULA_FILE = ROOT / ".tools" / "eula-accepted"
EULA_URL = "https://aka.ms/MinecraftEULA"
# The server configuration shared with the live backend (server/minecraft.mjs):
# the gamerules, the server.properties defaults and the JVM arguments.
PROFILE_FILE = ROOT / "harness" / "data" / "server_profile.json"
MOTD = "redfab test server"


class EulaNotAccepted(RuntimeError):
    pass


def require_eula(path: Path = EULA_FILE) -> None:
    """Running the Minecraft server binds whoever runs it to Mojang's EULA, so the
    harness only starts one after that person has accepted it, which
    `tools/setup.sh --accept-eula` records in `path`."""
    if not path.is_file():
        raise EulaNotAccepted(
            f"the Minecraft server's EULA ({EULA_URL}) has not been accepted on this machine; "
            f"read it, then run `tools/setup.sh --accept-eula`, which records the acceptance in {path}"
        )


def seed_argument(text: str) -> int:
    """An argparse type for `--seed`: an integer, or `random` for one from the clock
    (printed by the caller so a failure can be reproduced)."""
    if text == "random":
        return int(time.time())
    return int(text)


def stray_server_pattern(run_dir: Path) -> str:
    """The `pkill -f` pattern (an extended regex) for a server started in `run_dir`.
    Anchored after the directory, so `mechanics-vanilla` does not also match
    `mechanics-vanilla-rx`."""
    escaped = re.sub(r"[.^$*+?()\[\]{}|\\]", lambda m: "\\" + m.group(0), str(run_dir))
    return rf"-Dredfab\.run={escaped}( |$)"


def kill_stray_server(run_dir: Path) -> subprocess.CompletedProcess:
    """Kills any server still running in `run_dir`. The pattern starts with a dash,
    which BSD pkill would read as options; `--` ends them (macOS printed its usage
    and killed nothing before)."""
    return subprocess.run(["pkill", "-9", "-f", "--", stray_server_pattern(run_dir)], check=False,
                          capture_output=True, text=True)


def cargo() -> str:
    """The cargo executable: on PATH, or rustup's default location."""
    found = shutil.which("cargo")
    if found:
        return found
    home = Path.home() / ".cargo" / "bin" / "cargo"
    return str(home) if home.exists() else "cargo"


def redfab(*args: str, check: bool = True) -> subprocess.CompletedProcess:
    """Runs the redfab CLI from the checkout (`cargo run -p redfab-cli`) with its output
    captured; with `check`, a failure raises CalledProcessError."""
    return subprocess.run([cargo(), "run", "-q", "-p", "redfab-cli", "--", *args], cwd=ROOT, check=check,
                          capture_output=True, text=True)


def read_profile() -> dict:
    """The shared server profile (harness/data/server_profile.json)."""
    return json.loads(PROFILE_FILE.read_text())


def server_properties(game_port: int, rcon_port: int, password: str, *,
                      empty_world: bool = True, redstone_experiments: bool = False,
                      extra: dict[str, str] | None = None) -> dict[str, str]:
    """What the harness writes to server.properties: the profile's defaults, this
    run's ports and password, and the harness's own differences on top: its motd,
    no void preset when the world is to have ground, Mojang's experimental
    redstone pack when asked for, then the caller's `extra`."""
    properties = dict(read_profile()["properties"])
    properties["server-port"] = str(game_port)
    properties["rcon.port"] = str(rcon_port)
    properties["rcon.password"] = password
    properties["motd"] = MOTD
    if not empty_world:
        del properties["generator-settings"]
    if redstone_experiments:
        properties["initial-enabled-packs"] = "vanilla,redstone_experiments"
    properties.update(extra or {})
    return properties


def read_versions() -> dict[str, str]:
    """Parse versions.env (KEY=value lines)."""
    pins = {}
    for line in (ROOT / "versions.env").read_text().splitlines():
        if "=" in line and not line.lstrip().startswith("#"):
            key, value = line.split("=", 1)
            pins[key.strip()] = value.strip()
    return pins


def _free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _port_open(port: int) -> bool:
    with socket.socket() as sock:
        sock.settimeout(0.5)
        return sock.connect_ex(("127.0.0.1", port)) == 0


class VanillaServer:
    """A vanilla server process plus an authenticated RCON connection.

    Use as a context manager: the server is stopped, and killed if it does not
    stop, when the block exits.
    """

    def __init__(self, name: str, properties: dict[str, str] | None = None, *,
                 empty_world: bool = True, redstone_experiments: bool = False, modded: bool = False):
        """`empty_world` makes a void superflat world (nothing but the blocks we place).
        `redstone_experiments` enables Mojang's experimental redstone data pack,
        which must be chosen when the world is created. `modded` runs the server
        through Fabric Loader with the observer mod (build/mod/redfab-observer.jar),
        which streams block changes on `observer_port`."""
        pins = read_versions()
        self.redstone_experiments = redstone_experiments
        self.empty_world = empty_world
        self.modded = modded
        self.observer_port = _free_port() if modded else 0
        self.mc_version = pins["MC_VERSION"]
        self.run_dir = ROOT / "build" / "mc" / name
        self.java = JAVA
        self.jar = ROOT / ".tools" / "mc" / f"server-{self.mc_version}.jar"
        self.game_port = _free_port()
        self.rcon_port = _free_port()
        self._password = secrets.token_hex(16)
        self._extra_properties = properties or {}
        self._proc: subprocess.Popen | None = None
        self.rcon: Rcon | None = None
        self.startup_seconds = 0.0

    def start(self, timeout: float = 240.0) -> None:
        require_eula()
        if not self.java.exists():
            raise RuntimeError(f"{self.java} is missing; run tools/setup.sh")
        # If an earlier harness process was killed hard, its server may still be
        # running in this run directory. Stop it before reusing the directory.
        kill_stray_server(self.run_dir)
        if self.run_dir.exists():
            shutil.rmtree(self.run_dir)
        self.run_dir.mkdir(parents=True)
        # Accepted by whoever ran `tools/setup.sh --accept-eula` (see require_eula).
        (self.run_dir / "eula.txt").write_text("eula=true\n")
        properties = server_properties(
            self.game_port, self.rcon_port, self._password,
            empty_world=self.empty_world, redstone_experiments=self.redstone_experiments,
            extra=self._extra_properties,
        )
        (self.run_dir / "server.properties").write_text(
            "".join(f"{key}={value}\n" for key, value in properties.items())
        )

        if self.modded:
            # Fabric's launcher expects its libraries beside it; the installed set is
            # copied from .tools/fabric, and the mod goes into mods/.
            fabric = ROOT / ".tools" / "fabric"
            for item in ("libraries", ".fabric", "versions"):
                shutil.copytree(fabric / item, self.run_dir / item, symlinks=True)
            shutil.copy(fabric / "fabric-server-launch.jar", self.run_dir)
            (self.run_dir / "fabric-server-launcher.properties").write_text(f"serverJar={self.jar}\n")
            (self.run_dir / "mods").mkdir()
            mod = ROOT / "build" / "mod" / "redfab-observer.jar"
            if not mod.exists():
                raise RuntimeError(f"{mod} is missing; run tools/build_mod.sh")
            shutil.copy(mod, self.run_dir / "mods")

        started = time.monotonic()
        log = open(self.run_dir / "server.log", "wb")
        self._proc = subprocess.Popen(
            self.launch_command(),
            cwd=self.run_dir,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        atexit.register(self._kill)

        deadline = started + timeout
        while time.monotonic() < deadline:
            if self._proc.poll() is not None:
                raise RuntimeError(f"server exited during startup; see {self.run_dir / 'server.log'}")
            if _port_open(self.rcon_port):
                self.rcon = Rcon("127.0.0.1", self.rcon_port, self._password)
                self.startup_seconds = time.monotonic() - started
                return
            time.sleep(0.25)
        self._kill()
        raise TimeoutError(f"server did not open RCON within {timeout}s; see {self.run_dir / 'server.log'}")

    def launch_command(self) -> list[str]:
        """The server's command line: the profile's JVM arguments, then this run's markers."""
        if self.modded:
            launch = [f"-Dredfab.observer.port={self.observer_port}", "-jar", str(self.run_dir / "fabric-server-launch.jar")]
        else:
            launch = ["-jar", str(self.jar)]
        return [
            str(self.java),
            *read_profile()["jvm_args"],
            # Unpack the bundled libraries once, not into every run directory.
            f"-DbundlerRepoDir={ROOT / '.tools' / 'mc' / 'bundle'}",
            # Marker so stray processes can be found by run directory.
            f"-Dredfab.run={self.run_dir}",
            *launch,
            "--nogui",
        ]

    def command(self, text: str) -> str:
        assert self.rcon is not None, "server not started"
        return self.rcon.command(text)

    def stop(self, timeout: float = 60.0) -> None:
        if self._proc is None:
            return
        if self._proc.poll() is None and self.rcon is not None:
            try:
                self.rcon.command("stop")
            except (OSError, RconError):
                # Either the server closed the connection before replying (normal
                # during shutdown) or the connection was already dead. SIGTERM
                # runs the server's own shutdown hook, so it is safe in both cases.
                if self._proc.poll() is None:
                    self._proc.terminate()
        if self.rcon is not None:
            self.rcon.close()
            self.rcon = None
        try:
            self._proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self._kill()

    def is_running(self) -> bool:
        return self._proc is not None and self._proc.poll() is None

    def _kill(self) -> None:
        if self._proc is not None and self._proc.poll() is None:
            self._proc.kill()
            self._proc.wait(timeout=30)

    def __enter__(self) -> "VanillaServer":
        self.start()
        return self

    def __exit__(self, *exc) -> None:
        self.stop()
