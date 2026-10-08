#!/usr/bin/env bash
# Installs the pinned tools from versions.env. Safe to re-run: anything already
# present with the right version or hash is skipped.
#
#   tools/setup.sh --accept-eula   accept the Minecraft server's EULA and install everything
#   tools/setup.sh                 only if the EULA was accepted on an earlier run
#   tools/setup.sh --clean         also remove old server run directories and gate logs under build/
#
# Project-local: JDK 25 and the Minecraft server jar (.tools/), Python packages
# (.venv/), Node packages (node_modules/). Outside the project: rustup in ~/.cargo
# and ~/.rustup (your shell profile is not modified), and Verilator through the
# system package manager (Homebrew on macOS, apt on Debian and Ubuntu).
#
# Running the Minecraft server binds whoever runs it to Mojang's EULA
# (https://aka.ms/MinecraftEULA). This script, the harness and the live backend
# only start a server after `--accept-eula` has recorded your acceptance in
# .tools/eula-accepted.

set -euo pipefail
cd "$(dirname "$0")/.."
source ./versions.env
mkdir -p .tools/mc

say() { printf '== %s\n' "$*"; }

EULA_FILE=.tools/eula-accepted
EULA_URL=https://aka.ms/MinecraftEULA
CLEAN=0
for arg in "$@"; do
    case "$arg" in
        --accept-eula)
            printf 'accepted %s on %s\n' "$EULA_URL" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$EULA_FILE"
            ;;
        --clean) CLEAN=1 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done
if [ ! -f "$EULA_FILE" ]; then
    cat >&2 <<MSG
The Minecraft server's EULA ($EULA_URL) must be accepted by whoever runs the
server, and this setup starts one (to install Fabric Loader). Read it, then run:

    tools/setup.sh --accept-eula

which records the acceptance in $EULA_FILE. Nothing was installed.
MSG
    exit 1
fi

if [ $CLEAN = 1 ]; then
    say "removing build/mc and build/check"
    rm -rf build/mc build/check
fi

# --- JDK (Minecraft 26.x needs Java 25). One tarball per platform; the layout differs
# (macOS puts the JDK under Contents/Home), so .tools/java links to the binary wherever it is.
platform=$(uname -s | tr '[:upper:]' '[:lower:]')_$(uname -m)
case "$platform" in
    darwin_arm64)   JDK_URL=$JDK_URL_DARWIN_ARM64;   JDK_SHA256=$JDK_SHA256_DARWIN_ARM64 ;;
    darwin_x86_64)  JDK_URL=$JDK_URL_DARWIN_X86_64;  JDK_SHA256=$JDK_SHA256_DARWIN_X86_64 ;;
    linux_x86_64)   JDK_URL=$JDK_URL_LINUX_X86_64;   JDK_SHA256=$JDK_SHA256_LINUX_X86_64 ;;
    linux_aarch64)  JDK_URL=$JDK_URL_LINUX_AARCH64;  JDK_SHA256=$JDK_SHA256_LINUX_AARCH64 ;;
    *) echo "no JDK pinned for $platform in versions.env" >&2; exit 1 ;;
esac
JAVA=.tools/java
link_java() {
    if [ -x .tools/jdk/Contents/Home/bin/java ]; then
        ln -sfn jdk/Contents/Home/bin/java "$JAVA"
    elif [ -x .tools/jdk/bin/java ]; then
        ln -sfn jdk/bin/java "$JAVA"
    else
        return 1
    fi
}
if link_java && "$JAVA" -version 2>&1 | grep -q "${JDK_VERSION%%+*}"; then
    say "JDK $JDK_VERSION already installed"
else
    say "downloading JDK $JDK_VERSION for $platform"
    tmp=$(mktemp -d)
    curl -fsSL "$JDK_URL" -o "$tmp/jdk.tar.gz"
    echo "$JDK_SHA256  $tmp/jdk.tar.gz" | shasum -a 256 -c -
    rm -rf .tools/jdk && mkdir -p .tools/jdk
    tar -xzf "$tmp/jdk.tar.gz" -C .tools/jdk --strip-components=1
    rm -rf "$tmp"
    link_java || { echo "the JDK tarball has neither Contents/Home/bin/java nor bin/java" >&2; exit 1; }
    "$JAVA" -version 2>&1 | grep -q "${JDK_VERSION%%+*}" || { echo "$JAVA is not JDK $JDK_VERSION" >&2; exit 1; }
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
        # Accepted by whoever ran `tools/setup.sh --accept-eula` (recorded in .tools/eula-accepted).
        echo "eula=true" > eula.txt
        printf 'server-port=25599\nenable-rcon=false\nonline-mode=false\nlevel-type=minecraft\\:flat\n' > server.properties
        # Starts the server once to fetch the libraries; stop it as soon as it is up.
        ../java -Xmx1G -jar fabric-server-launch.jar nogui > install.log 2>&1 &
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
# The libraries came from Maven unverified; their tree must digest to the pin (./check asserts it too).
libraries_sha256=$(cd "$FABRIC" && find libraries -type f | LC_ALL=C sort | xargs shasum -a 256 | shasum -a 256 | cut -d' ' -f1)
if [ "$libraries_sha256" != "$FABRIC_LIBRARIES_SHA256" ]; then
    echo "the Fabric libraries in $FABRIC/libraries digest to $libraries_sha256, versions.env pins $FABRIC_LIBRARIES_SHA256" >&2
    echo "(if the loader pin was bumped on purpose, put the new digest in versions.env)" >&2
    exit 1
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

# --- Verilator (from the system package manager; ./check asserts the version)
if ! command -v verilator >/dev/null; then
    if command -v brew >/dev/null; then
        say "installing Verilator with Homebrew"
        brew install verilator
    elif command -v apt-get >/dev/null; then
        say "installing Verilator with apt (asks for sudo)"
        sudo apt-get install -y verilator
    else
        echo "install Verilator $VERILATOR_VERSION yourself; no package manager found" >&2; exit 1
    fi
fi
say "$(verilator --version)"
verilator --version | grep -q "Verilator $VERILATOR_VERSION " ||
    echo "warning: versions.env pins Verilator $VERILATOR_VERSION; ./check will fail on another version"

# --- Node (not installed here; the version is pinned so a mismatch is said early)
command -v node >/dev/null || { echo "Node $NODE_MAJOR is required (https://nodejs.org); install it and re-run" >&2; exit 1; }
node --version | grep -q "^v$NODE_MAJOR\." || { echo "Node $(node --version) found; versions.env pins Node $NODE_MAJOR" >&2; exit 1; }

# --- Python and Node packages (exact versions from the lock files)
command -v uv >/dev/null || { echo "uv is required (https://docs.astral.sh/uv/); install it and re-run" >&2; exit 1; }
uv sync --frozen
npm ci --no-audit --no-fund

say "setup complete; run ./check --vanilla"
