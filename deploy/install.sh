#!/usr/bin/env bash
# Sets up the live playground on a fresh Debian 12 or Ubuntu 24.04 machine with
# this checkout at /opt/redfab (REDFAB_ROOT to put it elsewhere). Run as root:
#
#   deploy/install.sh DOMAIN
#
# Safe to re-run: every step skips what is already in place. It does these, in
# order: the firewall (22, 80, 443 only; the game, RCON and the stream stay on
# loopback), the `redfab` user, Caddy from its package source, Node 26 from the
# official tarball by checksum, the project's own setup for a server
# (tools/setup.sh --server: the JDK, the server jar, Fabric and the Node
# packages; no Rust, Verilator or Python), the observer mod, the systemd units
# and the Caddyfile for DOMAIN. Two things it never does for you: accept
# Mojang's EULA (run `tools/setup.sh --accept-eula --server` as the redfab user
# yourself; it stops and says so), and build the core (deploy/ship_core.sh
# copies the one built on your machine into dist/; it stops and says so).

set -euo pipefail

DOMAIN=${1:?usage: deploy/install.sh DOMAIN}
ROOT=${REDFAB_ROOT:-/opt/redfab}
cd "$ROOT"
source ./versions.env

say() { printf '== %s\n' "$*"; }
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }

# --- the firewall
if command -v ufw >/dev/null; then
    say "firewall: 22, 80, 443"
    ufw allow OpenSSH >/dev/null
    ufw allow 80/tcp >/dev/null
    ufw allow 443/tcp >/dev/null
    ufw --force enable >/dev/null
else
    say "ufw is not installed; make sure only 22, 80 and 443 are reachable"
fi

# --- the user that runs it
if ! id redfab >/dev/null 2>&1; then
    say "creating the redfab user"
    useradd --system --home-dir "$ROOT" --shell /usr/sbin/nologin redfab
fi
chown -R redfab:redfab "$ROOT"

# --- Caddy (from its own package source, as its documentation says)
if ! command -v caddy >/dev/null; then
    say "installing Caddy"
    apt-get install -y debian-keyring debian-archive-keyring apt-transport-https curl gnupg >/dev/null
    curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
    curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' > /etc/apt/sources.list.d/caddy-stable.list
    apt-get update >/dev/null
    apt-get install -y caddy >/dev/null
fi

# --- Node (the major is pinned in versions.env; the tarball is verified against
# the release's published checksums)
if ! command -v node >/dev/null || ! node --version | grep -q "^v$NODE_MAJOR\."; then
    say "installing Node $NODE_MAJOR"
    apt-get install -y libatomic1 >/dev/null  # Node's Linux build loads it; Ubuntu's cloud image lacks it
    arch=$(uname -m)
    case "$arch" in
        x86_64) node_arch=x64 ;;
        aarch64) node_arch=arm64 ;;
        *) echo "no Node build for $arch" >&2; exit 1 ;;
    esac
    tmp=$(mktemp -d)
    curl -fsSL "https://nodejs.org/dist/latest-v$NODE_MAJOR.x/SHASUMS256.txt" -o "$tmp/SHASUMS256.txt"
    file=$(grep -o "node-v$NODE_MAJOR\.[0-9.]*-linux-$node_arch\.tar\.xz" "$tmp/SHASUMS256.txt" | head -1)
    [ -n "$file" ] || { echo "no Node $NODE_MAJOR tarball for linux-$node_arch in the release's checksums" >&2; exit 1; }
    curl -fsSL "https://nodejs.org/dist/latest-v$NODE_MAJOR.x/$file" -o "$tmp/$file"
    (cd "$tmp" && grep " $file\$" SHASUMS256.txt | sha256sum -c -)
    mkdir -p /usr/local/lib/nodejs
    tar -xJf "$tmp/$file" -C /usr/local/lib/nodejs
    for bin in node npm npx; do
        ln -sfn "/usr/local/lib/nodejs/${file%.tar.xz}/bin/$bin" "/usr/local/bin/$bin"
    done
    rm -rf "$tmp"
fi
say "$(node --version)"

# --- the project's own setup, as the redfab user (the EULA is yours to accept)
if [ ! -f .tools/eula-accepted ]; then
    cat >&2 <<MSG
Running the Minecraft server binds whoever runs it to Mojang's EULA
(https://aka.ms/MinecraftEULA). Read it, then, as the redfab user:

    sudo -u redfab tools/setup.sh --accept-eula --server

and run this script again.
MSG
    exit 1
fi
say "the project's setup for a server"
sudo -u redfab -H env PATH="/usr/local/bin:$PATH" tools/setup.sh --server
say "the observer mod"
sudo -u redfab -H env PATH="/usr/local/bin:$PATH" tools/build_mod.sh >/dev/null

# --- the core, built on your machine (wasm-identical holds it to the native build)
if [ ! -f dist/redfab_wasm.wasm ]; then
    cat >&2 <<MSG
No core in $ROOT/dist/redfab_wasm.wasm. From your checkout, with the Rust toolchain:

    deploy/ship_core.sh root@$DOMAIN

and run this script again.
MSG
    exit 1
fi

# --- the units and the front door
say "systemd and Caddy for $DOMAIN"
install -m 644 deploy/redfab-live.service /etc/systemd/system/redfab-live.service
install -m 644 deploy/redfab-restart.service /etc/systemd/system/redfab-restart.service
install -m 644 deploy/redfab-restart.timer /etc/systemd/system/redfab-restart.timer
sed "s/redfab.example/$DOMAIN/" deploy/Caddyfile > /etc/caddy/Caddyfile
mkdir -p /var/log/redfab && chown redfab:redfab /var/log/redfab
install -m 644 deploy/status_log.cron /etc/cron.d/redfab-status
systemctl daemon-reload
systemctl enable --now redfab-live.service redfab-restart.timer
systemctl reload caddy || systemctl restart caddy

say "done. From any machine with this checkout: node tools/smoke_remote.mjs https://$DOMAIN"
say "logs: journalctl -u redfab-live -f; status: curl -s https://$DOMAIN/api/status"
