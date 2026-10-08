"""The recordings-versus-pinned-jar check must notice a recording from another build."""

import json
from pathlib import Path

from harness.pins import stale_recordings


def _trace(path: Path, mc_version: str, jar_sha1: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"format": "redfab-trace/1", "source": {"kind": "vanilla", "mc_version": mc_version,
                                                                      "jar_sha1": jar_sha1}, "ticks": []}))


def test_stale_recordings_are_named_and_current_ones_pass(tmp_path: Path):
    _trace(tmp_path / "mechanics" / "a.vanilla.trace.json", "26.3", "abc")
    _trace(tmp_path / "fuzz" / "b.vanilla-rx.trace.json", "26.3", "abc")
    assert stale_recordings(tmp_path, "26.3", "abc") == []
    _trace(tmp_path / "fuzz" / "c.vanilla.trace.json", "26.2", "abc")
    _trace(tmp_path / "fuzz" / "d.vanilla.trace.json", "26.3", "old")
    stale = stale_recordings(tmp_path, "26.3", "abc")
    assert [s.split(":")[0] for s in stale] == ["fuzz/c.vanilla.trace.json", "fuzz/d.vanilla.trace.json"]


PLATFORMS = ("DARWIN_ARM64", "DARWIN_X86_64", "LINUX_X86_64", "LINUX_AARCH64")
ARCHIVES = {"DARWIN_ARM64": "aarch64_mac", "DARWIN_X86_64": "x64_mac", "LINUX_X86_64": "x64_linux", "LINUX_AARCH64": "aarch64_linux"}


def test_every_supported_platform_has_a_jdk_url_and_hash_for_the_pinned_version():
    from harness.server import read_versions

    pins = read_versions()
    version = pins["JDK_VERSION"]  # e.g. 25.0.4.1+1
    for platform in PLATFORMS:
        url, sha = pins[f"JDK_URL_{platform}"], pins[f"JDK_SHA256_{platform}"]
        assert url.startswith("https://github.com/adoptium/temurin25-binaries/releases/download/jdk-" + version.replace("+", "%2B")), url
        assert f"_{ARCHIVES[platform]}_hotspot_{version.replace('+', '_')}.tar.gz" in url, url
        assert len(sha) == 64 and int(sha, 16) >= 0, (platform, sha)
    assert len({pins[f"JDK_SHA256_{p}"] for p in PLATFORMS}) == len(PLATFORMS), "two platforms share a hash"


def test_setup_links_java_for_both_jdk_layouts(tmp_path: Path):
    import subprocess

    from harness.server import ROOT

    script = (ROOT / "tools" / "setup.sh").read_text()
    link_java = script[script.index("link_java() {"):script.index("}\n", script.index("link_java() {")) + 2]
    for machine, layout in (("mac", "jdk/Contents/Home/bin"), ("linux", "jdk/bin")):
        tools = tmp_path / machine / ".tools"
        (tools / layout).mkdir(parents=True)
        (tools / layout / "java").write_text("#!/bin/sh\necho fake java\n")
        (tools / layout / "java").chmod(0o755)
        run = subprocess.run(["bash", "-c", f'JAVA=.tools/java\n{link_java}\nlink_java && .tools/java'],
                             cwd=tools.parent, capture_output=True, text=True)
        assert run.returncode == 0 and run.stdout == "fake java\n", (layout, run.stderr)
        assert (tools / "java").is_symlink() and str((tools / "java").readlink()) == layout + "/java"
    empty = tmp_path / "none" / ".tools"
    empty.mkdir(parents=True)
    run = subprocess.run(["bash", "-c", f"JAVA=.tools/java\n{link_java}\nlink_java"], cwd=empty.parent, capture_output=True, text=True)
    assert run.returncode == 1
