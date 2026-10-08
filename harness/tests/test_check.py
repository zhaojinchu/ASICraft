"""The gate script itself: `--only` runs one check and names an unknown one, every run
leaves a SUMMARY with a header, one line per check and a verdict, and the negative-control
helper can fail. The runs here write their SUMMARY to a temporary path (REDFAB_CHECK_SUMMARY)
so they do not clobber the record of the run that may be running them."""

import os
import subprocess
from pathlib import Path

from harness.server import ROOT


def run_check(tmp_path: Path, *args: str) -> tuple[subprocess.CompletedProcess, list[str]]:
    summary = tmp_path / "SUMMARY"
    env = {**os.environ, "REDFAB_CAFFEINATED": "1", "REDFAB_CHECK_SUMMARY": str(summary)}
    result = subprocess.run([str(ROOT / "check"), *args], cwd=ROOT, env=env, capture_output=True, text=True)
    return result, summary.read_text().splitlines() if summary.exists() else []


def test_an_unknown_check_name_exits_2_and_is_recorded(tmp_path: Path):
    result, summary = run_check(tmp_path, "--only", "no-such-check")
    assert result.returncode == 2
    assert "no check named 'no-such-check'; the names are what a run prints" in result.stdout
    assert summary[0].startswith("redfab check --only no-such-check started 20")
    assert summary[1:] == ["no check named no-such-check"]


def test_a_bad_argument_exits_2(tmp_path: Path):
    result, summary = run_check(tmp_path, "--wat")
    assert result.returncode == 2 and "unknown argument: --wat" in result.stderr
    assert summary == []


def test_one_fast_check_runs_alone_and_the_summary_records_it(tmp_path: Path):
    # negatives-can-fail is the self-test of the refused_with helper: it needs no tool but sh.
    result, summary = run_check(tmp_path, "--only", "negatives-can-fail")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "  PASS  negatives-can-fail" in result.stdout
    assert "all 1 checks passed in" in result.stdout
    assert "stage 0" not in result.stdout, "a single check prints no stage headings"
    assert summary[0].startswith("redfab check --only negatives-can-fail started 20") and " at " in summary[0]
    name, secs = summary[1].split()[1:]
    assert summary[1].startswith("PASS ") and name == "negatives-can-fail" and secs.isdigit()
    assert summary[2].startswith("all 1 checks passed in ") and summary[2].endswith(" s")
    assert len(summary) == 3


def test_the_negative_control_helper_fails_on_a_crash_without_the_message(tmp_path: Path):
    # The same assertions the check makes, run straight against the helper: a crash for
    # another reason, a success, and the message with a success are all failures.
    script = (ROOT / "check").read_text()
    helper = script[script.index("refused_with() {"):script.index("# ---------------------------------------------------------------- stage 0")]
    cases = {
        "sh -c 'echo other; exit 1'": 1,
        "sh -c 'exit 0'": 1,
        "sh -c 'echo the message; exit 0'": 1,
        "/nonexistent/program": 1,
        "sh -c 'echo the message; exit 1'": 0,
    }
    for command, expected in cases.items():
        result = subprocess.run(["bash", "-c", f"{helper}\nrefused_with 'the message' {command}"], capture_output=True, text=True)
        assert result.returncode == expected, (command, result.stdout)
    assert "NOT refused: the command succeeded" in subprocess.run(
        ["bash", "-c", f"{helper}\nrefused_with 'the message' sh -c 'exit 0'"], capture_output=True, text=True).stdout
