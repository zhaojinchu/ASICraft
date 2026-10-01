#!/usr/bin/env bash
# Installs the pinned tools from versions.env. Safe to re-run: anything already
# present with the right version or hash is skipped.
#
# Project-local: JDK 25 and the Minecraft server jar (.tools/), Python packages
# (.venv/), Node packages (node_modules/).
# Outside the project: rustup in ~/.cargo and ~/.rustup (your shell profile is
# not modified), and Verilator through Homebrew.

set -euo pipefail
cd "$(dirname "$0")/.."
source ./versions.env
mkdir -p .tools/mc

say() { printf '== %s\n' "$*"; }

# --- JDK (Minecraft 26.x needs Java 25)
JAVA=.tools/jdk/Contents/Home/bin/java
if [ -x "$JAVA" ] && "$JAVA" -version 2>&1 | grep -q "${JDK_VERSION%%+*}"; then
    say "JDK $JDK_VERSION already installed"
else
    say "downloading JDK $JDK_VERSION"
    tmp=$(mktemp -d)
    curl -fsSL "$JDK_URL" -o "$tmp/jdk.tar.gz"
    echo "$JDK_SHA256  $tmp/jdk.tar.gz" | shasum -a 256 -c -
    rm -rf .tools/jdk && mkdir -p .tools/jdk
    tar -xzf "$tmp/jdk.tar.gz" -C .tools/jdk --strip-components=1
    rm -rf "$tmp"
fi

# --- Minecraft server jar
JAR=.tools/mc/server-$MC_VERSION.jar
if [ -f "$JAR" ] && [ "$(shasum -a 1 "$JAR" | cut -d' ' -f1)" = "$MC_SERVER_SHA1" ]; then
    say "Minecraft server $MC_VERSION already downloaded"
else
    say "downloading Minecraft server $MC_VERSION"
    curl -fsSL "$MC_SERVER_URL" -o "$JAR"
    echo "$MC_SERVER_SHA1  $JAR" | shasum -a 1 -c -
fi

# --- Rust (version comes from rust-toolchain.toml)
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v rustup >/dev/null; then
    say "installing rustup (shell profile left untouched)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
        sh -s -- -y --no-modify-path --default-toolchain none
fi
rustup toolchain install >/dev/null
say "$(cargo --version)"

# --- Verilator
if command -v verilator >/dev/null; then
    say "$(verilator --version)"
else
    say "installing Verilator with Homebrew"
    brew install verilator
fi

# --- Python and Node packages (exact versions from the lock files)
uv sync --frozen
npm ci --no-audit --no-fund

say "setup complete; run ./check --vanilla"
