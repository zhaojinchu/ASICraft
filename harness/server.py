"""Starts and stops a vanilla Minecraft server for tests.

Each server gets a fresh run directory under build/mc/, so no world state leaks
between runs. The server only listens on 127.0.0.1.
"""

import atexit
import json
import secrets
import shutil
import socket
import subprocess
import time
from pathlib import Path

from .rcon import Rcon, RconError

ROOT = Path(__file__).resolve().parent.parent


def cargo() -> str:
    """The cargo executable: on PATH, or rustup's default location."""
    found = shutil.which("cargo")
    if found:
        return found
    home = Path.home() / ".cargo" / "bin" / "cargo"
    return str(home) if home.exists() else "cargo"


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
        self.java = ROOT / ".tools" / "jdk" / "Contents" / "Home" / "bin" / "java"
        self.jar = ROOT / ".tools" / "mc" / f"server-{self.mc_version}.jar"
        self.game_port = _free_port()
        self.rcon_port = _free_port()
        self._password = secrets.token_hex(16)
        self._extra_properties = properties or {}
        self._proc: subprocess.Popen | None = None
        self.rcon: Rcon | None = None
        self.startup_seconds = 0.0

    def start(self, timeout: float = 240.0) -> None:
        # If an earlier harness process was killed hard, its server may still be
        # running in this run directory. Stop it before reusing the directory.
        subprocess.run(["pkill", "-9", "-f", f"redfab.run={self.run_dir}"], check=False)
        if self.run_dir.exists():
            shutil.rmtree(self.run_dir)
        self.run_dir.mkdir(parents=True)
        # The project owner accepted the Minecraft EULA for local testing on 2026-10-01.
        (self.run_dir / "eula.txt").write_text("eula=true\n")
        properties = {
            "server-ip": "127.0.0.1",
            "server-port": str(self.game_port),
            "enable-rcon": "true",
            "rcon.port": str(self.rcon_port),
            "rcon.password": self._password,
            "broadcast-rcon-to-ops": "false",
            "level-type": "minecraft\\:flat",
            "generate-structures": "false",
            # The server would pause ticking 60 s after the last player left; we never have players.
            "pause-when-empty-seconds": "-1",
            "spawn-protection": "0",
            "view-distance": "4",
            "simulation-distance": "4",
            "max-tick-time": "-1",
            "difficulty": "peaceful",
            "motd": "redfab test server",
        }
        if self.empty_world:
            preset = json.dumps({"layers": [], "biome": "minecraft:the_void"})
            properties["generator-settings"] = preset.replace(":", "\\:")
        if self.redstone_experiments:
            properties["initial-enabled-packs"] = "vanilla,redstone_experiments"
        properties.update(self._extra_properties)
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
            launch = [f"-Dredfab.observer.port={self.observer_port}", "-jar", str(self.run_dir / "fabric-server-launch.jar")]
        else:
            launch = ["-jar", str(self.jar)]

        started = time.monotonic()
        log = open(self.run_dir / "server.log", "wb")
        self._proc = subprocess.Popen(
            [
                str(self.java),
                "-Xms512M",
                "-Xmx2G",
                # Unpack the bundled libraries once, not into every run directory.
                f"-DbundlerRepoDir={ROOT / '.tools' / 'mc' / 'bundle'}",
                # Marker so stray processes can be found by run directory.
                f"-Dredfab.run={self.run_dir}",
                *launch,
                "--nogui",
            ],
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
