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
    assert caddy.count("reverse_proxy") == 1, "one backend"
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
