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


def run_helpers(tmp_path: Path, body: str, only: str = "", vanilla: int = 0) -> tuple[str, list[str]]:
    """Runs `body` after the check's verdict helpers (record, run, run_background,
    wait_background) with the script's state variables set as a run sets them."""
    script = (ROOT / "check").read_text()
    helpers = script[script.index("# record <name>"):script.index("# refused_with <message>")]
    (tmp_path / "build" / "check").mkdir(parents=True, exist_ok=True)
    summary = tmp_path / "SUMMARY"
    summary.unlink(missing_ok=True)
    prelude = f'set -u\nSUMMARY={summary}\nPASSED=0\nFAILED=()\nONLY="{only}"\nVANILLA={vanilla}\n'
    result = subprocess.run(["bash", "-c", prelude + helpers + body], cwd=tmp_path, capture_output=True, text=True)
    assert result.returncode == 0, result.stdout + result.stderr
    return result.stdout, summary.read_text().splitlines() if summary.exists() else []


def test_a_background_check_runs_beside_the_next_ones_and_is_recorded_when_waited_for(tmp_path: Path):
    out, summary = run_helpers(
        tmp_path,
        "run_background slow sh -c 'sleep 1; echo done; exit 0'\n"
        "run quick sh -c 'exit 0'\n"
        "wait_background\n"
        'echo "passed=$PASSED failed=${FAILED[*]:-none}"\n',
    )
    lines = out.splitlines()
    assert lines[0].startswith("  ....  slow") and "background" in lines[0], out
    assert lines[1].startswith("  PASS  quick"), "the next check ran without waiting"
    assert lines[2].startswith("  PASS  slow"), out
    assert lines[3] == "passed=2 failed=none"
    names = [line.split()[1] for line in summary]
    assert names == ["quick", "slow"], summary
    assert all(line.startswith("PASS ") and line.split()[2].isdigit() for line in summary)
    assert (tmp_path / "build" / "check" / "slow.log").read_text() == "done\n"


def test_a_failing_background_check_fails_the_run_with_its_log(tmp_path: Path):
    # The seconds recorded are the job's own: it fails at once while the next check
    # takes two seconds, and it is waited for only after that.
    out, summary = run_helpers(
        tmp_path,
        "run_background bad sh -c 'echo why it failed; exit 3'\n"
        "run slow sh -c 'sleep 2'\n"
        "wait_background\n"
        'echo "passed=$PASSED failed=${FAILED[*]:-none}"\n',
    )
    assert "  FAIL  bad" in out and "(see build/check/bad.log)" in out, out
    assert "        | why it failed" in out, "the failing log's tail is shown"
    assert out.splitlines()[-1] == "passed=1 failed=bad"
    assert [line.split()[1] for line in summary] == ["slow", "bad"], summary
    assert summary[1].startswith("FAIL bad ") and int(summary[1].split()[2]) < 2, summary


def test_a_background_check_runs_in_place_under_only_or_with_the_server(tmp_path: Path):
    # With --only, and in the vanilla tier (beside the server it would compete for
    # memory), the check runs like any other: its verdict comes before the next check's.
    body = "run_background bg sh -c 'exit 0'\nrun other sh -c 'exit 0'\nwait_background\n"
    out, summary = run_helpers(tmp_path, body, only="bg")
    assert out.splitlines() == [line for line in out.splitlines() if "background" not in line], out
    assert [line.split()[1] for line in summary] == ["bg"], "only the named check runs"
    out, summary = run_helpers(tmp_path, body, vanilla=1)
    assert "background" not in out, out
    assert [line.split()[1] for line in summary] == ["bg", "other"], summary


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
