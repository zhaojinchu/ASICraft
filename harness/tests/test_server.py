"""Server bookkeeping that needs no server: the EULA gate and the stray-process pattern."""

import re
from pathlib import Path

import pytest

from harness.server import EULA_URL, EulaNotAccepted, kill_stray_server, require_eula, seed_argument, stray_server_pattern


def test_server_refuses_to_start_without_the_recorded_eula_acceptance(tmp_path: Path):
    with pytest.raises(EulaNotAccepted) as refused:
        require_eula(tmp_path / "eula-accepted")
    assert EULA_URL in str(refused.value) and "--accept-eula" in str(refused.value)
    (tmp_path / "eula-accepted").write_text("accepted\n")
    require_eula(tmp_path / "eula-accepted")


def test_stray_server_pattern_matches_only_its_own_run_directory():
    pattern = stray_server_pattern(Path("/x/build/mc/mechanics-vanilla"))
    own = "java -DbundlerRepoDir=/x/.tools/mc/bundle -Dredfab.run=/x/build/mc/mechanics-vanilla -jar server.jar --nogui"
    modded = "java -Dredfab.run=/x/build/mc/mechanics-vanilla -Dredfab.observer.port=5 -jar launch.jar --nogui"
    other = "java -Dredfab.run=/x/build/mc/mechanics-vanilla-rx -jar server.jar --nogui"
    assert re.search(pattern, own) and re.search(pattern, modded)
    assert not re.search(pattern, other)
    # Only characters an extended regex treats specially are escaped (`\-` would be undefined in POSIX ERE).
    assert "\\-" not in pattern and "\\." in pattern


def test_seed_argument_takes_an_integer_or_random():
    assert seed_argument("7") == 7
    assert isinstance(seed_argument("random"), int)
    with pytest.raises(ValueError):
        seed_argument("yesterday")


def test_the_stray_server_kill_is_a_pattern_to_pkill_not_options(tmp_path: Path):
    # The pattern starts with "-D"; BSD pkill took it as options, printed its usage and
    # killed nothing. Nothing runs in a fresh directory, so pkill finds no process (1).
    result = kill_stray_server(tmp_path / "mechanics-vanilla")
    assert result.returncode == 1, result.stderr
    assert "usage" not in result.stderr and "illegal option" not in result.stderr
