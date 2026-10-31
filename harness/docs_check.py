"""Stage 0 check: the documents agree with the code and the tree.

    uv run python -m harness.docs_check

Nothing else reads README.md or docs/, so a number quoted there could drift from the
code without any test noticing. This ties each quoted fact to its source:

- README.md and LICENSE exist, and the license is MIT in LICENSE and every manifest;
- the tool versions README names equal versions.env and .python-version, and the
  interpreter this runs under is the pinned one;
- README's refusal table quotes every `// error:` line of tests/smoke/*.sv;
- every design under designs/ is named in README.md, and PLAN.md's `designs/` line
  lists exactly those; the paths "Where things live" and "Where to read more" name exist;
- the limits README quotes equal the constants in the code (measuredCells, MAX_INPUTS,
  plotSize); the flags and commands it quotes exist;
- the burnout budget the documents state equals BURNOUT_BUDGET;
- the fixture and recording counts in mechanics.md equal fixtures/mechanics.

Each function takes text and returns the problems it finds, so a drift can be tested
without a tree; `problems(root)` reads the real one.
"""

from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path

from .server import ROOT, read_versions


def backticked(text: str) -> list[str]:
    """The single-backticked names in `text`, in order."""
    return re.findall(r"`([^`\n]+)`", text)


def section(text: str, heading: str) -> str:
    """The text under a Markdown heading, up to the next heading of any level."""
    match = re.search(rf"^#+ {re.escape(heading)}\n(.*?)(?=^#+ |\Z)", text, re.M | re.S)
    return match.group(1) if match else ""


def constant(text: str, pattern: str, where: str) -> str:
    """The first group of `pattern` in `text`, or a failure naming `where`."""
    match = re.search(pattern, text)
    if not match:
        raise LookupError(f"{where}: nothing matches {pattern!r}")
    return match.group(1)


def smoke_errors(sources: dict[str, str]) -> list[str]:
    """Every `// error: TEXT` header line of the smoke sources (name -> text)."""
    return [line[len("// error: "):] for text in sources.values() for line in text.splitlines() if line.startswith("// error: ")]


def check_license(readme: str, license_text: str, manifests: dict[str, str]) -> list[str]:
    """LICENSE is MIT and every manifest says so (manifests: path -> the license it declares)."""
    out = []
    if not license_text.startswith("MIT License"):
        out.append("LICENSE does not start with 'MIT License'")
    for path, declared in manifests.items():
        if declared != "MIT":
            out.append(f"{path} declares license {declared!r}, LICENSE is MIT")
    if "MIT" not in section(readme, "License"):
        out.append("README.md's License section does not say MIT")
    return out


def check_versions(readme: str, pins: dict[str, str], python_pin: str, running: str) -> list[str]:
    """README names the pinned tool versions; the Python running this is the pinned one."""
    out = []
    for phrase in (f"Minecraft Java {pins['MC_VERSION']}", f"Verilator {pins['VERILATOR_VERSION']}",
                   f"Node {pins['NODE_MAJOR']}", f"JDK {pins['JDK_VERSION'].split('.')[0]}", f"Python {python_pin}"):
        if phrase not in readme:
            out.append(f"README.md does not mention '{phrase}' (versions.env / .python-version)")
    if running != python_pin:
        out.append(f".python-version pins Python {python_pin} but this runs under {running}; uv did not pick it up")
    return out


def check_refusals(readme: str, errors: list[str]) -> list[str]:
    """Every refusal message of tests/smoke/*.sv is quoted in README's table."""
    return [f"README.md's refusal table lacks: {text}" for text in errors if text not in readme]


def check_designs(readme: str, plan: str, names: list[str]) -> list[str]:
    """Every design directory is named in README, and PLAN.md's `designs/` line lists exactly them."""
    out = [f"README.md does not name design `{name}`" for name in names if f"`{name}`" not in readme]
    line = next((ln for ln in plan.splitlines() if ln.startswith("- `designs/`:")), "")
    if not line:
        return out + ["docs/PLAN.md has no '- `designs/`:' line under Where things live"]
    listed = sorted(backticked(line)[1:])
    if listed != sorted(names):
        out.append(f"docs/PLAN.md's designs/ line lists {listed}, designs/ holds {sorted(names)}")
    return out


def path_tokens(text: str) -> list[tuple[str, str]]:
    """The backticked paths of each line of `text` (a token with a slash in it, or one
    ending in .md), paired with the first path on its line so `mechanics/` in the
    `fixtures/` line can be resolved under it. Globs are left out."""
    out = []
    for line in text.splitlines():
        tokens = [t for t in backticked(line) if ("/" in t or t.endswith(".md")) and "*" not in t and " " not in t]
        out += [(t, tokens[0]) for t in tokens]
    return out


def check_paths(text: str, where: str, root: Path) -> list[str]:
    """Every path a section names exists on disk."""
    out = []
    for token, first in path_tokens(text):
        if not (root / token).exists() and not (root / first / token).exists():
            out.append(f"{where} names `{token}`, which does not exist")
    return out


def check_limits(readme: str, measured_cells: str, max_inputs: str, plot: tuple[str, str]) -> list[str]:
    """The numbers README's Limits section quotes equal the code's constants."""
    out = []
    # The heading carries the date of the last measurement.
    dated = re.search(r"^#+ Limits \(measured \d{4}-\d{2}-\d{2}\)\n(.*?)(?=^#+ |\Z)", readme, re.M | re.S)
    limits = dated.group(1) if dated else section(readme, "Limits")
    for phrase, source in ((f"about {measured_cells} cells (`measuredCells`)", "playground/compile.js measuredCells"),
                           (f"| Inputs | {max_inputs} bits |", "build.rs MAX_INPUTS"),
                           (f"up to {plot[0]} x {plot[1]} blocks", "server/live.mjs plotSize")):
        if phrase not in limits:
            out.append(f"README.md's Limits section lacks '{phrase}' ({source})")
    if f"At most {max_inputs} input bits" not in readme:
        out.append(f"README.md does not say 'At most {max_inputs} input bits' (build.rs MAX_INPUTS)")
    return out


def check_commands(readme: str, setup: str, check: str) -> list[str]:
    """The flags and commands README tells the user to run exist where it says."""
    out = []
    for phrase in ("tools/setup.sh --accept-eula", "./check --only NAME", "`--clean`", "/place template", "/forceload add", "--top"):
        if phrase not in readme:
            out.append(f"README.md does not mention '{phrase}'")
    if "--accept-eula" not in setup:
        out.append("tools/setup.sh has no --accept-eula")
    for flag in ("--only", "--clean", "--vanilla"):
        if f"./check {flag}" not in check:
            out.append(f"check's usage header does not list {flag}")
    return out


def check_budget(budget: str, mechanics: str, design: str, plan: str) -> list[str]:
    """The burnout budget every document states equals BURNOUT_BUDGET."""
    out = []
    for name, text, phrase in (
        ("docs/mechanics.md", mechanics, f"allows at most **{budget}** (`BURNOUT_BUDGET`"),
        ("docs/DESIGN.md Appendix A", design, f"Budget: at most {budget} turn-offs per 60-gt window (`BURNOUT_BUDGET`"),
        ("docs/DESIGN.md note 5", design, f"(`BURNOUT_BUDGET`, stage 9) is **{budget}**"),
        ("docs/DESIGN.md note 5 derivation", design, f"the budget in the code is {budget}, see *As built* below"),
        ("docs/PLAN.md", plan, f"(stage 9, `BURNOUT_BUDGET`) is {budget} turn-offs"),
    ):
        if phrase not in text:
            out.append(f"{name} does not state the burnout budget as '{phrase}' (BURNOUT_BUDGET = {budget})")
    return out


def check_fixture_counts(mechanics: str, fixtures: int, recordings: int) -> list[str]:
    """mechanics.md's header states the number of fixtures and recordings on disk."""
    out = []
    for phrase in (f"`mechanics-verify`: {fixtures} fixtures, {recordings} recordings", f"All {fixtures} fixtures behave identically"):
        if phrase not in mechanics:
            out.append(f"docs/mechanics.md does not say '{phrase}' (fixtures/mechanics holds {fixtures} fixtures, {recordings} recordings)")
    return out


def problems(root: Path = ROOT) -> list[str]:
    """Every disagreement between the documents and the tree under `root`."""
    out = []
    for required in ("README.md", "LICENSE", ".python-version"):
        if not (root / required).is_file() or not (root / required).read_text().strip():
            out.append(f"{required} is missing or empty")
    if out:
        return out
    read = lambda rel: (root / rel).read_text()  # noqa: E731
    readme, plan, design, mechanics = read("README.md"), read("docs/PLAN.md"), read("docs/DESIGN.md"), read("docs/mechanics.md")
    pins = read_versions() if root == ROOT else {}
    python_pin = read(".python-version").strip()
    running = "%d.%d" % sys.version_info[:2]

    manifests = {
        "Cargo.toml": tomllib.loads(read("Cargo.toml")).get("workspace", {}).get("package", {}).get("license"),
        "package.json": json.loads(read("package.json")).get("license"),
        "pyproject.toml": tomllib.loads(read("pyproject.toml"))["project"].get("license"),
        "mod/resources/fabric.mod.json": json.loads(read("mod/resources/fabric.mod.json")).get("license"),
    }
    out += check_license(readme, read("LICENSE"), manifests)
    if "LICENSE" not in tomllib.loads(read("pyproject.toml"))["project"].get("license-files", []):
        out.append("pyproject.toml does not list LICENSE in license-files")
    out += check_versions(readme, pins, python_pin, running)
    sources = {p.name: p.read_text() for p in sorted((root / "tests" / "smoke").glob("*.sv"))}
    out += check_refusals(readme, smoke_errors(sources))
    out += check_designs(readme, plan, sorted(p.name for p in (root / "designs").iterdir() if p.is_dir()))
    out += check_paths(section(plan, "Where things live"), "docs/PLAN.md Where things live", root)
    out += check_paths(section(readme, "Where to read more"), "README.md Where to read more", root)
    try:
        out += check_limits(
            readme,
            constant(read("playground/compile.js"), r"measuredCells:\s*(\d+)", "playground/compile.js"),
            constant(read("crates/redfab-core/src/build.rs"), r"pub const MAX_INPUTS: usize = (\d+);", "build.rs"),
            re.search(r"plotSize:\s*\[(\d+),\s*(\d+)\]", read("server/live.mjs")).groups(),  # type: ignore[union-attr]
        )
        out += check_budget(
            constant(read("crates/redfab-core/src/timing.rs"), r"pub const BURNOUT_BUDGET: usize = (\d+);", "timing.rs"),
            mechanics, design, plan,
        )
    except (LookupError, AttributeError) as e:
        out.append(f"a constant moved: {e}")
    out += check_commands(readme, read("tools/setup.sh"), read("check"))
    mech = root / "fixtures" / "mechanics"
    out += check_fixture_counts(mechanics, len(list(mech.glob("*.fixture.json"))), len(list(mech.glob("*.trace.json"))))
    return out


def main() -> int:
    found = problems()
    for problem in found:
        print("DRIFT " + problem)
    if found:
        print(f"{len(found)} place(s) where the documents and the tree disagree")
        return 1
    print("README.md, LICENSE and docs/ agree with versions.env, the smoke sources, designs/, the constants and the fixtures")
    return 0


if __name__ == "__main__":
    sys.exit(main())
