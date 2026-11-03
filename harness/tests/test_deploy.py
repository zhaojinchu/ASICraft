"""The deployment files (deploy/) and the server-only setup: the scripts parse, the
unit runs the backend on loopback behind the proxy with the shipped core, the
Caddyfile proxies to the unit's port, the runbook names every file, and
tools/setup.sh has a --server mode that leaves out the development tools."""

import re
import subprocess

from harness.server import ROOT

DEPLOY = ROOT / "deploy"


def unit_fields(text: str) -> dict[str, str]:
    return dict(line.split("=", 1) for line in text.splitlines() if "=" in line and not line.startswith(("#", "[")))


def test_every_shell_script_parses():
    for script in [*DEPLOY.glob("*.sh"), ROOT / "tools" / "setup.sh"]:
        run = subprocess.run(["bash", "-n", str(script)], capture_output=True, text=True)
        assert run.returncode == 0, (script.name, run.stderr)
        assert script.stat().st_mode & 0o111, f"{script.name} is not executable"


def test_the_unit_runs_the_backend_on_loopback_behind_the_proxy_with_the_shipped_core():
    unit = unit_fields((DEPLOY / "redfab-live.service").read_text())
    assert unit["Restart"] == "always" and unit["User"] == "redfab"
    exec_start = unit["ExecStart"]
    assert "server/live.mjs" in exec_start
    for option in ("--host 127.0.0.1", "--trust-proxy", "--wasm /opt/redfab/dist/redfab_wasm.wasm", "--port 8766"):
        assert option in exec_start, option
    assert unit["WorkingDirectory"] == "/opt/redfab"
    assert "MemoryMax" in unit, "the unit caps memory below the box's"


def test_the_caddyfile_proxies_to_the_units_port_and_takes_the_name_from_the_installer():
    caddy = (DEPLOY / "Caddyfile").read_text()
    unit = unit_fields((DEPLOY / "redfab-live.service").read_text())
    port = re.search(r"--port (\d+)", unit["ExecStart"]).group(1)
    assert f"reverse_proxy 127.0.0.1:{port}" in caddy
    assert caddy.count("reverse_proxy") == 2, "the backend and the video relay, nothing else"
    assert "redfab.example" in caddy, "the placeholder install.sh replaces"
    install = (DEPLOY / "install.sh").read_text()
    assert "s/redfab.example/$DOMAIN/" in install and "/etc/caddy/Caddyfile" in install


def test_the_installer_never_accepts_the_eula_or_builds_the_core_itself():
    install = (DEPLOY / "install.sh").read_text()
    assert "eula=true" not in install and "--accept-eula --server" in install, "the operator accepts the EULA"
    assert "cargo" not in install and "ship_core.sh" in install, "the core is shipped, not built there"
    assert "tools/setup.sh --server" in install and "tools/build_mod.sh" in install
    for unit in ("redfab-live.service", "redfab-restart.service", "redfab-restart.timer"):
        assert f"deploy/{unit}" in install, unit
    assert "status_log.cron" in install


def test_the_video_relay_runs_as_a_unit_on_loopback_behind_the_proxy_with_its_media_ports_open():
    """Stage 14: MediaMTX takes the camera's WHIP publish behind a password and serves WHEP reads
    to anyone, its HTTP under Caddy at /camera/, its ICE listeners on 8189; the backend asks its
    API on loopback whether the stream is up; the camera key reaches the backend from a file."""
    unit = unit_fields((DEPLOY / "redfab-relay.service").read_text())
    assert unit["Restart"] == "always" and unit["User"] == "redfab"
    assert unit["ExecStart"].endswith("/usr/local/bin/mediamtx /etc/redfab/mediamtx.yml")
    conf = (DEPLOY / "mediamtx.yml").read_text()
    for line in ("webrtcAddress: 127.0.0.1:8889", "webrtcLocalUDPAddress: :8189", "webrtcLocalTCPAddress: :8189", "apiAddress: 127.0.0.1:9997", "api: yes"):
        assert line in conf, line
    for off in ("rtsp: no", "rtmp: no", "hls: no", "srt: no", "moq: no"):
        assert off in conf, f"{off}: every protocol but WebRTC stays off"
    assert "redfab.example" in conf and "RELAY_PASSWORD" in conf, "the installer puts the name and the password in"
    assert re.search(r"user: camera\n\s+pass: RELAY_PASSWORD", conf), "the publisher is the camera user with the installer's password"
    assert "action: publish" in conf and "action: read" in conf
    assert re.search(r"^paths:\n\s+camera:", conf, re.M), "the one path is camera"
    caddy = (DEPLOY / "Caddyfile").read_text()
    assert "handle_path /camera/*" in caddy and "reverse_proxy 127.0.0.1:8889" in caddy
    install = (DEPLOY / "install.sh").read_text()
    assert "8189/udp" in install and "8189/tcp" in install
    assert "MEDIAMTX_VERSION" in install and "MEDIAMTX_SHA256" in install and "sha256sum -c" in install, "the relay is fetched by its pinned checksum"
    assert "deploy/redfab-relay.service" in install and "deploy/mediamtx.yml" in install
    assert "REDFAB_CAMERA_KEY" in install and "/etc/redfab/camera.env" in install and "RELAY_PASSWORD" in install
    live = (DEPLOY / "redfab-live.service").read_text()
    assert "EnvironmentFile=-/etc/redfab/camera.env" in live and "--relay-api http://127.0.0.1:9997" in live
    # The game's port is fixed on the box, so the PC's tunnel and its key's permitopen can name it.
    assert "--game-port 25565" in live
    camera_doc = (DEPLOY / "CAMERA.md").read_text()
    assert 'permitopen="127.0.0.1:25565"' in camera_doc and "-L 25565:127.0.0.1:25565" in camera_doc
    assert "whitelist add" not in camera_doc, "the backend whitelists the camera's player; the operator need not"
    versions = (ROOT / "versions.env").read_text()
    assert re.search(r"^MEDIAMTX_VERSION=v1\.\d+\.\d+$", versions, re.M) and re.search(r"^MEDIAMTX_SHA256=[0-9a-f]{64}$", versions, re.M)
    readme = (DEPLOY / "README.md").read_text()
    assert "8189" in readme and "camera" in readme.lower(), "the runbook says what to open for the camera and where its setup lives"
    assert (DEPLOY / "CAMERA.md").exists(), "the PC's runbook"
    smoke = (ROOT / "tools" / "smoke_remote.mjs").read_text()
    assert "/camera/camera/whep" in smoke and "502" in smoke, "the smoke reaches the relay through the proxy"


def test_the_restart_timer_runs_the_idle_check_weekly():
    timer = unit_fields((DEPLOY / "redfab-restart.timer").read_text())
    assert timer["OnCalendar"].startswith("Sun ")
    service = unit_fields((DEPLOY / "redfab-restart.service").read_text())
    assert service["ExecStart"].endswith("deploy/restart_when_idle.sh")
    script = (DEPLOY / "restart_when_idle.sh").read_text()
    assert "systemctl restart redfab-live" in script and "/api/status" in script


def test_the_runbook_names_every_deploy_file_and_the_smoke():
    readme = (DEPLOY / "README.md").read_text()
    for path in DEPLOY.iterdir():
        if path.name != "README.md":
            assert path.name in readme, f"deploy/README.md does not mention {path.name}"
    assert "tools/smoke_remote.mjs" in readme and "--accept-eula --server" in readme
    assert "NOT AN OFFICIAL MINECRAFT PRODUCT" in readme


def test_the_setup_script_has_a_server_mode_without_the_development_tools():
    setup = (ROOT / "tools" / "setup.sh").read_text()
    assert "--server" in setup.split("set -euo pipefail")[0], "the usage header lists --server"
    assert "--server) SERVER=1" in setup
    # Rust, Verilator and Python are development tools: each section opens under the guard.
    for first_line in ('export PATH="$HOME/.cargo/bin:$PATH"', "if ! command -v verilator >/dev/null; then", "command -v uv >/dev/null"):
        assert f"if [ $SERVER = 0 ]; then\n{first_line}" in setup, f"{first_line!r} is not guarded by --server"
    assert setup.count("if [ $SERVER = 0 ]; then") == 3
    # The JDK, the server jar, Fabric and the Node packages are not.
    assert "npm ci" in setup
