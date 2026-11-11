"""The documents must agree with the tree, and each drift the checker guards must be named."""

from pathlib import Path

from harness.docs_check import (
    ROOT,
    backticked,
    check_budget,
    check_commands,
    check_designs,
    check_fixture_counts,
    check_license,
    check_limits,
    check_paths,
    check_refusals,
    check_versions,
    problems,
    section,
    smoke_errors,
)

PINS = {"MC_VERSION": "26.3", "VERILATOR_VERSION": "5.052", "NODE_MAJOR": "26", "JDK_VERSION": "25.0.4.1+1"}
README = """# x

Minecraft Java 26.3, Verilator 5.052, Node 26, JDK 25, Python 3.14.

## License

MIT; see `LICENSE`.
"""


def test_the_tree_agrees_with_its_documents():
    assert problems(ROOT) == []


def test_sections_and_backticks_are_found():
    text = "# A\n\n`one` and `two/`\n\n## B\n\n`three.md`\n\n### C\n\nx\n"
    assert backticked(text) == ["one", "two/", "three.md"]
    assert section(text, "B") == "\n`three.md`\n\n"
    assert section(text, "D") == ""


def test_license_drift_is_named():
    manifests = {"Cargo.toml": "MIT", "package.json": "MIT"}
    assert check_license(README, "MIT License\n\nCopyright", manifests) == []
    assert check_license(README, "Apache License", manifests) == ["LICENSE does not start with 'MIT License'"]
    assert check_license(README, "MIT License", {"package.json": None}) == ["package.json declares license None, LICENSE is MIT"]
    assert check_license(README.replace("MIT;", "see"), "MIT License", manifests) == ["README.md's License section does not say MIT"]


def test_version_drift_is_named():
    assert check_versions(README, PINS, "3.14", "3.14") == []
    found = check_versions(README, {**PINS, "VERILATOR_VERSION": "5.053"}, "3.14", "3.14")
    assert found == ["README.md does not mention 'Verilator 5.053' (versions.env / .python-version)"]
    found = check_versions(README, PINS, "3.14", "3.12")
    assert found == [".python-version pins Python 3.14 but this runs under 3.12; uv did not pick it up"]
    assert len(check_versions(README, PINS, "3.13", "3.13")) == 1  # the README still says 3.14


def test_refusal_drift_is_named():
    sources = {"a.sv": "// Refused: x.\n// error: port `bus` is inout: no\nmodule a; endmodule\n",
               "b.sv": "// Builds.\n// expect: ok\n", "c.sv": "// error: one\n// error: two\n"}
    assert smoke_errors(sources) == ["port `bus` is inout: no", "one", "two"]
    readme = "| x | ``port `bus` is inout: no`` |\n| y | `one` |\n"
    assert check_refusals(readme, smoke_errors(sources)) == ["README.md's refusal table lacks: two"]


def test_design_list_drift_is_named():
    plan = "## Where things live\n\n- `designs/`: `nor2`, `not1` and the clocked `fsm`\n"
    readme = "`nor2`, `not1`, `fsm`"
    assert check_designs(readme, plan, ["fsm", "nor2", "not1"]) == []
    found = check_designs(readme, plan, ["fsm", "nor2", "not1", "adder8"])
    assert found == ["README.md does not name design `adder8`",
                     "docs/PLAN.md's designs/ line lists ['fsm', 'nor2', 'not1'], designs/ holds ['adder8', 'fsm', 'nor2', 'not1']"]
    assert check_designs(readme, "nothing", ["fsm"]) == ["docs/PLAN.md has no '- `designs/`:' line under Where things live"]


def test_missing_paths_are_named(tmp_path: Path):
    (tmp_path / "fixtures" / "mechanics").mkdir(parents=True)
    (tmp_path / "docs").mkdir()
    (tmp_path / "docs" / "PLAN.md").write_text("")
    text = "- `fixtures/`: recordings (`mechanics/`, `fuzz/`)\n- `docs/PLAN.md`, `cells/*.tile`, `synth.rs`\n"
    assert check_paths(text, "here", tmp_path) == ["here names `fuzz/`, which does not exist"]


def test_limit_drift_is_named():
    readme = ("## Limits\n\n| Routes | about 100 cells (`measuredCells`) |\n| Fits | up to 512 x 512 blocks |\n"
              "| Inputs | 64 bits |\n\n## Next\n\nAt most 64 input bits.\n")
    assert check_limits(readme, "100", "64", ("512", "512")) == []
    assert check_limits(readme, "120", "64", ("512", "512")) == [
        "README.md's Limits section lacks 'about 120 cells (`measuredCells`)' (playground/compile.js measuredCells)"]
    assert len(check_limits(readme, "100", "65", ("512", "512"))) == 2
    assert len(check_limits(readme, "100", "64", ("256", "512"))) == 1


def test_command_drift_is_named():
    readme = "tools/setup.sh --accept-eula ./check --only NAME `--clean` /place template /forceload add --top"
    check = "#   ./check --vanilla\n#   ./check --only NAME\n#   ./check --clean\n"
    assert check_commands(readme, "--accept-eula", check) == []
    assert check_commands(readme.replace("--top", ""), "--accept-eula", check) == ["README.md does not mention '--top'"]
    assert check_commands(readme, "", check) == ["tools/setup.sh has no --accept-eula"]
    assert check_commands(readme, "--accept-eula", check.replace("--clean", "--wipe")) == ["check's usage header does not list --clean"]


def test_budget_drift_is_named():
    mechanics = "allows at most **6** (`BURNOUT_BUDGET` in x)"
    design = ("Budget: at most 6 turn-offs per 60-gt window (`BURNOUT_BUDGET`, one to spare) ... "
              "(`BURNOUT_BUDGET`, stage 9) is **6**, one ... the budget in the code is 6, see *As built* below")
    plan = "(stage 9, `BURNOUT_BUDGET`) is 6 turn-offs in any 60 ticks"
    assert check_budget("6", mechanics, design, plan) == []
    found = check_budget("7", mechanics, design, plan)
    assert len(found) == 5 and all("BURNOUT_BUDGET = 7" in f for f in found)
    assert [f for f in check_budget("6", mechanics.replace("6", "7"), design, plan)] == [
        "docs/mechanics.md does not state the burnout budget as 'allows at most **6** (`BURNOUT_BUDGET`' (BURNOUT_BUDGET = 6)"]


def test_fixture_count_drift_is_named():
    mechanics = "(`mechanics-verify`: 31 fixtures, 62 recordings). **All 31 fixtures behave identically in both modes.**"
    assert check_fixture_counts(mechanics, 31, 62) == []
    assert check_fixture_counts(mechanics, 32, 64) == [
        "docs/mechanics.md does not say '`mechanics-verify`: 32 fixtures, 64 recordings' (fixtures/mechanics holds 32 fixtures, 64 recordings)",
        "docs/mechanics.md does not say 'All 32 fixtures behave identically' (fixtures/mechanics holds 32 fixtures, 64 recordings)"]


def test_a_missing_document_is_the_first_problem(tmp_path: Path):
    (tmp_path / "README.md").write_text("x")
    (tmp_path / "docs").mkdir()
    (tmp_path / "docs" / "REFERENCE.md").write_text("x")
    (tmp_path / ".python-version").write_text("3.14\n")
    assert problems(tmp_path) == ["LICENSE is missing or empty"]
    (tmp_path / "docs" / "REFERENCE.md").unlink()
    assert problems(tmp_path) == ["docs/REFERENCE.md is missing or empty", "LICENSE is missing or empty"]
