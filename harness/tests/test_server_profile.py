"""The server configuration the harness writes is the shared profile
(harness/data/server_profile.json), which the live backend reads too, plus the
harness's own differences and nothing else."""

import json
import re

from harness import session
from harness.server import MOTD, PROFILE_FILE, VanillaServer, read_profile, server_properties

ALWAYS_PER_RUN = {"server-port", "rcon.port", "rcon.password"}
HARNESS_OWN = {"motd"}


def test_the_profile_is_well_formed():
    profile = json.loads(PROFILE_FILE.read_text())
    assert set(profile) == {"doc", "gamerules", "max_block_modifications", "properties", "jvm_args"}
    assert len(profile["gamerules"]) == 14
    assert all(re.fullmatch(r"[a-z_]+", rule) and isinstance(value, str) for rule, value in profile["gamerules"].items())
    assert all(isinstance(value, str) for value in profile["properties"].values())
    assert profile["properties"]["server-ip"] == "127.0.0.1", "the server only ever listens on localhost"
    assert "generator-settings" in profile["properties"] and "the_void" in profile["properties"]["generator-settings"]
    assert profile["max_block_modifications"] > 32768, "above the game's default, so the live backend's plot clears take fewer fills"
    assert profile["jvm_args"] and all(arg.startswith("-X") for arg in profile["jvm_args"])


def test_the_session_gamerules_are_the_profile_gamerules():
    assert session.GAMERULES == read_profile()["gamerules"]
    assert session.GAMERULES["random_tick_speed"] == "0"


def test_the_properties_written_are_the_profile_plus_the_harness_differences():
    profile = read_profile()["properties"]
    written = server_properties(25565, 25575, "hunter2")
    for key, value in profile.items():
        assert written[key] == value, key
    assert written["server-port"] == "25565" and written["rcon.port"] == "25575" and written["rcon.password"] == "hunter2"
    assert written["motd"] == MOTD
    assert set(written) == set(profile) | ALWAYS_PER_RUN | HARNESS_OWN, "nothing else is written"
    assert "online-mode" not in written, "online-mode is the live backend's difference, not the harness's"

    ground = server_properties(1, 2, "p", empty_world=False)
    assert set(profile) - set(ground) == {"generator-settings"}, "a world with ground drops the void preset only"
    experiments = server_properties(1, 2, "p", redstone_experiments=True)
    assert experiments["initial-enabled-packs"] == "vanilla,redstone_experiments"
    overridden = server_properties(1, 2, "p", extra={"view-distance": "10", "motd": "mine"})
    assert overridden["view-distance"] == "10" and overridden["motd"] == "mine"


def test_the_launch_command_starts_with_the_profile_jvm_arguments():
    jvm_args = read_profile()["jvm_args"]
    server = VanillaServer("profile-test")
    argv = server.launch_command()
    assert argv[0] == str(server.java)
    assert argv[1:1 + len(jvm_args)] == jvm_args
    assert "-jar" in argv and argv[-1] == "--nogui"
    assert any(arg.startswith("-Dredfab.run=") for arg in argv), "the stray-process marker is there"
    modded = VanillaServer("profile-test", modded=True).launch_command()
    assert modded[1:1 + len(jvm_args)] == jvm_args
    assert any(arg.startswith("-Dredfab.observer.port=") for arg in modded)
