"""Compare two Yosys JSON netlists. They must be identical, including the
"creator" line, which names the exact Yosys build.

usage: netlist_eq.py <a.json> <b.json>
"""

import json
import sys
from pathlib import Path


def first_difference(a, b, path="$"):
    if type(a) is not type(b):
        return f"{path}: {type(a).__name__} vs {type(b).__name__}"
    if isinstance(a, dict):
        for key in sorted(set(a) | set(b)):
            if key not in a or key not in b:
                return f"{path}.{key}: present in only one file"
            diff = first_difference(a[key], b[key], f"{path}.{key}")
            if diff:
                return diff
    elif isinstance(a, list):
        if len(a) != len(b):
            return f"{path}: length {len(a)} vs {len(b)}"
        for i, (x, y) in enumerate(zip(a, b)):
            diff = first_difference(x, y, f"{path}[{i}]")
            if diff:
                return diff
    elif a != b:
        return f"{path}: {a!r} vs {b!r}"
    return None


def main() -> int:
    a, b = (json.loads(Path(p).read_text()) for p in sys.argv[1:3])
    diff = first_difference(a, b)
    if diff:
        print(f"netlists differ at {diff}", file=sys.stderr)
        return 1
    print("netlists identical")
    return 0


if __name__ == "__main__":
    sys.exit(main())
