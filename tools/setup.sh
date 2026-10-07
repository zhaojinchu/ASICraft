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

# --- Fabric Loader: the launcher jar, run once so it installs the loader and Mixin
# beside the vanilla server's libraries; .tools/fabric is then copied into each
# modded server's run directory.
FABRIC=.tools/fabric
if [ -f "$FABRIC/libraries/net/fabricmc/fabric-loader/$FABRIC_LOADER_VERSION/fabric-loader-$FABRIC_LOADER_VERSION.jar" ]; then
    say "Fabric Loader $FABRIC_LOADER_VERSION already installed"
else
    say "installing Fabric Loader $FABRIC_LOADER_VERSION"
    rm -rf "$FABRIC" && mkdir -p "$FABRIC"
    curl -fsSL "$FABRIC_LAUNCHER_URL" -o "$FABRIC/fabric-server-launch.jar"
    echo "$FABRIC_LAUNCHER_SHA256  $FABRIC/fabric-server-launch.jar" | shasum -a 256 -c -
    (
        cd "$FABRIC"
        printf 'serverJar=%s\n' "$(cd ../.. && pwd)/$JAR" > fabric-server-launcher.properties
        echo "eula=true" > eula.txt
        printf 'server-port=25599\nenable-rcon=false\nonline-mode=false\nlevel-type=minecraft\\:flat\n' > server.properties
        # Starts the server once to fetch the libraries; stop it as soon as it is up.
        ../jdk/Contents/Home/bin/java -Xmx1G -jar fabric-server-launch.jar nogui > install.log 2>&1 &
        pid=$!
        for _ in $(seq 1 120); do
            grep -q 'Done (' install.log 2>/dev/null && break
            sleep 1
        done
        kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null || true
        rm -rf world logs mods eula.txt server.properties banned-*.json ops.json usercache.json whitelist.json
    )
    [ -f "$FABRIC/.fabric/server/$MC_VERSION-server.jar" ] || { echo "Fabric install failed; see $FABRIC/install.log"; exit 1; }
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
