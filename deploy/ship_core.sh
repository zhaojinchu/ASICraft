#!/usr/bin/env bash
# Builds the core for the browser and the backend (wasm32, release: the build
# `wasm-identical` holds to the native one) on this machine and copies it to the
# deployment's dist/, with its checksum, so the deployed machine needs no Rust.
#
#   deploy/ship_core.sh USER@HOST [REMOTE_ROOT]
#
# Run it after every change to the core, then restart the backend there
# (systemctl restart redfab-live).
set -euo pipefail
cd "$(dirname "$0")/.."
target=${1:?usage: deploy/ship_core.sh USER@HOST [REMOTE_ROOT]}
remote_root=${2:-/opt/redfab}
export PATH="$HOME/.cargo/bin:$PATH"
cargo build -q -p redfab-wasm --target wasm32-unknown-unknown --release --locked
core=target/wasm32-unknown-unknown/release/redfab_wasm.wasm
sum=$(shasum -a 256 "$core" | cut -d' ' -f1)
echo "redfab_wasm.wasm $(wc -c < "$core") bytes, sha256 $sum (from $(git rev-parse --short HEAD))"
ssh "$target" "mkdir -p '$remote_root/dist'"
scp -q "$core" "$target:$remote_root/dist/redfab_wasm.wasm"
ssh "$target" "cd '$remote_root/dist' && echo '$sum  redfab_wasm.wasm' | sha256sum -c - && chown redfab:redfab redfab_wasm.wasm 2>/dev/null || true"
echo "shipped; restart the backend there: systemctl restart redfab-live"
