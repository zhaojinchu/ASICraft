"""Run tools/smoke.ys on one SystemVerilog file with the PyPI build of YoWASP Yosys.

usage: yosys_py.py <src.sv> <top> <out.json>
Exits non-zero if Yosys fails or the top module is missing from the netlist.
"""

import json
import os
import shutil
import sys
import tempfile
from pathlib import Path

import yowasp_yosys


def yosys_commands(top: str) -> str:
    lines = (Path(__file__).parent / "smoke.ys").read_text().splitlines()
    cmds = [ln.strip() for ln in lines if ln.strip() and not ln.lstrip().startswith("#")]
    return "; ".join(cmds).replace("{top}", top)


def main() -> int:
    src, top, out = Path(sys.argv[1]).resolve(), sys.argv[2], Path(sys.argv[3]).resolve()
    script = yosys_commands(top)
    with tempfile.TemporaryDirectory() as tmp:
        shutil.copy(src, Path(tmp) / "in.sv")
        os.chdir(tmp)  # YoWASP only sees files under the working directory
        code = yowasp_yosys.run_yosys(["-q", "-p", script])
        if code != 0:
            print(f"yosys exited with {code}", file=sys.stderr)
            return 1
        netlist = json.loads(Path("out.json").read_text())
    module = netlist["modules"].get(top)
    if module is None or not module["ports"]:
        print(f"top module {top!r} missing or has no ports", file=sys.stderr)
        return 1
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(netlist, indent=1, sort_keys=True))
    print(f"{netlist['creator']}\nports: {sorted(module['ports'])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
