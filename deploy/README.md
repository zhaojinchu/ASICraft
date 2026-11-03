# Putting the live playground online

Stage 13 of `docs/PLAN.md` ("Stage 13 (planned 2026-11-02)"): one machine runs
the backend (`server/live.mjs`), which starts the Fabric server with the observer
mod and serves the page; Caddy in front terminates TLS; `systemd` keeps the
backend up. The game, RCON and the mod's stream stay on loopback: the page is the
only window into the world, which is what makes `online-mode=false` acceptable.

## What you need

- A Debian 12 or Ubuntu 24.04 machine with 8 GB of memory (the game runs at
  `-Xmx2G`, a compile child peaks near 900 MB under a 2 GB cap, the backend keeps
  the plots' recordings; 4 GB swaps) and two to four cores. Ports 22, 80 and
  443 reachable, nothing else.
- A name pointing at it (an A record, and AAAA if the machine has IPv6). Not a
  name with "Minecraft" in it.
- This checkout at `/opt/redfab` on the machine, and on your own machine with the
  Rust toolchain (for the core) and `ssh` access to the box.

## Install

On the machine, as root:

    apt-get update && apt-get install -y git ufw
    git clone https://github.com/zhaojinchu/ASICraft /opt/redfab
    /opt/redfab/deploy/install.sh NAME

`install.sh` stops twice, on purpose, and tells you what to do:

1. The EULA. Running Mojang's server binds whoever runs it to
   https://aka.ms/MinecraftEULA; the setup never accepts it for you. As the
   `redfab` user, `tools/setup.sh --accept-eula --server` records your acceptance
   in `.tools/eula-accepted`, then run `install.sh` again.
2. The core. The backend's compiles and the page use the WebAssembly build of
   the core, which `./check` holds byte-identical to the native one
   (`wasm-identical`); it is built on your machine and shipped, so the box needs
   no Rust: from your checkout, `deploy/ship_core.sh root@NAME`, then run
   `install.sh` again.

The second run installs the units and the Caddyfile and starts everything. Caddy
fetches the certificate on the first request; within a minute:

    curl -s https://NAME/api/status
    node tools/smoke_remote.mjs https://NAME      # from any machine with this checkout

The smoke submits the inverter example, opens its plot's stream with the token,
flips the lever, waits for the lamp to follow and back, and releases the plot;
it exits 0 only if every step passed in time.

## Running it

- Logs: `journalctl -u redfab-live -f` (the backend's lines, with the game's
  console under them; no request carries its query string, so no token is ever
  logged). Status every minute in `/var/log/redfab/status.log`.
- `live: false` in the status means the stream is older than five seconds, RCON
  is down, or the Java process is gone; the backend reconnects on its own, and
  when the game dies it exits 2 and `systemd` restarts both. If it keeps dying,
  the journal says why.
- Updates: on the machine, as root,
  `cd /opt/redfab && git pull && sudo -u redfab npm ci --no-audit --no-fund && sudo -u redfab tools/build_mod.sh`,
  from your machine `deploy/ship_core.sh root@NAME` if the core changed, then
  `systemctl restart redfab-live`. A restart releases every plot; do it when the
  status shows none in use.
- A weekly timer (`redfab-restart.timer`, Sunday 09:00 UTC) restarts the
  backend if no plot is in use, so a long run's leaks in the Java process are
  cleared without cutting anyone off.
- The limits are the backend's (`server/README.md`): 8 plots, one plot and one
  compile per address, 30 submissions an hour, 90 s per compile, viewers capped
  per plot and per address. More plots: `--plots` in the unit file, then
  `systemctl daemon-reload && systemctl restart redfab-live`.
- Nothing needs backing up: the world is placed by command and filled with air
  again, the run directory is rebuilt at every start, and submitted sources are
  kept only as the placed structure and the recording while the plot is in use.

## The camera (stage 14)

The user's own game client, on their PC, joined to the server as a spectator
and streamed to the page; `deploy/CAMERA.md` is the PC's side. On the box,
`install.sh` adds the video relay (MediaMTX, `redfab-relay.service`, its
config from `mediamtx.yml` with the site's name and a generated password put
in, at `/etc/redfab/mediamtx.yml`), opens 8189 udp and tcp in ufw for the
media, and makes the two secrets once: `/etc/redfab/camera.env` (the mod's
key, `REDFAB_CAMERA_KEY`, which the backend's unit reads) and
`/etc/redfab/relay.env` (the publisher's password). Two things it cannot do:
**open 8189 udp and tcp in the cloud's network rules** (the Azure network
security group, beside 22, 80 and 443), and make the tunnel user the PC
connects through (`CAMERA.md`). Caddy passes `/camera/*` to the relay with
the prefix stripped, so the page plays `https://NAME/camera/camera/whep` and
OBS publishes to `https://NAME/camera/camera/whip`; the backend asks the
relay's API on loopback whether the stream is up and says so in
`/api/status` under `camera`. The smoke posts to the WHEP endpoint and fails
on a 502, which means the relay is down. Every viewer of the stream costs
about $0.10 an hour of egress at 3 Mbps, which is why the page plays only on
click and stops while hidden.

## Files

- `redfab-live.service`: the backend under `systemd` (`Restart=always`, loopback,
  `--trust-proxy`, the shipped core, a memory cap, the camera key from
  `/etc/redfab/camera.env`, the relay's API for the status).
- `redfab-relay.service`, `mediamtx.yml`: the video relay (MediaMTX) under
  `systemd` and its config: WebRTC only, the HTTP side on loopback behind
  Caddy, the media on 8189, reads open and publishing behind the password.
- `CAMERA.md`: the PC's runbook, from a clone on the PC: Prism Launcher with
  the camera mod, the tunnel, OBS, the two scheduled tasks.
- `pc/`: the PC's scripts: `tunnel.ps1` (the tunnel in a loop), `start.ps1`
  (the game and OBS started, or restarted), `install.ps1` (both registered
  as scheduled tasks), and `config.example.ps1`, copied to a git-ignored
  `config.ps1` and filled in.
- `Caddyfile`: the front door; `install.sh` puts the name in and installs it as
  `/etc/caddy/Caddyfile`.
- `install.sh`: the steps above, idempotent.
- `ship_core.sh`: builds the core here and copies it to the box with its checksum.
- `status_log.sh`, `status_log.cron`: the status, one line a minute.
- `restart_when_idle.sh`, `redfab-restart.service`, `redfab-restart.timer`: the
  weekly restart, only when idle.

NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR
MICROSOFT.
