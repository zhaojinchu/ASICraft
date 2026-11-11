# The live playground's backend

The playground page with a server behind it: the source goes to the server,
the server builds it, places it in the game, and the page shows the game's
blocks instead of its own simulation. Levers on the page reach probe blocks
in the world; nothing else from a browser ever reaches the game.

    node server/live.mjs [--port 8766] [--host 127.0.0.1] [--plots 8] [--run-dir build/mc/live] [--trust-proxy]
                         [--wasm PATH] [--camera-key KEY] [--relay-api URL] [--game-port N]

That starts a Fabric server with the observer mod (`tools/setup.sh` and
`tools/build_mod.sh` first), then serves the page at http://127.0.0.1:8766/.
Click **Run in the game**. The server does not start until the Minecraft
EULA has been accepted on this machine: `tools/setup.sh --accept-eula`
records that in `.tools/eula-accepted`, and without the file the backend
refuses with the EULA's link. Java is `.tools/java` (the link setup.sh makes)
or, before the link exists, the macOS JDK path.

The Minecraft server is configured from `harness/data/server_profile.json`,
the same file the harness uses: the gamerules that keep the world inert, the
`server.properties` defaults and the JVM arguments. The backend's own
differences go on top, explicitly: its motd, `online-mode=false`,
`gamemode=spectator` with `force-gamemode=true` (the camera's player is the
only one, and the server makes it a spectator as it joins), `view-distance`
16 in place of the profile's 4 (the camera sees 256 blocks; the simulation
distance stays 4, since the plots are forceloaded), and the
`max_block_modifications` gamerule raised for its plot clears (the harness
never fills past the game's default). `tools/live_test.mjs` and
`harness/tests/test_server_profile.py` hold both sides to the file.

## Behind a reverse proxy

The backend listens on 127.0.0.1 and speaks plain HTTP; putting it online
(stage 13) means a reverse proxy in front that terminates TLS. Two things
then matter:

- `--trust-proxy` (`trustProxy` in `createLive`) takes each client's address
  from the first entry of `X-Forwarded-For` instead of the socket. Without
  it, every client behind the proxy shares the proxy's address, and "one
  plot per address" becomes one plot for everyone. Turn it on only behind a
  proxy that sets the header itself, since a client can send one too.
- `--host` is the address the backend binds; keep it at 127.0.0.1 with the
  proxy on the same machine. Origins other than the page's own host go in
  `origins` (`createLive`), for a page served from another domain.

The plot token travels in the URL (`?token=`, and the `/live` upgrade), so
the proxy must serve the page over TLS (`https`, `wss`) or the token is
readable on the way; the backend never logs it (see below).

## What happens to a submission

1. `POST /api/submit` with `{source, top, name}` and nothing else, as
   `application/json`, from the page's own origin. Any other field is
   refused: a submission is source only. Per address: one plot at a time,
   one compile at a time, and so many submissions an hour (30).
2. The source is compiled in a child process (`sandbox.mjs`,
   `compile_child.mjs`): the same pipeline as the page and `redfab build`,
   YoWASP Yosys and the WebAssembly core, with the same limits on source size,
   cells and blocks. The child is killed past the time limit (90 s) and past the
   memory limit (2 GB; a normal compile peaks near 900 MB). Compiles run one at
   a time behind a short queue; beyond it, the answer is 429. A source seen
   before (the same bytes, top and name) is answered from a cache of the last
   32 results, refusals included, so the examples cost nothing after the first
   time and a hostile source cannot hold the compiler twice.
3. A free plot is taken (503 if none). Plots are a row of 512 by 512 cells in
   an otherwise empty world; a design larger than a cell is refused.
   `GET /api/fits?x=W&z=D` says beforehand whether a footprint would fit, and
   `GET /api/status` carries `limits.plotSize`.
4. The structure file the core wrote is placed with `/place template … strict`,
   exactly as the checks place designs, in forceloaded chunks. The backend
   watches the mod's stream until it shows the design in place, and records
   whether what appeared equals the design file (block states compared by
   meaning, `server/trace.mjs`). Then it asks the mod to forget the template
   and deletes the file: the game would otherwise cache every one.
5. The reply carries the plot number, a token for its levers, the design (the
   page draws it), the report, and `levers`: how lever changes are paced (see
   below). The page opens `ws://…/live?plot=N&token=T`. A plot whose
   submitter never connects is released after a minute.

## The stream

The mod sends every block change in the overworld with its tick. The backend
keeps each plot's blocks as the stream says, and relays changes inside the
plot to its viewers in design coordinates, with the lamp and lever states:

    {"tick": 1234, "changes": [["x,y,z", "state"], ...], "lamps": [0, 1], "inputs": [1, 0]}

A viewer holding the token sends `{"set": "a", "on": true}`; the backend sets
the probe block of input `a` with one `setblock`, and answers
`{"ok": true, "set": "a", "on": true, "tick": T, "queued": false}`. Lever
changes are paced by the design's timing: a combinational design takes one
change per input every `input_spacing` ticks (its torches' burnout budget,
`levers.spacing`), a clocked one takes changes `input_change_at` ticks after
an active edge (the window its flip-flops were measured for,
`levers.window` and `levers.at`). A change asked for before its slot is
`queued` for the tick `T` in the answer, and a newer request for the same
input replaces it, so toggling as fast as the socket allows costs the server
one command per slot, in a clocked design one per input per window: a second
request in the change tick itself waits for the next edge's window. Lever
changes the stream never confirms are forgotten after 30 s, and at most a few
per input are ever pending (the clock flip in flight is never among those
forgotten).

For a clocked design the backend drives the clock from the stream: an edge
due at tick T goes out as tick T's message arrives, so it lands between T and
T+1, which the trace format counts as the last thing in tick T. The clock's
commands jump the RCON queue, so no backlog of lever commands delays an
edge. There is one RCON connection, on purpose: the vanilla server runs every
RCON client's commands through one shared console buffer, so a second
connection gets the first one's replies glued to its own, or none at all
(seen live: a placement judged failed because a lever's reply arrived with
it). The stream shows where each edge actually landed; the next
edge is timed from there (a late edge stretches a half-period rather than
shortening the next one), and `GET /api/plots/N` reports `clock`: edges,
how many landed off schedule, the largest jitter, and the next edge's tick.
The mod flushes what changes between ticks under the number of the tick just
completed, so a command's effects never mix with the next tick's.

Everything since placement is a recording in the trace format:
`GET /api/plots/N/trace?token=T` (the owner's token) returns the fixture (the
design, placed strictly, with every lever change as an event at its tick) and
the trace (every change the stream reported, by tick). It is capped at
500,000 changes; past that the export is refused. `redfab check-fixtures`
replays the fixture in the simulator, and the checks require the replay to
equal the recording tick for tick: that is how the live server is held to the
same standard as everything else. `GET /api/plots/N/structure?token=T` is the
placed structure file, for the page's download.

A plot is released by its owner (`DELETE /api/plots/N?token=T`), after ten
minutes with nobody controlling it, or after an hour; the backend fills it
with air. A plot whose fill fails is `broken`, out of service, and retried
every 15 s until it clears. Viewers are capped per plot (16) and per address
(8), and a viewer that stops reading is dropped once 256 KB are waiting for
it. `/live` needs an explicit plot number and the page's origin.

## The camera

With `--camera-key KEY` (or `REDFAB_CAMERA_KEY` in the environment) the
backend takes one more socket, `/camera?key=KEY`: the camera mod
(`mod-camera/`) running in the user's own game client on their PC, joined to
the server through an SSH tunnel as a spectator, whose view OBS streams to a
relay on the box and the page plays (stage 14 of `docs/PLAN.md`;
`server/camera.mjs`). The mod says `{"hello": "redfab-camera", "player":
NAME}`; the backend puts the player on the run's `whitelist.json` by the
UUID an offline server gives it and runs `whitelist reload` (26.3's
whitelist is on by default, and nobody reads the run's RCON password for
it; `whitelist add NAME` would list the account's online UUID, which the
offline server never sees) and answers `{"hello": "redfab-live", "frame":
...}`. The hello comes from the title screen, before the player joins, so
spectator mode is the server's own (`force-gamemode`), not a command. The
game's port is a free one at every start unless `--game-port N` fixes it
(still on loopback), which a deployment with a camera does, so the PC's
tunnel can name it. From then on it sends the mod where
to look: `{"frame": {"plot": N, "origin": [x, y, z], "size": [sx, sy, sz]}}`
on every placement and lever change while nobody drives, and a driver's
inputs, `{"drive": {"move": [x, y, z], "look": [yaw, pitch], "plot": N,
"owner": true|false}}`, as they come. One mod at a time: a newer one's hello
replaces the one connected, whose socket the backend closes (a game killed
without closing its socket leaves it open on the box for minutes); the wrong
key is 403; without a key there is no socket (404).

Driving is asked for on a plot's `/live` socket: `{"drive": "take"}`,
`{"drive": "release"}`, or `{"drive": {"move": [x, y, z], "look": [yaw,
pitch]}}` with the move's numbers from -1 to 1 and the look in degrees. The
plot's owner (the token holder) drives at once and for as long as the plot is
theirs, taking the camera from whoever has it (who goes to the head of the
queue); any other viewer gets a 60-second turn when the camera is free, or a
place in the queue (`{"drive": "queued", "position": P}`); ten seconds without
input end a turn; a turn ends with `{"drive": "over", "why": ...}` and the
next in the queue hears `{"drive": "yours", "seconds": S}` (`null` for the
owner); when nobody holds it, the automatic camera frames the plot last
placed or touched again. An input from a viewer not driving is refused with
`not driving`; a malformed one with what a drive message is. `GET /api/status`
carries `camera`: `connected`, `player`, `driver` (`{plot, owner}` or null),
`queue`, `plot` (the one framed) and `streaming`, which `--relay-api URL`
(the relay's API on loopback) answers by asking whether the `camera` path is
being published, every five seconds; without a relay API it is null.

## When the game goes wrong

`GET /api/status` says `live: false` as soon as the stream is older than five
seconds, an RCON connection is down, or the Java process is gone, with the
details under `minecraft`. The stream reconnects on its own (one attempt at a
time, backing off to five seconds); when it comes back, every plot is released
as stale, because what the server did meanwhile was missed. Both RCON
connections re-dial and authenticate again after a close or a 30 s timeout;
the commands in hand are rejected at once rather than hung. When the Java
process exits on its own, `server/live.mjs` exits with status 2, so whatever
supervises it restarts the whole thing. The run directory keeps `server.pid`.

Logs carry a timestamp and never the query string of a request (the token
travels there); a failing request is logged by method and path alone.

## Files

- `live.mjs`: the HTTP and WebSocket front, `createLive` (any world: the
  checks give it a fake one) and `startLive` (launches Minecraft). Static
  files go through the playground's `serveStatic` (`tools/playground_serve.mjs`):
  the two packages under `/pkg/NAME@VERSION/` with immutable caching, the
  core with an ETag, the page and designs never cached. Bumping a package in
  `package.json` means moving the pin in `playground/index.html` or
  `playground/worker.js` too (`tools/playground_test.mjs` fails otherwise).
- `plots.mjs`: plots, their blocks as the stream says, levers and their
  pacing, the clock driver, viewers and the recording.
- `trace.mjs`: positions, canonical block states and trace comparison, shared
  with `tools/live_common.mjs`.
- `minecraft.mjs`: launches the Fabric server the way the harness does, with
  the world the backend talks to: place, forget, setblock (two lanes), fill,
  forceload, tick query, the stream, and `health()`.
- `sandbox.mjs`, `compile_child.mjs`: compiles in a child process with time
  and memory limits, behind a cache keyed by the source's hash.
- `rcon.mjs`, `ws.mjs`: the RCON client (re-dials) and a small WebSocket
  server (with backpressure), no dependencies.
- `camera.mjs`: the camera mod's socket, the automatic framing, the drive
  relay and the turn policy, and the relay's status (see The camera above).
