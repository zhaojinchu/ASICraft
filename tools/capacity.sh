#!/usr/bin/env bash
# Builds the capacity designs (tests/capacity, outside the gate) with the release
# binary, each stopped after CAP seconds (900 by default), into
# build/capacity/LABEL, and prints each one's footprint, repeaters and settle or
# period, beside an earlier LABEL's when one is given.
#   tools/capacity.sh LABEL [EARLIER_LABEL] [DESIGN ...]
set -u
cd "$(dirname "$0")/.."
label=${1:?usage: tools/capacity.sh LABEL [EARLIER_LABEL] [DESIGN ...]}
shift
earlier=""
if [ $# -gt 0 ] && [ -d "build/capacity/$1" ]; then
  earlier=$1
  shift
fi
designs=("$@")
[ ${#designs[@]} -eq 0 ] && designs=(mul4 alu8 counter8 regfile2x4 cpu4)
cargo build --release --locked -q || exit 1
bin=target/release/redfab
[ -x "$bin.exe" ] && bin=$bin.exe
out=build/capacity/$label
mkdir -p "$out"
cap=${CAP:-900}
for d in "${designs[@]}"; do
  rm -rf "${out:?}/$d"
  SECONDS=0
  "$bin" build "tests/capacity/$d/mapped.json" --top top -o "$out/$d" > "$out/$d.log" 2>&1 &
  pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$SECONDS" -gt "$cap" ]; then
      kill "$pid"
      echo "$d: stopped after $cap s"
      break
    fi
    sleep 2
  done
  wait "$pid" 2>/dev/null
  echo "$d: $SECONDS s, $(grep -m1 'placement:' "$out/$d.log" | sed 's/^ *//')"
done
python3 - "$label" "$earlier" "${designs[@]}" <<'PY'
import json, os, sys

label, earlier, designs = sys.argv[1], sys.argv[2], sys.argv[3:]
for d in designs:
    row = []
    for v in [earlier, label] if earlier else [label]:
        p = f"build/capacity/{v}/{d}/design.json"
        if not os.path.exists(p):
            row.append(f"{v}: -")
            continue
        j = json.load(open(p))
        seq = j["timing"].get("sequential")
        t = f"period {seq['period']}" if seq else f"settle {j['settle_ticks']}"
        x, z = j["size"][0], j["size"][2]
        row.append(f"{v}: {x} x {z} ({x * z}), {j['repeaters']} repeaters, {t}")
    print(d, " | ".join(row))
PY
